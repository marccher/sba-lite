use crate::state::{build_application_groups, MonitorState};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header::ACCEPT, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json, Response,
    },
};
use futures_util::stream::{Stream, StreamExt};
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

/// GET /applications — content negotiation:
/// - if the browser asks for "text/event-stream" (native EventSource),
///   respond with an SSE stream that publishes a new snapshot on every
///   poll cycle;
/// - otherwise respond with plain JSON, as before (compatible with curl,
///   scripts, one-off calls).
pub async fn applications_handler(
    headers: HeaderMap,
    State(state): State<MonitorState>,
) -> Response {
    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if accept.contains("text/event-stream") {
        sse_stream(state).await.into_response()
    } else if accept.contains("text/html") {
        // A plain browser refresh/navigation on this exact path (e.g.
        // hitting F5 while on /applications) sends Accept: text/html, not
        // an XHR/fetch Accept header — serve the SPA shell instead of raw
        // JSON, since this exact route otherwise takes priority over the
        // static-file fallback in main.rs.
        crate::spa_shell()
    } else {
        Json(build_application_groups(&state).await).into_response()
    }
}

async fn sse_stream(state: MonitorState) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // The real backend doesn't send an initial snapshot on this channel
    // (confirmed by the capture: only ":ping" until an application's
    // status actually changes) — the initial list comes from the separate
    // XHR call. Here we only forward real state-change events, one per
    // ApplicationGroup, in exactly the same format observed from the real
    // Java backend.
    let rx = state.subscribe();
    let live = BroadcastStream::new(rx).filter_map(|msg| async move {
        match msg {
            Ok(json) => Some(Ok(Event::default().data(json))),
            Err(_) => None, // receiver too slow / lagging: we ignore the gap
        }
    });

    Sse::new(live).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// GET /instances/{id}/health-groups — names of the configured health
/// groups (e.g. "liveness", "readiness"). Spring Boot exposes them as a
/// top-level "groups" field in the /actuator/health response itself, so we
/// derive them from there instead of making a separate call to the target.
pub async fn health_groups_handler(
    State(state): State<MonitorState>,
    Path(id): Path<String>,
) -> Response {
    let instances = state.instances.read().await;
    match instances.get(&id) {
        Some(view) => {
            let groups = view
                .status_info
                .details
                .get("groups")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(vec![]));
            Json(groups).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// GET /instances/events — the full historical journal (Journal view of
/// the UI). Same as /applications: SSE if the browser asks for it (a new
/// event for every JournalEvent recorded, otherwise just pings), plain
/// JSON for the initial fetch.
pub async fn journal_handler(headers: HeaderMap, State(state): State<MonitorState>) -> Response {
    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if accept.contains("text/event-stream") {
        let rx = state.subscribe_journal();
        let live = BroadcastStream::new(rx).filter_map(|msg| async move {
            match msg {
                Ok(json) => Some(Ok::<Event, Infallible>(Event::default().data(json))),
                Err(_) => None,
            }
        });

        Sse::new(live)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(15))
                    .text("ping"),
            )
            .into_response()
    } else if accept.contains("text/html") {
        // Same reasoning as applications_handler: a browser refresh on
        // this exact path would otherwise get raw JSON instead of the SPA
        // shell.
        crate::spa_shell()
    } else {
        let journal = state.journal.read().await;
        Json(journal.clone()).into_response()
    }
}

/// GET /instances/{id} — detail of a single instance. This exact path also
/// matches a real Vue Router route (/instances/:instanceId), so the same
/// content-negotiation fix as applications_handler applies here too.
pub async fn instance_detail_handler(
    headers: HeaderMap,
    State(state): State<MonitorState>,
    Path(id): Path<String>,
) -> Response {
    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if accept.contains("text/html") {
        return crate::spa_shell();
    }

    let instances = state.instances.read().await;
    match instances.get(&id) {
        Some(view) => Json(view.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// ANY /instances/{id}/actuator/{*path} — direct proxy to the instance's
/// real actuator, completely bypassing any Java backend for this call.
pub async fn instance_actuator_proxy(
    State(state): State<MonitorState>,
    Path((id, path)): Path<(String, String)>,
    req: Request,
) -> Response {
    let (base_url, bearer_token) = match state.actuator_target(&id) {
        Some(t) => t,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    // Spring Boot 3.x (Spring Framework 6) no longer automatically matches
    // "/env" and "/env/": we normalize by trimming any trailing slash to
    // avoid a 404 "No static resource" when the UI calls with a trailing
    // slash.
    let normalized_path = path.trim_end_matches('/');
    let target_url = format!("{base_url}/{normalized_path}{query}");

    let method = req.method().clone();
    let headers = req.headers().clone();

    tracing::debug!(target: "sbalite::proxy", "{method} {target_url}");

    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let mut upstream_req = state
        .http_client
        .request(method, &target_url)
        .body(body_bytes.to_vec());

    for (name, value) in headers.iter() {
        // The browser's "authorization" header makes no sense for the
        // remote target (the browser doesn't know the actuator's token):
        // we ignore it and only use the one statically configured for the
        // instance, if any.
        if crate::is_hop_by_hop(name.as_str())
            || name.as_str().eq_ignore_ascii_case("host")
            || name.as_str().eq_ignore_ascii_case("authorization")
        {
            continue;
        }
        upstream_req = upstream_req.header(name, value);
    }

    if let Some(token) = bearer_token.as_deref() {
        upstream_req = upstream_req.bearer_auth(token);
    }

    match upstream_req.send().await {
        Ok(resp) => {
            let status = resp.status();
            tracing::debug!(target: "sbalite::proxy", "  -> {status} {target_url}");
            let mut out_headers = HeaderMap::new();
            for (name, value) in resp.headers().iter() {
                if crate::is_hop_by_hop(name.as_str())
                    || name.as_str().eq_ignore_ascii_case("content-length")
                {
                    continue;
                }
                out_headers.insert(name.clone(), value.clone());
            }
            let bytes = resp.bytes().await.unwrap_or_default();

            const TRACE_BODY_MAX_BYTES: usize = 8192;
            if bytes.len() <= TRACE_BODY_MAX_BYTES {
                tracing::trace!(
                    target: "sbalite::proxy",
                    "  body: {}",
                    String::from_utf8_lossy(&bytes)
                );
            } else {
                tracing::trace!(
                    target: "sbalite::proxy",
                    "  body: <{} bytes, omitted from trace (threshold {TRACE_BODY_MAX_BYTES} bytes)>",
                    bytes.len()
                );
            }

            let mut builder = Response::builder().status(status);
            for (name, value) in out_headers.iter() {
                builder = builder.header(name, value);
            }
            builder.body(Body::from(bytes)).unwrap()
        }
        Err(e) => {
            tracing::error!("Actuator proxy error towards {target_url}: {e}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}
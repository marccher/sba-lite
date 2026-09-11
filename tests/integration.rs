//! End-to-end integration tests: each test starts a real `sbalite` server
//! (via `sbalite::build_app`, the exact same function `main.rs` uses) bound
//! to an OS-assigned ephemeral port, and talks to it over real HTTP with
//! `reqwest` — no mocked router, no `tower::oneshot` shortcuts.
//!
//! Tests that exercise the actuator proxy also spin up a second, tiny axum
//! server that stands in for a "remote Spring Boot Actuator" target.

use axum::{routing::get, Json, Router};
use futures_util::StreamExt;
use sbalite::{
    auth::AuthConfig,
    config::{InstanceConfig, InstancesConfig},
    state::{run_poll_once, MonitorState},
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpListener;

/// Starts `app` on an OS-assigned free port and returns its base URL
/// (e.g. "http://127.0.0.1:54321"). The server keeps running for the
/// remainder of the test process (background tokio task) — fine for
/// short-lived test binaries, no explicit shutdown needed.
async fn spawn_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// Spawns a minimal fake Spring Boot Actuator: enough for our poller and
/// proxy to talk to (discovery root with `_links`, plus `/health`). The
/// health status is controlled by the caller via `set_status`.
async fn spawn_fake_actuator() -> (String, tokio::sync::watch::Sender<&'static str>) {
    let (status_tx, status_rx) = tokio::sync::watch::channel("UP");

    let app = Router::new()
        .route(
            "/actuator",
            get(|| async move {
                Json(json!({
                    "_links": {
                        "self": { "href": "/actuator" },
                        "health": { "href": "/actuator/health" }
                    }
                }))
            }),
        )
        .route(
            "/actuator/health",
            get(move || {
                let rx = status_rx.clone();
                async move {
                    let status = *rx.borrow();
                    Json(json!({ "status": status }))
                }
            }),
        );

    let base_url = spawn_server(app).await;
    (format!("{base_url}/actuator"), status_tx)
}

fn test_monitor_state(instances: Vec<InstanceConfig>) -> MonitorState {
    let cfg = InstancesConfig {
        server: Default::default(),
        instances,
    };
    MonitorState::new(cfg, reqwest::Client::new())
}

fn no_auth() -> AuthConfig {
    AuthConfig { credentials: None }
}

#[tokio::test]
async fn applications_returns_json_by_default() {
    let state = test_monitor_state(vec![]);
    let app = sbalite::build_app(state, no_auth());
    let base = spawn_server(app).await;

    let resp = reqwest::get(format!("{base}/applications")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.is_array());
}

#[tokio::test]
async fn applications_returns_html_shell_on_browser_refresh() {
    // Regression test for the bug found in practice: a plain browser
    // refresh on this exact path (Accept: text/html) must get the SPA
    // shell, not raw JSON — the exact route otherwise takes priority over
    // the static-file fallback.
    let state = test_monitor_state(vec![]);
    let app = sbalite::build_app(state, no_auth());
    let base = spawn_server(app).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/applications"))
        .header("Accept", "text/html")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(content_type.starts_with("text/html"));

    let body = resp.text().await.unwrap();
    assert!(body.to_lowercase().contains("<html"));
}

#[tokio::test]
async fn basic_auth_rejects_without_credentials_and_accepts_with_them() {
    let state = test_monitor_state(vec![]);
    let auth = AuthConfig {
        credentials: Some(("admin".to_string(), "secret".to_string())),
    };
    let app = sbalite::build_app(state, auth);
    let base = spawn_server(app).await;

    let client = reqwest::Client::new();

    let unauthorized = client
        .get(format!("{base}/applications"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);
    assert!(unauthorized.headers().get("www-authenticate").is_some());

    let authorized = client
        .get(format!("{base}/applications"))
        .basic_auth("admin", Some("secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(authorized.status(), 200);

    let wrong_password = client
        .get(format!("{base}/applications"))
        .basic_auth("admin", Some("not-the-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_password.status(), 401);
}

#[tokio::test]
async fn actuator_proxy_forwards_requests_to_the_real_upstream() {
    let (actuator_base_url, _status_tx) = spawn_fake_actuator().await;

    let instance = InstanceConfig {
        name: "fake-service".to_string(),
        actuator_base_url,
        bearer_token: None,
    };
    let state = test_monitor_state(vec![instance]);
    let app = sbalite::build_app(state, no_auth());
    let base = spawn_server(app).await;

    let resp = reqwest::get(format!("{base}/instances/fake-service/actuator/health"))
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // Pure pass-through: the proxy must not transform this response (that
    // transformation only happens in the poller's own health check, not
    // in the on-demand actuator proxy).
    assert_eq!(body, json!({ "status": "UP" }));
}

#[tokio::test]
async fn actuator_proxy_returns_404_for_an_unknown_instance() {
    let state = test_monitor_state(vec![]);
    let app = sbalite::build_app(state, no_auth());
    let base = spawn_server(app).await;

    let resp = reqwest::get(format!("{base}/instances/does-not-exist/actuator/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn applications_sse_delivers_a_real_event_when_status_changes() {
    let (actuator_base_url, status_tx) = spawn_fake_actuator().await;

    let instance = InstanceConfig {
        name: "flaky-service".to_string(),
        actuator_base_url,
        bearer_token: None,
    };
    let state = test_monitor_state(vec![instance]);

    // Baseline poll: the instance is "new", so this only establishes the
    // starting state (UP) — no SSE event is expected to be emitted for it.
    run_poll_once(&state).await;

    let app = sbalite::build_app(state.clone(), no_auth());
    let base = spawn_server(app).await;

    let client = reqwest::Client::new();
    let sse_response = client
        .get(format!("{base}/applications"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(sse_response.status(), 200);

    let mut stream = sse_response.bytes_stream();

    // Flip the fake actuator's health to DOWN and re-poll: this is a real
    // status change, so it must be pushed to the open SSE connection.
    status_tx.send("DOWN").unwrap();
    run_poll_once(&state).await;

    let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("timed out waiting for an SSE event")
        .expect("stream ended without producing an event")
        .expect("error reading SSE chunk");

    let text = String::from_utf8_lossy(&chunk);
    assert!(
        text.contains("\"status\":\"DOWN\""),
        "expected the SSE event to report the new DOWN status, got: {text}"
    );
}

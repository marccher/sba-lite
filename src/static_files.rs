use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

/// The Vue UI, extracted statically and embedded into the executable at
/// compile time. The "ui-dist/" folder must exist (containing index.html
/// and the assets) BEFORE running "cargo build" — it's no longer enough
/// for it to exist only at runtime.
///
/// Note: in "debug" builds (cargo run / cargo build / cargo test without
/// --release), rust-embed defaults to reading files from disk on every
/// request instead of embedding them — handy for iterating on the UI
/// without recompiling, and it's also what integration tests exercise.
/// Only "--release" builds produce a truly self-contained binary that
/// doesn't need the ui-dist folder alongside it.
#[derive(RustEmbed)]
#[folder = "ui-dist/"]
struct UiAssets;

/// Serves the SPA's index.html shell directly. Used both by the static
/// fallback below and by handlers.rs (applications_handler,
/// journal_handler, instance_detail_handler) for their own content
/// negotiation: an exact route like /applications or /instances/events
/// takes priority over this fallback in Axum's routing, so a plain
/// browser refresh (Accept: text/html) on those paths would otherwise hit
/// the JSON/SSE handler directly instead of the page shell.
pub fn spa_shell() -> Response {
    match UiAssets::get("index.html") {
        Some(file) => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(file.data.into_owned()))
            .unwrap(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Serves the Vue UI embedded in the binary. If the requested file doesn't
/// exist but the path has no extension (so it's not a static asset like
/// .js/.css/.png), we assume it's a Vue Router client-side route (e.g.
/// /applications/xyz, /journal, /wallboard) and serve index.html anyway,
/// so refreshing or deep-linking to those pages works. In any other case,
/// 404 — there's no Java backend left to fall back to.
pub async fn handle_request(req: Request) -> Response {
    let uri_path = req.uri().path().to_string();

    match try_serve_embedded(&uri_path) {
        Some(response) => response,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn try_serve_embedded(uri_path: &str) -> Option<Response> {
    if uri_path.contains("..") {
        return None;
    }

    let relative = uri_path.trim_start_matches('/');
    let candidate = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };

    if let Some(file) = UiAssets::get(candidate) {
        let mime = mime_guess::from_path(candidate).first_or_octet_stream();
        return Some(
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", mime.as_ref())
                .body(Body::from(file.data.into_owned()))
                .unwrap(),
        );
    }

    let last_segment = relative.rsplit('/').next().unwrap_or(relative);
    if !last_segment.contains('.') && let Some(file) = UiAssets::get("index.html") {
        return Some(
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html; charset=utf-8")
                .body(Body::from(file.data.into_owned()))
                .unwrap(),
        );
    }

    None
}

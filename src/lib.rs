pub mod auth;
pub mod config;
pub mod handlers;
pub mod model;
pub mod state;
pub mod static_files;

use auth::AuthConfig;
use axum::{middleware, routing::get, Router};
use state::MonitorState;

/// Assembles the full application router: the monitor API routes (state,
/// journal, actuator proxy), the embedded-UI static fallback, and the
/// optional Basic Auth middleware wrapping everything.
///
/// This is the single source of truth for how the app is wired together —
/// both `main.rs` and the integration tests under `tests/` call this, so
/// tests exercise exactly the same routing as production, not a
/// hand-rolled approximation of it.
pub fn build_app(monitor_state: MonitorState, auth: AuthConfig) -> Router {
    let monitor_router = Router::new()
        .route(
            "/applications",
            get(handlers::applications_handler).post(handlers::applications_post_handler),
        )
        .route("/instances/events", get(handlers::journal_handler))
        .route("/instances/:id", get(handlers::instance_detail_handler))
        .route(
            "/instances/:id/health-groups",
            get(handlers::health_groups_handler),
        )
        .route(
            "/instances/:id/actuator/*path",
            axum::routing::any(handlers::instance_actuator_proxy),
        )
        .with_state(monitor_state);

    let static_router = Router::new().fallback(static_files::handle_request);

    Router::new()
        .merge(monitor_router)
        .merge(static_router)
        .layer(middleware::from_fn_with_state(auth, auth::require_basic_auth))
}
mod config;
mod handlers;
mod model;
mod state;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use base64::Engine;
use rust_embed::RustEmbed;
use state::MonitorState;
use std::net::SocketAddr;

/// The Vue UI, extracted statically and embedded into the executable at
/// compile time. The "ui-dist/" folder must exist (containing index.html
/// and the assets) BEFORE running "cargo build" — it's no longer enough
/// for it to exist only at runtime.
///
/// Note: in "debug" builds (cargo run / cargo build without --release),
/// rust-embed defaults to reading files from disk on every request instead
/// of embedding them — handy for iterating on the UI without recompiling.
/// Only "--release" builds produce a truly self-contained binary that
/// doesn't need the ui-dist folder alongside it.
#[derive(RustEmbed)]
#[folder = "ui-dist/"]
struct UiAssets;

#[derive(Clone)]
struct AuthConfig {
    /// If present, holds the expected (username, password). If None, the
    /// server remains unprotected (default behavior).
    credentials: Option<(String, String)>,
}

/// HTTP Basic Auth middleware: if auth.credentials is None, everything
/// passes through unchecked. Otherwise it requires an "Authorization:
/// Basic <base64(user:pass)>" header matching exactly the configured
/// credentials, responding 401 with WWW-Authenticate otherwise (the
/// browser then shows its native login prompt).
async fn require_basic_auth(State(auth): State<AuthConfig>, req: Request, next: Next) -> Response {
    let Some((expected_user, expected_pass)) = &auth.credentials else {
        return next.run(req).await;
    };

    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|creds| {
            creds
                .split_once(':')
                .map(|(u, p)| (u.to_string(), p.to_string()))
        })
        .map(|(user, pass)| &user == expected_user && &pass == expected_pass)
        .unwrap_or(false);

    if authorized {
        next.run(req).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"sbalite\"")
            .body(Body::empty())
            .unwrap()
    }
}

#[tokio::main]
async fn main() {
    let instances_config_path =
        std::env::var("INSTANCES_CONFIG").unwrap_or_else(|_| "instances.toml".to_string());

    // Load the config BEFORE anything else: the [server] section inside it
    // provides the defaults for port, poll interval, TLS and log level.
    // Environment variables, if set, still take precedence.
    let instances_cfg = config::load(&instances_config_path).unwrap_or_else(|e| {
        eprintln!(
            "Warning: could not load {instances_config_path} ({e}). \
             Starting with an empty configuration: no instances will be \
             monitored until you create/fix the file."
        );
        config::InstancesConfig::default()
    });
    let server_cfg = instances_cfg.server.clone();

    // Log level: RUST_LOG (if set) always takes precedence and is meant
    // for advanced users who want a custom directive (e.g.
    // "sbalite=debug,reqwest=debug"). For the average user, just set
    // log_level = "info" | "debug" | "trace" in instances.toml: it's
    // applied ONLY to this program's calls to remote servers, without the
    // noise from dependencies (reqwest, hyper, etc.).
    let log_filter = std::env::var("RUST_LOG").ok().unwrap_or_else(|| {
        let level = server_cfg
            .log_level
            .clone()
            .unwrap_or_else(|| "info".to_string());
        format!("sbalite={level}")
    });
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(log_filter))
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .or(server_cfg.port)
        .unwrap_or(9000);
    let bind_addr: SocketAddr = format!("0.0.0.0:{port}").parse().expect("invalid port");

    let poll_interval_secs: u64 = std::env::var("POLL_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .or(server_cfg.poll_interval_secs)
        .unwrap_or(10);

    // WARNING: disables TLS certificate verification for ALL proxy calls
    // (poller + actuator proxy). Only useful for development targets with
    // self-signed certificates. DO NOT use in production.
    let insecure_tls = std::env::var("INSECURE_TLS")
        .ok()
        .map(|v| v == "true" || v == "1")
        .or(server_cfg.insecure_tls)
        .unwrap_or(false);

    let http_client = {
        let mut builder = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure_tls)
            .user_agent("sbalite/0.1 (+monitoring client)");
    
        if let Some(ca_paths) = &server_cfg.ca_cert_path {
            for ca_path in ca_paths.clone().into_vec() {
                match std::fs::read(&ca_path) {
                    Ok(pem_bytes) => match reqwest::Certificate::from_pem(&pem_bytes) {
                        Ok(cert) => {
                            builder = builder.add_root_certificate(cert);
                            println!("Trusting additional CA certificate from {ca_path}");
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: {ca_path} is not a valid PEM certificate ({e}); ignoring."
                            );
                        }
                    },
                    Err(e) => {
                        eprintln!(
                            "Warning: could not read ca_cert_path entry {ca_path} ({e}); ignoring."
                        );
                    }
                }
            }
        }
    
        builder.build().expect("reqwest client")
    };

    if insecure_tls {
        eprintln!(
            "WARNING: insecure_tls is enabled — TLS certificate verification is disabled. \
             For local development only, do not use in production."
        );
    }

    let monitor_state = MonitorState::new(instances_cfg, http_client);
    state::spawn_poller(monitor_state.clone(), poll_interval_secs);
    let instance_count = monitor_state.configs.len();

    let auth_config = AuthConfig {
        credentials: match (&server_cfg.auth_username, &server_cfg.auth_password) {
            (Some(user), Some(pass)) => Some((user.clone(), pass.clone())),
            _ => None,
        },
    };
    if auth_config.credentials.is_some() {
        println!("Basic Auth enabled: access is protected by username/password.");
    }

    let monitor_router = Router::new()
        .route("/applications", get(handlers::applications_handler))
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

    let static_router = Router::new().fallback(handle_request);

    let app = Router::new()
        .merge(monitor_router)
        .merge(static_router)
        .layer(middleware::from_fn_with_state(
            auth_config,
            require_basic_auth,
        ));

    println!(
        "sbalite v{} — {instance_count} instances configured, polling every {poll_interval_secs}s (log_level={})",
        env!("CARGO_PKG_VERSION"),
        server_cfg.log_level.as_deref().unwrap_or("info"),
    );
    println!("Rust server listening on {bind_addr}");

    let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Hop-by-hop headers (RFC 7230 §6.1): used by the per-instance actuator
/// proxy in handlers.rs (no longer used here: there is no proxy to a Java
/// backend anymore).
pub fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// Serves the Vue UI embedded in the binary. If the requested file doesn't
/// exist but the path has no extension (so it's not a static asset like
/// .js/.css/.png), we assume it's a Vue Router client-side route (e.g.
/// /applications/xyz, /journal, /wallboard) and serve index.html anyway,
/// so refreshing or deep-linking to those pages works. In any other case,
/// 404 — there's no Java backend left to fall back to.
async fn handle_request(req: Request) -> Response {
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
    if !last_segment.contains('.') {
        if let Some(file) = UiAssets::get("index.html") {
            return Some(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/html; charset=utf-8")
                    .body(Body::from(file.data.into_owned()))
                    .unwrap(),
            );
        }
    }

    None
}
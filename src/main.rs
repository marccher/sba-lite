use sbalite::auth::AuthConfig;
use sbalite::{config, state::MonitorState};
use std::net::SocketAddr;

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
    sbalite::state::spawn_poller(monitor_state.clone(), poll_interval_secs);
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

    let app = sbalite::build_app(monitor_state, auth_config);

    println!(
        "sbalite v{} — {instance_count} instances configured, polling every {poll_interval_secs}s (log_level={})",
        env!("CARGO_PKG_VERSION"),
        server_cfg.log_level.as_deref().unwrap_or("info"),
    );
    println!("Rust server listening on {bind_addr}");

    let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

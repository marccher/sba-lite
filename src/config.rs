use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct InstancesConfig {
    /// Settings for the Rust server itself (port, poll interval, TLS, ...).
    /// All fields are optional: if absent, hardcoded defaults are used (or
    /// the corresponding environment variable, which still takes
    /// precedence).
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(rename = "instance", default)]
    pub instances: Vec<InstanceConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerConfig {
    /// Port the Rust server listens on (on all interfaces), e.g. 9001.
    pub port: Option<u16>,
    /// Global polling interval in seconds (defaults to 10 if absent).
    pub poll_interval_secs: Option<u64>,
    /// WARNING: if true, disables TLS certificate verification for all
    /// calls to remote actuators. For local development with self-signed
    /// certificates only — do not use in production.
    pub insecure_tls: Option<bool>,
    /// Log verbosity for calls to remote servers, useful for debug/trace.
    /// Values: "info" (default, errors and main events only), "debug"
    /// (every call made and its outcome), "trace" (like debug, plus the
    /// raw response body).
    pub log_level: Option<String>,
    /// Maximum number of events kept in the journal (Journal view of the
    /// UI). Beyond this threshold, the oldest events are discarded
    /// (circular buffer) — necessary because the process may run for
    /// weeks/months. Default: 5000.
    pub journal_max_events: Option<usize>,
    /// Optional credentials to protect access to the UI and API with HTTP
    /// Basic Auth (native browser login prompt). If BOTH fields are
    /// present, authentication is enabled; if either is missing, the
    /// server remains unprotected (default behavior).
    pub auth_username: Option<String>,
    pub auth_password: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstanceConfig {
    /// Instance name: shown in the UI and also used as the unique
    /// identifier (in /instances/{name}/... calls). Must be unique and
    /// stable — don't change it once chosen, or the UI will lose track of
    /// any in-flight detail calls.
    pub name: String,
    /// Base URL of the remote actuator, e.g.
    /// "https://host/context/actuator" (no trailing slash).
    pub actuator_base_url: String,
    /// Optional JWT/Bearer token: if present, sent as an "Authorization:
    /// Bearer <token>" header on every call to this instance's actuator
    /// (both from the poller and the on-demand proxy). Leave this field
    /// absent for instances that don't require authentication — an
    /// expired/invalid token causes a 401 even if the instance is UP.
    pub bearer_token: Option<String>,
}

/// Loads the configuration from a file. If the file doesn't exist or has
/// no [[instance]] entries, still returns a valid config with zero
/// instances (no crash: the server starts anyway, it just monitors
/// nothing until you populate the file).
pub fn load(path: &str) -> anyhow::Result<InstancesConfig> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("could not read {path}: {e}"))?;
    let cfg: InstancesConfig =
        toml::from_str(&raw).map_err(|e| anyhow::anyhow!("parse error in {path}: {e}"))?;
    Ok(cfg)
}
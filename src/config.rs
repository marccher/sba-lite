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
    /// Path(s) to PEM-encoded CA certificate(s) (e.g. one or more
    /// internal/corporate root CAs) to trust in ADDITION to the OS's
    /// system trust store, without disabling verification and without
    /// touching the system's certificate configuration. Use this instead
    /// of insecure_tls when your targets use certificates issued by
    /// internal PKIs the OS doesn't already trust. Accepts either a
    /// single path (`ca_cert_path = "/path/to/ca.pem"`) or a list
    /// (`ca_cert_path = ["/path/a.pem", "/path/b.pem"]`) if you have
    /// certificates from multiple different internal CAs.
    pub ca_cert_path: Option<OneOrMany>,
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

/// Accepts either a single TOML string or an array of strings for the
/// same field, so `ca_cert_path` can be written either way depending on
/// how many CA certificates the user needs to trust.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_cert_path_accepts_a_single_string() {
        let toml_str = r#"
            [server]
            ca_cert_path = "/etc/sbalite/ca.pem"
        "#;
        let cfg: InstancesConfig = toml::from_str(toml_str).unwrap();
        let paths = cfg.server.ca_cert_path.unwrap().into_vec();
        assert_eq!(paths, vec!["/etc/sbalite/ca.pem".to_string()]);
    }

    #[test]
    fn ca_cert_path_accepts_an_array() {
        let toml_str = r#"
            [server]
            ca_cert_path = ["/etc/sbalite/ca-a.pem", "/etc/sbalite/ca-b.pem"]
        "#;
        let cfg: InstancesConfig = toml::from_str(toml_str).unwrap();
        let paths = cfg.server.ca_cert_path.unwrap().into_vec();
        assert_eq!(
            paths,
            vec![
                "/etc/sbalite/ca-a.pem".to_string(),
                "/etc/sbalite/ca-b.pem".to_string()
            ]
        );
    }

    #[test]
    fn server_section_is_fully_optional() {
        // A config with only instances and no [server] section at all
        // must still parse, falling back to every ServerConfig field
        // being None (defaults applied later in main.rs).
        let toml_str = r#"
            [[instance]]
            name = "svc"
            actuator_base_url = "https://host/actuator"
        "#;
        let cfg: InstancesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.server.port, None);
        assert_eq!(cfg.server.insecure_tls, None);
        assert_eq!(cfg.instances.len(), 1);
        assert_eq!(cfg.instances[0].name, "svc");
        assert_eq!(cfg.instances[0].bearer_token, None);
    }

    #[test]
    fn instance_bearer_token_is_optional() {
        let toml_str = r#"
            [[instance]]
            name = "secured"
            actuator_base_url = "https://host/actuator"
            bearer_token = "abc123"
        "#;
        let cfg: InstancesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.instances[0].bearer_token.as_deref(), Some("abc123"));
    }

    #[test]
    fn empty_file_yields_zero_instances_not_an_error() {
        let cfg: InstancesConfig = toml::from_str("").unwrap();
        assert!(cfg.instances.is_empty());
    }

    #[test]
    fn load_reports_a_clear_error_for_a_missing_file() {
        let result = load("/this/path/definitely/does/not/exist.toml");
        assert!(result.is_err());
    }
}
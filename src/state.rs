use crate::config::{InstanceConfig, InstancesConfig};
use crate::model::{
    ApplicationGroup, EndpointView, InstanceView, JournalEvent, JournalEventKind, RegistrationView,
    StatusInfoView,
};
use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

const DEFAULT_JOURNAL_MAX_EVENTS: usize = 5000;

#[derive(Clone)]
pub struct MonitorState {
    pub instances: Arc<RwLock<HashMap<String, InstanceView>>>,
    pub configs: Arc<HashMap<String, InstanceConfig>>,
    pub http_client: reqwest::Client,
    /// Broadcast channel: every poll cycle publishes here the updated
    /// /applications JSON snapshot, consumed by active SSE streams.
    pub events_tx: tokio::sync::broadcast::Sender<String>,
    /// Broadcast channel dedicated to the journal: every new JournalEvent
    /// is published here (serialized individually), consumed by the
    /// Journal view's SSE streams.
    pub journal_events_tx: tokio::sync::broadcast::Sender<String>,
    /// Journal (Journal view of the UI): circular buffer, one event for
    /// every REGISTERED / STATUS_CHANGED / ENDPOINTS_DETECTED /
    /// INFO_CHANGED detected. Once it exceeds journal_max_events, the
    /// oldest events are automatically discarded — necessary so memory
    /// doesn't grow forever on a long-running process.
    pub journal: Arc<RwLock<VecDeque<JournalEvent>>>,
    journal_max_events: usize,
    /// Per-instance version counter, shared across all journal event
    /// types (it does not restart from zero for each type).
    journal_versions: Arc<RwLock<HashMap<String, u64>>>,
}

impl MonitorState {
    pub fn new(cfg: InstancesConfig, http_client: reqwest::Client) -> Self {
        let journal_max_events = cfg
            .server
            .journal_max_events
            .unwrap_or(DEFAULT_JOURNAL_MAX_EVENTS);

        let configs: HashMap<String, InstanceConfig> = cfg
            .instances
            .into_iter()
            .map(|i| (i.name.clone(), i))
            .collect();

        let (events_tx, _) = tokio::sync::broadcast::channel(32);
        let (journal_events_tx, _) = tokio::sync::broadcast::channel(64);

        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            configs: Arc::new(configs),
            http_client,
            events_tx,
            journal_events_tx,
            journal: Arc::new(RwLock::new(VecDeque::new())),
            journal_max_events,
            journal_versions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.events_tx.subscribe()
    }

    pub fn subscribe_journal(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.journal_events_tx.subscribe()
    }

    /// Records an event in the journal, assigning the next version number
    /// for that specific instance (counter shared across all event
    /// types).
    async fn record_journal_event(&self, instance: String, kind: JournalEventKind) {
        let version = {
            let mut versions = self.journal_versions.write().await;
            let v = versions.entry(instance.clone()).or_insert(0);
            let assigned = *v;
            *v += 1;
            assigned
        };

        let event = JournalEvent {
            instance,
            version,
            timestamp: now_iso(),
            kind,
        };

        if let Ok(json) = serde_json::to_string(&event) {
            let _ = self.journal_events_tx.send(json); // no active receiver = fine
        }

        let mut journal = self.journal.write().await;
        journal.push_back(event);
        while journal.len() > self.journal_max_events {
            journal.pop_front();
        }
    }

    /// Looks up base_url + optional bearer token for a given id, for the
    /// on-demand actuator proxy (loggers, mappings, beans, thread dump,
    /// etc.).
    pub fn actuator_target(&self, instance_id: &str) -> Option<(String, Option<String>)> {
        self.configs
            .get(instance_id)
            .map(|c| (c.actuator_base_url.clone(), c.bearer_token.clone()))
    }
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Extracts only "scheme://host[:port]" from a full URL, regardless of how
/// many path segments follow (e.g. "/context/actuator").
fn origin_of(url: &str) -> String {
    match url.splitn(2, "://").collect::<Vec<_>>().as_slice() {
        [scheme, rest] => {
            let host_part = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{host_part}")
        }
        _ => url.to_string(),
    }
}

/// Applies the "Authorization: Bearer <token>" header to the request, if a
/// token is configured for this instance.
fn with_auth(builder: reqwest::RequestBuilder, token: Option<&str>) -> reqwest::RequestBuilder {
    match token {
        Some(t) => builder.bearer_auth(t),
        None => builder,
    }
}

/// Queries the actuator root (HAL discovery: {"_links": {...}}) to
/// dynamically discover which endpoints are exposed, instead of
/// hardcoding them.
async fn discover_endpoints(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
) -> Vec<EndpointView> {
    tracing::debug!(target: "sbalite::poller", "GET {base_url} (discovery)");

    let resp = match with_auth(client.get(base_url), token)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) => {
            tracing::debug!(target: "sbalite::poller", "  -> {} {base_url}", r.status());
            r
        }
        Err(e) => {
            tracing::debug!(target: "sbalite::poller", "  -> error {base_url}: {e}");
            return Vec::new();
        }
    };

    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(target: "sbalite::poller", "  -> non-JSON body {base_url}: {e}");
            return Vec::new();
        }
    };
    tracing::trace!(target: "sbalite::poller", "  body: {body}");

    let links = match body.get("_links").and_then(|v| v.as_object()) {
        Some(m) => m,
        None => return Vec::new(),
    };

    links
        .iter()
        .filter(|(k, _)| k.as_str() != "self")
        .filter_map(|(k, v)| {
            let href = v.get("href")?.as_str()?.to_string();
            Some(EndpointView {
                id: k.clone(),
                url: href,
            })
        })
        .collect()
}

/// Performs a single health-check call and returns the StatusInfoView in
/// the same format observed in the real Java backend.
async fn check_health(
    client: &reqwest::Client,
    _endpoints: &[EndpointView],
    base_url: &str,
    token: Option<&str>,
) -> StatusInfoView {
    // We do NOT use the "absolute" URL discovered via _links: behind a
    // reverse proxy that terminates TLS without correctly propagating
    // X-Forwarded-Proto, Spring can report a scheme (http instead of
    // https) different from the one we actually configured, causing calls
    // to an endpoint that doesn't respond as expected (observed in
    // practice: a 503 from a route that doesn't listen in plaintext). The
    // path itself is standard and predictable, so we always build it from
    // our own configured base_url, whose correct scheme we know for
    // certain.
    let health_url = format!("{base_url}/health");

    tracing::debug!(target: "sbalite::poller", "GET {health_url} (health)");

    let result = with_auth(client.get(&health_url), token)
        .timeout(Duration::from_secs(9))
        .send()
        .await;

    match result {
        Ok(resp) => {
            let status_code = resp.status();
            tracing::debug!(target: "sbalite::poller", "  -> {status_code} {health_url}");
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            tracing::trace!(target: "sbalite::poller", "  body: {body}");

            // Spring responds with HTTP 503 (not 200) when the aggregate
            // health is DOWN, but the body is still a complete and valid
            // health JSON — just as it would be with a 200. So we do NOT
            // use the HTTP status to decide whether the body is usable: we
            // always try to read a "status" string field from it, and
            // treat the body as a valid health payload if we find it,
            // regardless of the HTTP code. The generic placeholder
            // (path/error/status/timestamp) is reserved for cases where
            // the body is NOT a real health payload (e.g. a generic
            // non-Spring HTML/JSON error page, a 404 on the wrong path,
            // etc.).
            let parsed_status = body.get("status").and_then(|s| s.as_str());

            if let Some(status) = parsed_status {
                let status = status.to_string();

                // "details" = the body minus "status", with a twist: if
                // Spring uses "components" to nest the sub-indicators (db,
                // diskSpace, ping, ssl...), we "hoist" it to become
                // directly the content of "details" instead of leaving it
                // nested one level deeper — this is the shape the Vue UI
                // expects in order to render each sub-component as a
                // separate health node (colored badge) instead of raw
                // text.
                let details = match body {
                    Value::Object(mut map) => {
                        map.remove("status");
                        match map.remove("components") {
                            Some(Value::Object(mut components)) => {
                                // any remaining fields (e.g. "groups") stay
                                // alongside, we don't lose them
                                for (k, v) in map {
                                    components.insert(k, v);
                                }
                                Value::Object(components)
                            }
                            Some(other) => {
                                map.insert("components".into(), other);
                                Value::Object(map)
                            }
                            None => Value::Object(map),
                        }
                    }
                    other => other,
                };

                StatusInfoView {
                    status,
                    details,
                    out_of_service: false,
                    restricted: false,
                }
            } else {
                let mut details = Map::new();
                details.insert("path".into(), Value::String(health_url.clone()));
                details.insert(
                    "error".into(),
                    Value::String(
                        status_code
                            .canonical_reason()
                            .unwrap_or("Error")
                            .to_string(),
                    ),
                );
                details.insert("status".into(), Value::Number(status_code.as_u16().into()));
                details.insert("timestamp".into(), Value::String(now_iso()));

                StatusInfoView {
                    status: "DOWN".to_string(),
                    details: Value::Object(details),
                    out_of_service: false,
                    restricted: false,
                }
            }
        }
        Err(e) => {
            tracing::debug!(target: "sbalite::poller", "  -> error {health_url}: {e}");
            let mut details = Map::new();
            let exception_name = if e.is_timeout() {
                "java.util.concurrent.TimeoutException"
            } else if e.is_connect() {
                "java.net.ConnectException"
            } else {
                "java.io.IOException"
            };
            details.insert("exception".into(), Value::String(exception_name.into()));
            details.insert("message".into(), Value::String(e.to_string()));

            StatusInfoView {
                status: "OFFLINE".to_string(),
                details: Value::Object(details),
                out_of_service: false,
                restricted: false,
            }
        }
    }
}

/// Groups instances by "name", like the Java backend does — used both by
/// the /applications REST handler and by the poller for SSE broadcasts.
pub async fn build_application_groups(state: &MonitorState) -> Vec<ApplicationGroup> {
    let instances = state.instances.read().await;

    let mut groups: HashMap<String, Vec<InstanceView>> = HashMap::new();
    for view in instances.values() {
        groups
            .entry(view.registration.name.clone())
            .or_default()
            .push(view.clone());
    }

    groups
        .into_iter()
        .map(|(name, mut group_instances)| {
            group_instances.sort_by(|a, b| a.id.cmp(&b.id));

            let status = if group_instances.iter().any(|i| i.status_info.status == "UP") {
                "UP"
            } else if group_instances
                .iter()
                .all(|i| i.status_info.status == "OFFLINE")
            {
                "OFFLINE"
            } else {
                "DOWN"
            }
                .to_string();

            let status_timestamp = group_instances
                .iter()
                .map(|i| i.status_timestamp.clone())
                .max()
                .unwrap_or_default();

            let build_version = group_instances
                .first()
                .and_then(|i| i.build_version.clone());

            ApplicationGroup {
                build_version,
                instances: group_instances,
                name,
                status,
                status_timestamp,
            }
        })
        .collect()
}

/// Runs one full polling cycle over all configured instances and updates
/// the shared state.
async fn poll_once(state: &MonitorState) {
    let configs: Vec<InstanceConfig> = state.configs.values().cloned().collect();

    let futures = configs.into_iter().map(|cfg| {
        let client = state.http_client.clone();
        async move {
            let token = cfg.bearer_token.as_deref();
            let endpoints = discover_endpoints(&client, &cfg.actuator_base_url, token).await;
            let status_info = check_health(&client, &endpoints, &cfg.actuator_base_url, token).await;

            let has_info_endpoint = endpoints.iter().any(|e| e.id == "info");
            let info_value = if has_info_endpoint {
                // Same reason as the /health fix: we don't trust the
                // scheme reported in the discovered href, we build the URL
                // from our own configured base_url.
                let info_url = format!("{}/info", cfg.actuator_base_url);
                tracing::debug!(target: "sbalite::poller", "GET {info_url} (info)");
                match with_auth(client.get(&info_url), token)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await
                {
                    Ok(r) => {
                        tracing::debug!(target: "sbalite::poller", "  -> {} {info_url}", r.status());
                        Some(async move { r.json::<Value>().await.unwrap_or(Value::Null) })
                    }
                    Err(e) => {
                        tracing::debug!(target: "sbalite::poller", "  -> error {info_url}: {e}");
                        None
                    }
                }
            } else {
                None
            };
            let info_value = match info_value {
                Some(fut) => {
                    let v = fut.await;
                    tracing::trace!(target: "sbalite::poller", "  body: {v}");
                    v
                }
                None => Value::Object(Map::new()),
            };

            let build_version = info_value
                .get("build")
                .and_then(|b| b.get("version"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let context_path = cfg
                .actuator_base_url
                .splitn(4, '/')
                .nth(3)
                .map(|p| format!("/{p}"))
                .unwrap_or_default();

            let mut metadata = Map::new();
            metadata.insert(
                "management.context-path".into(),
                Value::String(context_path),
            );

            let view = InstanceView {
                build_version,
                endpoints,
                id: cfg.name.clone(),
                info: info_value,
                registered: true,
                registration: RegistrationView {
                    health_url: format!("{}/health", cfg.actuator_base_url),
                    management_url: cfg.actuator_base_url.clone(),
                    metadata: Value::Object(metadata),
                    name: cfg.name.clone(),
                    service_url: origin_of(&cfg.actuator_base_url),
                    source: "static".to_string(),
                },
                status_info,
                status_timestamp: now_iso(),
                tags: Value::Object(Map::new()),
                version: 1, // see README note: we don't keep a history of version changes here
            };

            (cfg.name.clone(), view)
        }
    });

    let results: Vec<(String, InstanceView)> = futures_util::future::join_all(futures).await;

    let mut guard = state.instances.write().await;

    // We detect REAL changes by comparing against the previous value,
    // before overwriting it. On the first poll (no previous value) the
    // instance is "new": we emit REGISTERED followed by the other events
    // with the initial state, replicating the sequence observed in the
    // real Java backend (REGISTERED for all instances, then
    // STATUS_CHANGED/ENDPOINTS_DETECTED/INFO_CHANGED as each async call
    // completes).
    let mut changed_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut journal_events: Vec<(String, JournalEventKind)> = Vec::new();

    for (id, view) in results {
        let previous = guard.get(&id).cloned();

        match &previous {
            None => {
                journal_events.push((
                    id.clone(),
                    JournalEventKind::Registered {
                        registration: view.registration.clone(),
                    },
                ));
                journal_events.push((
                    id.clone(),
                    JournalEventKind::StatusChanged {
                        status_info: view.status_info.clone(),
                    },
                ));
                journal_events.push((
                    id.clone(),
                    JournalEventKind::EndpointsDetected {
                        endpoints: view.endpoints.clone(),
                    },
                ));
                journal_events.push((
                    id.clone(),
                    JournalEventKind::InfoChanged {
                        info: view.info.clone(),
                    },
                ));
                // On the very first detection we don't consider the state
                // "changed" for SSE broadcast purposes: it's the initial
                // baseline, which already arrives via the XHR fetch — not
                // a push to notify as if it were a live event.
            }
            Some(prev) => {
                if prev.status_info.status != view.status_info.status {
                    changed_names.insert(view.registration.name.clone());
                    journal_events.push((
                        id.clone(),
                        JournalEventKind::StatusChanged {
                            status_info: view.status_info.clone(),
                        },
                    ));
                }
                if prev.endpoints != view.endpoints {
                    journal_events.push((
                        id.clone(),
                        JournalEventKind::EndpointsDetected {
                            endpoints: view.endpoints.clone(),
                        },
                    ));
                }
                if prev.info != view.info {
                    journal_events.push((
                        id.clone(),
                        JournalEventKind::InfoChanged {
                            info: view.info.clone(),
                        },
                    ));
                }
            }
        }

        // keep the previous version incremented, if already present
        let version = previous.as_ref().map(|v| v.version + 1).unwrap_or(1);
        let mut view = view;
        view.version = version;
        guard.insert(id, view);
    }
    drop(guard);

    for (instance, kind) in journal_events {
        state.record_journal_event(instance, kind).await;
    }

    if changed_names.is_empty() {
        return; // nothing relevant changed: let the keep-alive send the pings
    }

    let groups = build_application_groups(state).await;

    // Publish ONLY the applications whose state actually changed in this
    // cycle — the real backend doesn't send an event on every poll, only
    // on actual changes (confirmed by the capture: only ":ping" is seen on
    // cycles with no changes).
    for group in &groups {
        if !changed_names.contains(&group.name) {
            continue;
        }
        if let Ok(json) = serde_json::to_string(group) {
            let _ = state.events_tx.send(json); // no active receiver = fine
        }
    }
}

/// Starts the periodic background polling task.
pub fn spawn_poller(state: MonitorState, interval_secs: u64) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            ticker.tick().await;
            poll_once(&state).await;
        }
    });
}
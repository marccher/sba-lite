use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ApplicationGroup {
    #[serde(rename = "buildVersion")]
    pub build_version: Option<String>,
    pub instances: Vec<InstanceView>,
    pub name: String,
    pub status: String,
    #[serde(rename = "statusTimestamp")]
    pub status_timestamp: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstanceView {
    #[serde(rename = "buildVersion")]
    pub build_version: Option<String>,
    pub endpoints: Vec<EndpointView>,
    pub id: String,
    pub info: Value,
    pub registered: bool,
    pub registration: RegistrationView,
    #[serde(rename = "statusInfo")]
    pub status_info: StatusInfoView,
    #[serde(rename = "statusTimestamp")]
    pub status_timestamp: String,
    pub tags: Value,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EndpointView {
    pub id: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistrationView {
    #[serde(rename = "healthUrl")]
    pub health_url: String,
    #[serde(rename = "managementUrl")]
    pub management_url: String,
    pub metadata: Value,
    pub name: String,
    #[serde(rename = "serviceUrl")]
    pub service_url: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusInfoView {
    pub status: String,
    pub details: Value,
    #[serde(rename = "outOfService")]
    pub out_of_service: bool,
    pub restricted: bool,
}

/// A single journal event (Journal view of the UI). "version" is a
/// monotonic counter PER INSTANCE, shared across all event types (it does
/// not restart from zero for each type) — this replicates exactly the
/// behavior observed in the real Java backend.
#[derive(Debug, Clone, Serialize)]
pub struct JournalEvent {
    pub instance: String,
    pub version: u64,
    pub timestamp: String,
    #[serde(flatten)]
    pub kind: JournalEventKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum JournalEventKind {
    #[serde(rename = "REGISTERED")]
    Registered { registration: RegistrationView },
    #[serde(rename = "STATUS_CHANGED")]
    StatusChanged {
        #[serde(rename = "statusInfo")]
        status_info: StatusInfoView,
    },
    #[serde(rename = "ENDPOINTS_DETECTED")]
    EndpointsDetected { endpoints: Vec<EndpointView> },
    #[serde(rename = "INFO_CHANGED")]
    InfoChanged { info: Value },
}
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceEventType {
    DiscoveryChanged,
    DiscoveryUnchanged,
    DiscoveryTapSkipped,
    DiscoveryError,
    ExecutionCreated,
    ExecutionCompleted,
    ExecutionFailed,
    ExecutionCancelled,
    ExecutionNotSubmitted,
    ConfigActivated,
    AlertFired,
    AlertDeliveryFailed,
}

impl ProvenanceEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DiscoveryChanged => "discovery.changed",
            Self::DiscoveryUnchanged => "discovery.unchanged",
            Self::DiscoveryTapSkipped => "discovery.tap_skipped",
            Self::DiscoveryError => "discovery.error",
            Self::ExecutionCreated => "execution.created",
            Self::ExecutionCompleted => "execution.completed",
            Self::ExecutionFailed => "execution.failed",
            Self::ExecutionCancelled => "execution.cancelled",
            Self::ExecutionNotSubmitted => "execution.not_submitted",
            Self::ConfigActivated => "config.activated",
            Self::AlertFired => "alert.fired",
            Self::AlertDeliveryFailed => "alert.delivery_failed",
        }
    }
}

pub fn build_provenance_payload(fields: &[(&str, Value)]) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in fields {
        map.insert((*k).into(), v.clone());
    }
    Value::Object(map)
}

use crate::{DimClient, TranslatorClient};
use async_trait::async_trait;
use beampipe_domain::{Diagnostic, ExecutionStatus, Failure, FailureClass, RetryDisposition};
use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaliugeComponent {
    Translator,
    DataIslandManager,
    NodeManager,
}

impl DaliugeComponent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Translator => "daliuge_translator",
            Self::DataIslandManager => "daliuge_dim",
            Self::NodeManager => "daliuge_node_manager",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaliugeErrorKind {
    Connectivity,
    Timeout,
    HttpStatus,
    InvalidResponse,
    Compatibility,
    Conflict,
    NotFound,
}

#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[error("{component:?} {operation} failed: {message}")]
pub struct DaliugeClientError {
    pub component: DaliugeComponent,
    pub operation: String,
    pub endpoint: String,
    pub kind: DaliugeErrorKind,
    pub message: String,
    pub http_status: Option<u16>,
    pub retryable: bool,
    pub response_excerpt: Option<String>,
}

impl DaliugeClientError {
    pub fn request(
        component: DaliugeComponent,
        operation: impl Into<String>,
        endpoint: impl Into<String>,
        error: reqwest::Error,
    ) -> Self {
        let kind = if error.is_timeout() {
            DaliugeErrorKind::Timeout
        } else {
            DaliugeErrorKind::Connectivity
        };
        Self {
            component,
            operation: operation.into(),
            endpoint: endpoint.into(),
            kind,
            message: if error.is_timeout() {
                "request timed out".into()
            } else if error.is_connect() {
                "connection failed".into()
            } else {
                "request failed".into()
            },
            http_status: None,
            retryable: true,
            response_excerpt: None,
        }
    }

    pub fn invalid_response(
        component: DaliugeComponent,
        operation: impl Into<String>,
        endpoint: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            component,
            operation: operation.into(),
            endpoint: endpoint.into(),
            kind: DaliugeErrorKind::InvalidResponse,
            message: message.into(),
            http_status: None,
            retryable: false,
            response_excerpt: None,
        }
    }

    pub fn compatibility(
        component: DaliugeComponent,
        operation: impl Into<String>,
        endpoint: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            component,
            operation: operation.into(),
            endpoint: endpoint.into(),
            kind: DaliugeErrorKind::Compatibility,
            message: message.into(),
            http_status: None,
            retryable: false,
            response_excerpt: None,
        }
    }

    pub fn failure_class(&self) -> FailureClass {
        match self.kind {
            DaliugeErrorKind::Connectivity => FailureClass::Connectivity,
            DaliugeErrorKind::Timeout => FailureClass::Timeout,
            DaliugeErrorKind::Compatibility | DaliugeErrorKind::InvalidResponse => {
                FailureClass::Unsupported
            }
            DaliugeErrorKind::Conflict => FailureClass::Conflict,
            DaliugeErrorKind::NotFound => FailureClass::NotFound,
            DaliugeErrorKind::HttpStatus => FailureClass::DependencyUnavailable,
        }
    }

    pub fn as_failure(&self) -> Failure {
        Failure::new(
            format!("daliuge.{:?}", self.kind).to_ascii_lowercase(),
            self.component.as_str(),
            self.failure_class(),
            self.message.clone(),
            if self.retryable {
                RetryDisposition::Safe
            } else {
                RetryDisposition::AfterRemediation
            },
            if self.retryable {
                "Beampipe will retain current state and retry or reconcile the operation"
            } else {
                "Beampipe will retain current state until configuration is corrected"
            },
        )
        .with_operator_action(match self.kind {
            DaliugeErrorKind::Connectivity | DaliugeErrorKind::Timeout => {
                "verify the DALiuGE endpoint and network path with `beampipe daliuge inspect`"
            }
            DaliugeErrorKind::Compatibility => {
                "verify the deployment uses the supported DALiuGE translator and manager APIs"
            }
            DaliugeErrorKind::Conflict => {
                "inspect the existing DALiuGE session before retrying the operation"
            }
            _ => "inspect DALiuGE manager logs and the execution observation history",
        })
    }
}

pub async fn checked_json<T: DeserializeOwned>(
    response: reqwest::Response,
    component: DaliugeComponent,
    operation: &str,
    endpoint: &str,
) -> Result<T, DaliugeClientError> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| DaliugeClientError::request(component, operation, endpoint, error))?;
    if !status.is_success() {
        let kind = match status.as_u16() {
            404 => DaliugeErrorKind::NotFound,
            409 => DaliugeErrorKind::Conflict,
            _ => DaliugeErrorKind::HttpStatus,
        };
        let response_excerpt = bounded_excerpt(&bytes);
        return Err(DaliugeClientError {
            component,
            operation: operation.into(),
            endpoint: endpoint.into(),
            kind,
            message: format!("DALiuGE returned HTTP {}", status.as_u16()),
            http_status: Some(status.as_u16()),
            retryable: status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429,
            response_excerpt,
        });
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        let mut failure = DaliugeClientError::invalid_response(
            component,
            operation,
            endpoint,
            format!("invalid JSON response: {error}"),
        );
        failure.response_excerpt = bounded_excerpt(&bytes);
        failure
    })
}

pub async fn checked_empty(
    response: reqwest::Response,
    component: DaliugeComponent,
    operation: &str,
    endpoint: &str,
) -> Result<(), DaliugeClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let bytes = response.bytes().await.unwrap_or_default();
    let kind = match status.as_u16() {
        404 => DaliugeErrorKind::NotFound,
        409 => DaliugeErrorKind::Conflict,
        _ => DaliugeErrorKind::HttpStatus,
    };
    Err(DaliugeClientError {
        component,
        operation: operation.into(),
        endpoint: endpoint.into(),
        kind,
        message: format!("DALiuGE returned HTTP {}", status.as_u16()),
        http_status: Some(status.as_u16()),
        retryable: status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429,
        response_excerpt: bounded_excerpt(&bytes),
    })
}

fn bounded_excerpt(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    Some(text.into_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaliugeSessionState {
    Pristine,
    Building,
    Deploying,
    Running,
    Finished,
    Cancelled,
    Failed,
    Unknown,
}

impl DaliugeSessionState {
    pub fn from_raw(value: &Value) -> Self {
        if let Some(status) = value.as_i64() {
            return Self::from_number(status);
        }
        if let Some(status) = value.as_str() {
            return Self::from_name(status);
        }
        if let Some(object) = value.as_object() {
            if let Some(status) = object.get("status") {
                return Self::from_raw(status);
            }
            return aggregate_node_states(object.values().map(Self::from_raw));
        }
        Self::Unknown
    }

    fn from_number(value: i64) -> Self {
        match value {
            0 => Self::Pristine,
            1 => Self::Building,
            2 => Self::Deploying,
            3 => Self::Running,
            4 => Self::Finished,
            5 => Self::Cancelled,
            6 => Self::Failed,
            _ => Self::Unknown,
        }
    }

    fn from_name(value: &str) -> Self {
        match value.trim().to_ascii_uppercase().as_str() {
            "PRISTINE" => Self::Pristine,
            "BUILDING" => Self::Building,
            "DEPLOYING" => Self::Deploying,
            "RUNNING" => Self::Running,
            "FINISHED" | "COMPLETED" => Self::Finished,
            "CANCELLED" | "CANCELED" => Self::Cancelled,
            "FAILED" | "FAIL" | "ERROR" => Self::Failed,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pristine => "pristine",
            Self::Building => "building",
            Self::Deploying => "deploying",
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub fn execution_status(self) -> ExecutionStatus {
        match self {
            Self::Finished => ExecutionStatus::Completed,
            Self::Cancelled => ExecutionStatus::Cancelled,
            Self::Failed => ExecutionStatus::Failed,
            Self::Pristine | Self::Building | Self::Deploying | Self::Running | Self::Unknown => {
                ExecutionStatus::Running
            }
        }
    }
}

fn aggregate_node_states(states: impl Iterator<Item = DaliugeSessionState>) -> DaliugeSessionState {
    let states: Vec<_> = states.collect();
    if states.is_empty() || states.contains(&DaliugeSessionState::Unknown) {
        return DaliugeSessionState::Unknown;
    }
    if states.contains(&DaliugeSessionState::Failed) {
        return DaliugeSessionState::Failed;
    }
    if states
        .iter()
        .all(|state| *state == DaliugeSessionState::Finished)
    {
        return DaliugeSessionState::Finished;
    }
    if states
        .iter()
        .all(|state| *state == DaliugeSessionState::Cancelled)
    {
        return DaliugeSessionState::Cancelled;
    }
    for state in [
        DaliugeSessionState::Running,
        DaliugeSessionState::Deploying,
        DaliugeSessionState::Building,
        DaliugeSessionState::Pristine,
    ] {
        if states.contains(&state) {
            return state;
        }
    }
    DaliugeSessionState::Unknown
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaliugeSessionObservation {
    pub state: DaliugeSessionState,
    pub raw: Value,
    pub per_node: BTreeMap<String, DaliugeSessionState>,
    pub observed_at: DateTime<Utc>,
}

impl DaliugeSessionObservation {
    pub fn from_raw(raw: Value) -> Self {
        let per_node = raw
            .as_object()
            .filter(|object| !object.contains_key("status"))
            .map(|object| {
                object
                    .iter()
                    .map(|(node, value)| (node.clone(), DaliugeSessionState::from_raw(value)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            state: DaliugeSessionState::from_raw(&raw),
            raw,
            per_node,
            observed_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaliugeCapabilities {
    pub updated_translation_api: bool,
    pub session_api: bool,
    pub manager_topology: bool,
    pub session_logs: bool,
    pub submission_methods: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaliugeTranslatorInfo {
    pub endpoint: String,
    pub version: Option<String>,
    pub capabilities: DaliugeCapabilities,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaliugeSessionSummary {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub status: Value,
    #[serde(default)]
    pub size: Value,
}

impl DaliugeSessionSummary {
    pub fn state(&self) -> DaliugeSessionState {
        DaliugeSessionState::from_raw(&self.status)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaliugeManagerInfo {
    pub endpoint: String,
    pub version: Option<String>,
    pub hosts: Vec<String>,
    pub nodes: Vec<String>,
    pub sessions: Vec<DaliugeSessionSummary>,
    pub capabilities: DaliugeCapabilities,
    pub diagnostics: Vec<Diagnostic>,
}

#[async_trait]
pub trait DaliugeTranslator: TranslatorClient {
    async fn inspect(
        &self,
        manager_host: Option<&str>,
        manager_port: Option<i32>,
    ) -> Result<DaliugeTranslatorInfo, DaliugeClientError>;
}

#[async_trait]
pub trait DaliugeManager: DimClient {
    async fn inspect(&self) -> Result<DaliugeManagerInfo, DaliugeClientError>;
    async fn sessions(&self) -> Result<Vec<DaliugeSessionSummary>, DaliugeClientError>;
    async fn session_observation(
        &self,
        session_id: &str,
    ) -> Result<DaliugeSessionObservation, DaliugeClientError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_upstream_session_numbers_are_preserved() {
        let states = [
            DaliugeSessionState::Pristine,
            DaliugeSessionState::Building,
            DaliugeSessionState::Deploying,
            DaliugeSessionState::Running,
            DaliugeSessionState::Finished,
            DaliugeSessionState::Cancelled,
            DaliugeSessionState::Failed,
        ];
        for (number, expected) in states.into_iter().enumerate() {
            assert_eq!(DaliugeSessionState::from_raw(&json!(number)), expected);
        }
    }

    #[test]
    fn composite_state_retains_per_node_values() {
        let observation = DaliugeSessionObservation::from_raw(json!({
            "nm-a:8000": 4,
            "nm-b:8000": 6,
        }));
        assert_eq!(observation.state, DaliugeSessionState::Failed);
        assert_eq!(
            observation.per_node["nm-a:8000"],
            DaliugeSessionState::Finished
        );
    }
}

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct Diagnostic {
    pub path: String,
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn error(
        path: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            severity: DiagnosticSeverity::Error,
            code: code.into(),
            message: message.into(),
            hint: None,
        }
    }

    pub fn warning(
        path: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            severity: DiagnosticSeverity::Warning,
            code: code.into(),
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Validation,
    Configuration,
    Authentication,
    Authorization,
    NotFound,
    Conflict,
    DependencyUnavailable,
    Connectivity,
    Timeout,
    RateLimited,
    Unsupported,
    Cancelled,
    InconsistentState,
    Internal,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Configuration => "configuration",
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::DependencyUnavailable => "dependency_unavailable",
            Self::Connectivity => "connectivity",
            Self::Timeout => "timeout",
            Self::RateLimited => "rate_limited",
            Self::Unsupported => "unsupported",
            Self::Cancelled => "cancelled",
            Self::InconsistentState => "inconsistent_state",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetryDisposition {
    Automatic,
    Safe,
    AfterRemediation,
    Unsafe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct Failure {
    pub code: String,
    pub component: String,
    pub class: FailureClass,
    pub message: String,
    pub retry: RetryDisposition,
    pub system_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Failure {
    pub fn new(
        code: impl Into<String>,
        component: impl Into<String>,
        class: FailureClass,
        message: impl Into<String>,
        retry: RetryDisposition,
        system_action: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            component: component.into(),
            class,
            message: message.into(),
            retry,
            system_action: system_action.into(),
            operator_action: None,
            log_reference: None,
            diagnostics: Vec::new(),
        }
    }

    pub fn with_operator_action(mut self, action: impl Into<String>) -> Self {
        self.operator_action = Some(action.into());
        self
    }

    pub fn with_log_reference(mut self, reference: impl Into<String>) -> Self {
        self.log_reference = Some(reference.into());
        self
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        self.diagnostics = diagnostics;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_failure_context_is_omitted() {
        let failure = Failure::new(
            "database_unavailable",
            "postgres",
            FailureClass::DependencyUnavailable,
            "database unavailable",
            RetryDisposition::Safe,
            "the request was not completed",
        );
        let value = serde_json::to_value(failure).expect("serialize failure");
        assert!(value.get("operator_action").is_none());
        assert!(value.get("log_reference").is_none());
        assert!(value.get("diagnostics").is_none());
    }
}

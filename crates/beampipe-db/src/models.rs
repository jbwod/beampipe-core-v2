use beampipe_domain::{ExecutionPhase, ExecutionStatus};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UserRow {
    pub id: i32,
    pub uuid: Uuid,
    pub name: String,
    pub username: String,
    pub email: String,
    pub hashed_password: String,
    pub profile_image_url: String,
    pub is_deleted: bool,
    pub is_superuser: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct SourceRegistryRow {
    pub uuid: Uuid,
    pub project_module: String,
    pub source_identifier: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_attempted_at: Option<DateTime<Utc>>,
    pub stale_after_hours: Option<i32>,
    pub discovery_signature: Option<String>,
    pub last_executed_discovery_signature: Option<String>,
    pub discovery_claim_token: Option<String>,
    pub discovery_claim_expires_at: Option<DateTime<Utc>>,
    pub workflow_run_pending: bool,
    pub workflow_run_pending_at: Option<DateTime<Utc>>,
    pub workflow_claim_token: Option<String>,
    pub workflow_claimed_at: Option<DateTime<Utc>>,
    pub workflow_claim_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ArchiveMetadataRow {
    pub uuid: Uuid,
    pub project_module: String,
    pub source_identifier: String,
    pub sbid: String,
    pub metadata_json: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ExecutionRow {
    pub uuid: Uuid,
    pub project_module: String,
    pub sources: Value,
    pub archive_name: String,
    pub deployment_profile_id: Option<Uuid>,
    pub project_config_id: Option<Uuid>,
    pub workflow_manifest: Option<Value>,
    pub execution_phase: Option<String>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub last_error: Option<String>,
    pub created_by_id: Option<i32>,
    pub status: String,
    pub retry_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl ExecutionRow {
    pub fn status_enum(&self) -> Option<ExecutionStatus> {
        match self.status.as_str() {
            "pending" => Some(ExecutionStatus::Pending),
            "running" => Some(ExecutionStatus::Running),
            "awaiting_scheduler" => Some(ExecutionStatus::AwaitingScheduler),
            "not_submitted" => Some(ExecutionStatus::NotSubmitted),
            "completed" => Some(ExecutionStatus::Completed),
            "failed" => Some(ExecutionStatus::Failed),
            "retrying" => Some(ExecutionStatus::Retrying),
            "cancelled" => Some(ExecutionStatus::Cancelled),
            _ => None,
        }
    }

    pub fn phase_enum(&self) -> Option<ExecutionPhase> {
        match self.execution_phase.as_deref() {
            Some("stage_and_manifest") => Some(ExecutionPhase::StageAndManifest),
            Some("submit") => Some(ExecutionPhase::Submit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DeploymentProfileRow {
    pub uuid: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub project_module: Option<String>,
    pub is_default: bool,
    pub translation: Value,
    pub deployment: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProjectConfigRow {
    pub uuid: Uuid,
    pub project_id: String,
    pub version: i32,
    pub spec: Value,
    pub spec_sha256: String,
    pub active: bool,
    pub uploaded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct JobRow {
    pub uuid: Uuid,
    pub kind: String,
    pub payload: Value,
    pub status: String,
    pub execution_id: Option<Uuid>,
    pub phase: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub next_run_at: DateTime<Utc>,
    pub locked_until: Option<DateTime<Utc>>,
    pub idempotency_key: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct ProvenanceEventRow {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub project_module: String,
    pub source_identifier: Option<String>,
    pub execution_id: Option<Uuid>,
    pub actor: Option<String>,
    pub correlation_id: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct NotificationChannelRow {
    pub uuid: Uuid,
    pub name: String,
    pub kind: String,
    pub config: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct AlertRuleRow {
    pub uuid: Uuid,
    pub name: String,
    pub project_module: Option<String>,
    pub enabled: bool,
    pub severity: String,
    pub trigger_kind: String,
    pub trigger_config: Value,
    pub channel_ids: Vec<Uuid>,
    pub cooldown_minutes: i32,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct AlertDeliveryRow {
    pub uuid: Uuid,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub status: String,
    pub payload: Value,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

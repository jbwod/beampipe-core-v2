use beampipe_domain::{
    ControlPhase, DaliugeState, ExecutionAxes, ExecutionPhase, ExecutionStatus, FailureClass,
    OutputState, SchedulerState, SubmissionState, TerminalOutcome,
};
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
    pub deployment_profile_revision: Option<i32>,
    pub deployment_profile_snapshot: Option<Value>,
    pub project_config_id: Option<Uuid>,
    pub discovery_signature: Option<String>,
    pub workflow_manifest: Option<Value>,
    pub manifest_sha256: Option<String>,
    pub source_graph_sha256: Option<String>,
    pub patched_graph_sha256: Option<String>,
    pub physical_graph_sha256: Option<String>,
    pub execution_phase: Option<String>,
    pub control_phase: Option<String>,
    pub submission_state: Option<String>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub scheduler_state: Option<String>,
    pub scheduler_raw_state: Option<String>,
    pub scheduler_reason: Option<String>,
    pub daliuge_session_id: Option<String>,
    pub daliuge_manager_url: Option<String>,
    pub daliuge_state: Option<String>,
    pub daliuge_raw_status: Option<Value>,
    pub output_state: Option<String>,
    pub output_verification_required: bool,
    pub remote_session_dir: Option<String>,
    pub terminal_outcome: Option<String>,
    pub failure_class: Option<String>,
    pub phase_timestamps: Value,
    pub last_reconciled_at: Option<DateTime<Utc>>,
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

    pub fn axes(&self) -> ExecutionAxes {
        ExecutionAxes {
            control_phase: self
                .control_phase
                .as_deref()
                .and_then(ControlPhase::parse)
                .unwrap_or_default(),
            submission: self
                .submission_state
                .as_deref()
                .and_then(SubmissionState::parse)
                .unwrap_or_default(),
            scheduler: self
                .scheduler_state
                .as_deref()
                .and_then(SchedulerState::parse)
                .unwrap_or_default(),
            daliuge: self
                .daliuge_state
                .as_deref()
                .and_then(DaliugeState::parse)
                .unwrap_or_default(),
            outputs: self
                .output_state
                .as_deref()
                .and_then(OutputState::parse)
                .unwrap_or_default(),
            output_verification_required: self.output_verification_required,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionStatePatch {
    pub control_phase: Option<ControlPhase>,
    pub submission_state: Option<SubmissionState>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub scheduler_state: Option<SchedulerState>,
    pub scheduler_raw_state: Option<String>,
    pub scheduler_reason: Option<String>,
    pub daliuge_session_id: Option<String>,
    pub daliuge_manager_url: Option<String>,
    pub daliuge_state: Option<DaliugeState>,
    pub daliuge_raw_status: Option<Value>,
    pub remote_session_dir: Option<String>,
    pub output_state: Option<OutputState>,
    pub terminal_outcome: Option<TerminalOutcome>,
    pub failure_class: Option<FailureClass>,
    pub last_error: Option<String>,
    pub last_reconciled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionProvenancePatch {
    pub deployment_profile_revision: Option<i32>,
    pub deployment_profile_snapshot: Option<Value>,
    pub discovery_signature: Option<String>,
    pub manifest_sha256: Option<String>,
    pub source_graph_sha256: Option<String>,
    pub patched_graph_sha256: Option<String>,
    pub physical_graph_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DeploymentProfileRow {
    pub uuid: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub project_module: Option<String>,
    pub is_default: bool,
    pub max_concurrent_executions: Option<i32>,
    pub translation: Value,
    pub deployment: Value,
    pub revision: i32,
    pub spec_sha256: Option<String>,
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
    pub lease_owner: Option<Uuid>,
    pub lease_token: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub pool: String,
    pub required_capability: Option<String>,
    pub required_labels: Value,
    pub priority: i32,
    pub idempotency_key: Option<String>,
    pub last_error: Option<String>,
    pub failure_class: Option<String>,
    pub dead_lettered_at: Option<DateTime<Utc>>,
    pub dead_letter_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct WorkerInstanceRow {
    pub uuid: Uuid,
    pub instance_name: String,
    pub host_name: String,
    pub process_id: Option<i32>,
    pub role: String,
    pub pool: String,
    pub capabilities: Vec<String>,
    pub labels: Value,
    pub version: String,
    pub concurrency_limit: i32,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub last_heartbeat_at: DateTime<Utc>,
    pub draining_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRegistration {
    pub uuid: Uuid,
    pub instance_name: String,
    pub host_name: String,
    pub process_id: Option<i32>,
    pub role: String,
    pub pool: String,
    pub capabilities: Vec<String>,
    pub labels: Value,
    pub version: String,
    pub concurrency_limit: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct WorkerPoolSummary {
    pub pool: String,
    pub active_workers: i64,
    pub draining_workers: i64,
    pub unhealthy_workers: i64,
    pub concurrency_limit: i64,
    pub active_leases: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct OperatorOverviewCounts {
    pub registered_sources: i64,
    pub pending_admissions: i64,
    pub running_executions: i64,
    pub failed_executions: i64,
    pub queue_depth: i64,
    pub active_workers: i64,
    pub stale_workers: i64,
    pub recent_alerts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct JobClaimHistoryRow {
    pub uuid: Uuid,
    pub job_id: Uuid,
    pub worker_id: Option<Uuid>,
    pub lease_token: Option<Uuid>,
    pub event: String,
    pub occurred_at: DateTime<Utc>,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct ExecutionObservationRow {
    pub uuid: Uuid,
    pub execution_id: Uuid,
    pub kind: String,
    pub normalized_state: String,
    pub raw_state: Option<String>,
    pub reason: Option<String>,
    pub payload: Value,
    pub source_version: Option<String>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionObservationInput {
    pub kind: String,
    pub normalized_state: String,
    pub raw_state: Option<String>,
    pub reason: Option<String>,
    #[serde(default)]
    pub payload: Value,
    pub source_version: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema)]
pub struct ExecutionArtifactRow {
    pub uuid: Uuid,
    pub execution_id: Uuid,
    pub kind: String,
    pub storage_kind: String,
    pub uri: Option<String>,
    pub inline_json: Option<Value>,
    pub media_type: String,
    pub sha256: String,
    pub size_bytes: Option<i64>,
    pub producer_phase: String,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionArtifactInput {
    pub kind: String,
    pub storage_kind: String,
    pub uri: Option<String>,
    pub inline_json: Option<Value>,
    pub media_type: String,
    pub sha256: String,
    pub size_bytes: Option<i64>,
    pub producer_phase: String,
    #[serde(default)]
    pub metadata: Value,
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

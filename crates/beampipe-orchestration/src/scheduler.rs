use async_trait::async_trait;
use beampipe_domain::{Failure, FailureClass, RetryDisposition, SchedulerState};
use beampipe_profiles::SlurmRemoteDeploymentConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

use crate::clients::SshSlurmClient;
use crate::slurm_deploy::{resolve_remote_user, submit_slurm_session, SlurmSubmitParams};
use crate::slurm_ssh::{
    query_slurm_states_batch, validate_slurm_job_id, SlurmSshSession, SlurmTarget,
};
use crate::{OrchestrationError, SlurmJobPollResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerKind {
    SlurmRemote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerErrorKind {
    Configuration,
    Connectivity,
    Authentication,
    HostVerification,
    Timeout,
    Command,
    InvalidResponse,
    NotFound,
    SubmissionUncertain,
    Internal,
}

#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[error("{scheduler:?} {operation} failed: {message}")]
pub struct SchedulerAdapterError {
    pub scheduler: SchedulerKind,
    pub operation: String,
    pub target: String,
    pub kind: SchedulerErrorKind,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SchedulerAdapterError {
    fn configuration(operation: &str, message: impl Into<String>) -> Self {
        Self {
            scheduler: SchedulerKind::SlurmRemote,
            operation: operation.into(),
            target: String::new(),
            kind: SchedulerErrorKind::Configuration,
            message: message.into(),
            retryable: false,
            detail: None,
        }
    }

    fn backend(operation: &str, target: &str, error: OrchestrationError) -> Self {
        let detail = bounded_detail(&error.to_string());
        let lower = detail.to_ascii_lowercase();
        let (kind, retryable, message) =
            if matches!(&error, OrchestrationError::SubmissionUncertain(_)) {
                (
                    SchedulerErrorKind::SubmissionUncertain,
                    false,
                    "scheduler submission outcome is uncertain",
                )
            } else if lower.contains("host key") || lower.contains("known_hosts") {
                (
                    SchedulerErrorKind::HostVerification,
                    false,
                    "SSH host verification failed",
                )
            } else if lower.contains("auth") || lower.contains("credential") {
                (
                    SchedulerErrorKind::Authentication,
                    false,
                    "scheduler authentication failed",
                )
            } else if lower.contains("timed out") || lower.contains("timeout") {
                (
                    SchedulerErrorKind::Timeout,
                    true,
                    "scheduler operation timed out",
                )
            } else if lower.contains("connect") || lower.contains("unreachable") {
                (
                    SchedulerErrorKind::Connectivity,
                    true,
                    "scheduler connection failed",
                )
            } else if lower.contains("command failed") || lower.contains("ssh exec") {
                (
                    SchedulerErrorKind::Command,
                    false,
                    "scheduler command failed",
                )
            } else {
                (
                    SchedulerErrorKind::Internal,
                    false,
                    "scheduler operation failed",
                )
            };
        Self {
            scheduler: SchedulerKind::SlurmRemote,
            operation: operation.into(),
            target: target.into(),
            kind,
            message: message.into(),
            retryable,
            detail: Some(detail),
        }
    }

    fn invalid_response(operation: &str, target: &str, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            scheduler: SchedulerKind::SlurmRemote,
            operation: operation.into(),
            target: target.into(),
            kind: SchedulerErrorKind::InvalidResponse,
            message: "scheduler returned an invalid response".into(),
            retryable: false,
            detail: Some(bounded_detail(&detail)),
        }
    }

    pub fn failure_class(&self) -> FailureClass {
        match self.kind {
            SchedulerErrorKind::Configuration => FailureClass::Configuration,
            SchedulerErrorKind::Connectivity => FailureClass::Connectivity,
            SchedulerErrorKind::Authentication => FailureClass::Authentication,
            SchedulerErrorKind::HostVerification => FailureClass::Authorization,
            SchedulerErrorKind::Timeout => FailureClass::Timeout,
            SchedulerErrorKind::NotFound => FailureClass::NotFound,
            SchedulerErrorKind::SubmissionUncertain => FailureClass::InconsistentState,
            SchedulerErrorKind::Command
            | SchedulerErrorKind::InvalidResponse
            | SchedulerErrorKind::Internal => FailureClass::DependencyUnavailable,
        }
    }

    pub fn as_failure(&self) -> Failure {
        Failure::new(
            format!("scheduler.{:?}", self.kind).to_ascii_lowercase(),
            "slurm",
            self.failure_class(),
            self.message.clone(),
            if self.retryable {
                RetryDisposition::Safe
            } else {
                RetryDisposition::AfterRemediation
            },
            if self.retryable {
                "Beampipe will retry or reconcile the scheduler operation"
            } else {
                "Beampipe will leave the execution unchanged"
            },
        )
        .with_operator_action(match self.kind {
            SchedulerErrorKind::HostVerification => {
                "verify the login host key in the configured known_hosts file"
            }
            SchedulerErrorKind::Authentication => {
                "verify the configured SSH identity and scheduler account access"
            }
            SchedulerErrorKind::SubmissionUncertain => {
                "reconcile scheduler jobs for the execution before retrying submission"
            }
            _ => "run `beampipe doctor --profile <profile>` for scheduler diagnostics",
        })
    }
}

fn bounded_detail(value: &str) -> String {
    value.chars().take(2048).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerSubmissionRequest {
    pub execution_id: String,
    pub daliuge_session_id: String,
    pub physical_graph: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerSubmission {
    pub scheduler: SchedulerKind,
    pub external_job_id: String,
    pub remote_session_dir: Option<String>,
    pub submitted_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerJobObservation {
    pub scheduler: SchedulerKind,
    pub external_job_id: String,
    pub state: SchedulerState,
    pub raw_state: String,
    pub reason: Option<String>,
    pub source: String,
    pub exit_code: Option<i32>,
    pub observed_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerNameLookupSourceCompletion {
    pub squeue_completed: bool,
    pub sacct_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerNameLookup {
    pub job_name: String,
    pub accounting_not_before: DateTime<Utc>,
    pub query_started_at: DateTime<Utc>,
    pub query_completed_at: DateTime<Utc>,
    pub source_completion: SchedulerNameLookupSourceCompletion,
    pub matches: Vec<SchedulerJobObservation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerConnectivity {
    pub target: String,
    pub scheduler_version: Option<String>,
    pub commands: BTreeMap<String, bool>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerLogLocations {
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub scheduler_log: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerQueueInfo {
    pub partition: Option<String>,
    pub states: BTreeMap<String, u64>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerCapacity {
    pub total_nodes: u64,
    pub idle_nodes: u64,
    pub allocated_nodes: u64,
    pub down_nodes: u64,
    pub partitions: BTreeMap<String, u64>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerResourceRequest {
    pub account: String,
    pub partition: Option<String>,
    pub nodes: i32,
    pub tasks: Option<i32>,
    pub cpus_per_task: Option<i32>,
    pub memory: Option<String>,
    pub wall_time_minutes: i32,
    pub constraint: Option<String>,
    pub quality_of_service: Option<String>,
    pub modules: Vec<String>,
    pub container_runtime: Option<String>,
    pub environment_setup: Vec<String>,
}

impl SchedulerResourceRequest {
    pub fn from_slurm_profile(profile: &SlurmRemoteDeploymentConfig) -> Self {
        Self {
            account: profile.account.clone(),
            partition: profile.resources.partition.clone(),
            nodes: profile.effective_nodes(),
            tasks: profile.resources.tasks,
            cpus_per_task: profile.resources.cpus_per_task,
            memory: profile.resources.memory.clone(),
            wall_time_minutes: profile.effective_wall_time_minutes(),
            constraint: profile.resources.constraint.clone(),
            quality_of_service: profile.resources.quality_of_service.clone(),
            modules: profile.modules.as_deref().map(lines).unwrap_or_default(),
            container_runtime: profile.container_runtime.clone(),
            environment_setup: profile
                .environment_setup
                .as_deref()
                .map(lines)
                .unwrap_or_default(),
        }
    }

    pub fn render_sbatch_directives(&self) -> String {
        let mut lines = vec![
            format!("#SBATCH --account={}", self.account),
            format!("#SBATCH --nodes={}", self.nodes),
            format!(
                "#SBATCH --time={:02}:{:02}:00",
                self.wall_time_minutes / 60,
                self.wall_time_minutes % 60
            ),
        ];
        for (flag, value) in [
            ("partition", self.partition.as_deref()),
            ("mem", self.memory.as_deref()),
            ("constraint", self.constraint.as_deref()),
            ("qos", self.quality_of_service.as_deref()),
        ] {
            if let Some(value) = value {
                lines.push(format!("#SBATCH --{flag}={value}"));
            }
        }
        if let Some(tasks) = self.tasks {
            lines.push(format!("#SBATCH --ntasks={tasks}"));
        }
        if let Some(cpus) = self.cpus_per_task {
            lines.push(format!("#SBATCH --cpus-per-task={cpus}"));
        }
        lines.join("\n")
    }
}

fn lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[async_trait]
pub trait SchedulerAdapter: Send + Sync {
    fn kind(&self) -> SchedulerKind;
    fn resource_request(&self) -> Result<SchedulerResourceRequest, SchedulerAdapterError>;
    async fn test_connectivity(&self) -> Result<SchedulerConnectivity, SchedulerAdapterError>;
    async fn submit(
        &self,
        request: SchedulerSubmissionRequest,
    ) -> Result<SchedulerSubmission, SchedulerAdapterError>;
    async fn status(
        &self,
        external_job_id: &str,
    ) -> Result<SchedulerJobObservation, SchedulerAdapterError>;
    async fn status_batch(
        &self,
        external_job_ids: &[String],
    ) -> Result<Vec<SchedulerJobObservation>, SchedulerAdapterError>;
    async fn find_by_name(
        &self,
        job_name: &str,
        not_before: DateTime<Utc>,
    ) -> Result<SchedulerNameLookup, SchedulerAdapterError>;
    async fn accounting(
        &self,
        external_job_id: &str,
    ) -> Result<SchedulerJobObservation, SchedulerAdapterError>;
    async fn cancel(&self, external_job_id: &str) -> Result<(), SchedulerAdapterError>;
    async fn log_locations(
        &self,
        external_job_id: &str,
    ) -> Result<SchedulerLogLocations, SchedulerAdapterError>;
    async fn queue(&self) -> Result<SchedulerQueueInfo, SchedulerAdapterError>;
    async fn capacity(&self) -> Result<SchedulerCapacity, SchedulerAdapterError>;
}

impl SshSlurmClient {
    fn scheduler_profile(
        &self,
        operation: &str,
    ) -> Result<&SlurmRemoteDeploymentConfig, SchedulerAdapterError> {
        self.deployment.as_ref().ok_or_else(|| {
            SchedulerAdapterError::configuration(operation, "slurm deployment profile is required")
        })
    }

    fn scheduler_target(
        &self,
        operation: &str,
    ) -> Result<(SlurmTarget, String), SchedulerAdapterError> {
        let profile = self.scheduler_profile(operation)?;
        let username = self
            .remote_user
            .clone()
            .unwrap_or_else(|| resolve_remote_user(profile));
        let target = SlurmTarget::from_deployment(profile, &username);
        let display = format!("{}@{}:{}", username, target.login_node, target.ssh_port);
        Ok((target, display))
    }
}

#[async_trait]
impl SchedulerAdapter for SshSlurmClient {
    fn kind(&self) -> SchedulerKind {
        SchedulerKind::SlurmRemote
    }

    fn resource_request(&self) -> Result<SchedulerResourceRequest, SchedulerAdapterError> {
        Ok(SchedulerResourceRequest::from_slurm_profile(
            self.scheduler_profile("render_resources")?,
        ))
    }

    async fn test_connectivity(&self) -> Result<SchedulerConnectivity, SchedulerAdapterError> {
        let (target, display) = self.scheduler_target("connectivity")?;
        let mut session = SlurmSshSession::connect(&target)
            .await
            .map_err(|error| SchedulerAdapterError::backend("connectivity", &display, error))?;
        let mut commands = BTreeMap::new();
        for command in ["sbatch", "squeue", "sacct", "scancel", "scontrol"] {
            let available = session
                .run_command(&format!(
                    "command -v {command} >/dev/null 2>&1 && echo yes || echo no"
                ))
                .await
                .map_err(|error| SchedulerAdapterError::backend("connectivity", &display, error))?
                .trim()
                == "yes";
            commands.insert(command.to_string(), available);
        }
        let scheduler_version = session
            .run_command("sinfo --version")
            .await
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let _ = session.close().await;
        if commands.values().any(|available| !available) {
            return Err(SchedulerAdapterError {
                scheduler: SchedulerKind::SlurmRemote,
                operation: "connectivity".into(),
                target: display,
                kind: SchedulerErrorKind::Configuration,
                message: "one or more required SLURM commands are unavailable".into(),
                retryable: false,
                detail: Some(
                    commands
                        .iter()
                        .filter(|(_, available)| !**available)
                        .map(|(command, _)| command.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            });
        }
        Ok(SchedulerConnectivity {
            target: display,
            scheduler_version,
            commands,
            checked_at: Utc::now(),
        })
    }

    async fn submit(
        &self,
        request: SchedulerSubmissionRequest,
    ) -> Result<SchedulerSubmission, SchedulerAdapterError> {
        let profile = self.scheduler_profile("submit")?.clone();
        let username = self
            .remote_user
            .clone()
            .unwrap_or_else(|| resolve_remote_user(&profile));
        let target = format!("{}@{}:{}", username, profile.login_node, profile.ssh_port);
        let result = submit_slurm_session(SlurmSubmitParams {
            execution_id: request.execution_id,
            session_id: request.daliuge_session_id,
            pgt_json: request.physical_graph,
            deployment: profile,
            username,
        })
        .await
        .map_err(|error| SchedulerAdapterError::backend("submit", &target, error))?;
        Ok(SchedulerSubmission {
            scheduler: SchedulerKind::SlurmRemote,
            external_job_id: result.slurm_job_id,
            remote_session_dir: Some(result.session_dir),
            submitted_at: Utc::now(),
            metadata: serde_json::json!({
                "legacy_composite_job_id": result.composite_scheduler_job_id,
                "staging_root": result.staging_root,
            }),
        })
    }

    async fn status(
        &self,
        external_job_id: &str,
    ) -> Result<SchedulerJobObservation, SchedulerAdapterError> {
        let mut values = self.status_batch(&[external_job_id.to_string()]).await?;
        values.pop().ok_or_else(|| SchedulerAdapterError {
            scheduler: SchedulerKind::SlurmRemote,
            operation: "status".into(),
            target: self.login_node.clone(),
            kind: SchedulerErrorKind::NotFound,
            message: "scheduler job was not returned by status lookup".into(),
            retryable: true,
            detail: Some(external_job_id.into()),
        })
    }

    async fn status_batch(
        &self,
        external_job_ids: &[String],
    ) -> Result<Vec<SchedulerJobObservation>, SchedulerAdapterError> {
        let parsed: Vec<(String, String)> = external_job_ids
            .iter()
            .map(|raw| {
                let parsed = beampipe_domain::slurm::parse_scheduler_job_id(raw);
                (raw.clone(), parsed.slurm_job_id)
            })
            .collect();
        for (_, id) in &parsed {
            validate_job_id(id)?;
        }
        let ids: Vec<String> = parsed.iter().map(|(_, id)| id.clone()).collect();
        let (target, display) = self.scheduler_target("status_batch")?;
        let mut session = SlurmSshSession::connect(&target)
            .await
            .map_err(|error| SchedulerAdapterError::backend("status_batch", &display, error))?;
        let results = query_slurm_states_batch(&mut session, &ids)
            .await
            .map_err(|error| SchedulerAdapterError::backend("status_batch", &display, error))?;
        let _ = session.close().await;
        let observed_at = Utc::now();
        Ok(parsed
            .into_iter()
            .map(|(raw_id, id)| {
                let result = results.get(&id).cloned().unwrap_or(SlurmJobPollResult {
                    raw_state: String::new(),
                    normalized_state: "UNKNOWN".into(),
                    source: "none",
                    exit_code: None,
                    raw_line: None,
                });
                let reason = result
                    .raw_line
                    .as_deref()
                    .and_then(|line| line.split_once('|').map(|(_, reason)| reason.to_string()));
                SchedulerJobObservation {
                    scheduler: SchedulerKind::SlurmRemote,
                    external_job_id: raw_id,
                    state: SchedulerState::from_normalized(&result.normalized_state),
                    raw_state: result.raw_state,
                    reason,
                    source: result.source.into(),
                    exit_code: result.exit_code,
                    observed_at,
                    metadata: result
                        .raw_line
                        .map_or(Value::Null, |line| serde_json::json!({"raw_line": line})),
                }
            })
            .collect())
    }

    async fn find_by_name(
        &self,
        job_name: &str,
        not_before: DateTime<Utc>,
    ) -> Result<SchedulerNameLookup, SchedulerAdapterError> {
        validate_job_name(job_name)?;
        let accounting_not_before = slurm_accounting_not_before(not_before);
        let query_started_at = Utc::now();
        let (target, display) = self.scheduler_target("find_by_name")?;
        let mut session = SlurmSshSession::connect(&target)
            .await
            .map_err(|error| SchedulerAdapterError::backend("find_by_name", &display, error))?;
        let quoted_name = shell_quote(job_name);
        let squeue = session
            .run_command(&format!("squeue -h --name={quoted_name} -o '%i|%j|%T|%R'"))
            .await
            .map_err(|error| SchedulerAdapterError::backend("find_by_name", &display, error))?;
        let sacct = session
            .run_command(&sacct_name_lookup_command(job_name, &accounting_not_before))
            .await
            .map_err(|error| SchedulerAdapterError::backend("find_by_name", &display, error))?;
        let _ = session.close().await;
        let query_completed_at = Utc::now();
        complete_named_job_lookup(
            job_name,
            accounting_not_before,
            query_started_at,
            query_completed_at,
            &squeue,
            &sacct,
        )
        .map_err(|error| {
            SchedulerAdapterError::invalid_response("find_by_name", &display, error.to_string())
        })
    }

    async fn accounting(
        &self,
        external_job_id: &str,
    ) -> Result<SchedulerJobObservation, SchedulerAdapterError> {
        self.status(external_job_id).await
    }

    async fn cancel(&self, external_job_id: &str) -> Result<(), SchedulerAdapterError> {
        let parsed = beampipe_domain::slurm::parse_scheduler_job_id(external_job_id);
        validate_job_id(&parsed.slurm_job_id)?;
        let (target, display) = self.scheduler_target("cancel")?;
        let mut session = SlurmSshSession::connect(&target)
            .await
            .map_err(|error| SchedulerAdapterError::backend("cancel", &display, error))?;
        session
            .run_command(&format!("scancel -- {}", parsed.slurm_job_id))
            .await
            .map_err(|error| SchedulerAdapterError::backend("cancel", &display, error))?;
        let _ = session.close().await;
        Ok(())
    }

    async fn log_locations(
        &self,
        external_job_id: &str,
    ) -> Result<SchedulerLogLocations, SchedulerAdapterError> {
        let parsed = beampipe_domain::slurm::parse_scheduler_job_id(external_job_id);
        validate_job_id(&parsed.slurm_job_id)?;
        let root = parsed.session_dir.or_else(|| {
            self.deployment
                .as_ref()
                .map(|profile| profile.log_dir.clone())
        });
        Ok(match root {
            Some(root) => SchedulerLogLocations {
                stdout: Some(format!("{root}/slurm-{}.out", parsed.slurm_job_id)),
                stderr: Some(format!("{root}/slurm-{}.err", parsed.slurm_job_id)),
                scheduler_log: Some(root),
            },
            None => SchedulerLogLocations::default(),
        })
    }

    async fn queue(&self) -> Result<SchedulerQueueInfo, SchedulerAdapterError> {
        let (target, display) = self.scheduler_target("queue")?;
        let mut session = SlurmSshSession::connect(&target)
            .await
            .map_err(|error| SchedulerAdapterError::backend("queue", &display, error))?;
        let output = session
            .run_command("squeue -h -u \"$USER\" -o '%T|%P'")
            .await
            .map_err(|error| SchedulerAdapterError::backend("queue", &display, error))?;
        let _ = session.close().await;
        let mut states = BTreeMap::new();
        let mut partition = None;
        for line in output.lines() {
            let Some((state, observed_partition)) = line.trim().split_once('|') else {
                continue;
            };
            *states.entry(state.trim().to_ascii_lowercase()).or_insert(0) += 1;
            if partition.is_none() && !observed_partition.trim().is_empty() {
                partition = Some(observed_partition.trim().to_string());
            }
        }
        Ok(SchedulerQueueInfo {
            partition,
            states,
            observed_at: Utc::now(),
        })
    }

    async fn capacity(&self) -> Result<SchedulerCapacity, SchedulerAdapterError> {
        let (target, display) = self.scheduler_target("capacity")?;
        let mut session = SlurmSshSession::connect(&target)
            .await
            .map_err(|error| SchedulerAdapterError::backend("capacity", &display, error))?;
        let output = session
            .run_command("sinfo -h -o '%P|%D|%t'")
            .await
            .map_err(|error| SchedulerAdapterError::backend("capacity", &display, error))?;
        let _ = session.close().await;
        let mut capacity = SchedulerCapacity {
            observed_at: Utc::now(),
            ..Default::default()
        };
        for line in output.lines() {
            let fields: Vec<_> = line.trim().split('|').collect();
            if fields.len() != 3 {
                continue;
            }
            let nodes = fields[1].trim().parse::<u64>().unwrap_or(0);
            capacity.total_nodes += nodes;
            *capacity
                .partitions
                .entry(fields[0].trim_end_matches('*').to_string())
                .or_insert(0) += nodes;
            match fields[2].trim().to_ascii_lowercase().as_str() {
                "idle" => capacity.idle_nodes += nodes,
                "alloc" | "allocated" | "mix" | "mixed" => capacity.allocated_nodes += nodes,
                "down" | "drain" | "drained" => capacity.down_nodes += nodes,
                _ => {}
            }
        }
        Ok(capacity)
    }
}

fn validate_job_id(value: &str) -> Result<(), SchedulerAdapterError> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-+".contains(character))
    {
        return Err(SchedulerAdapterError::configuration(
            "job_id",
            "scheduler job ID contains unsupported characters",
        ));
    }
    Ok(())
}

fn validate_job_name(value: &str) -> Result<(), SchedulerAdapterError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(SchedulerAdapterError::configuration(
            "job_name",
            "scheduler job name contains unsupported characters",
        ));
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn slurm_accounting_not_before(requested: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(requested.timestamp(), 0)
        .expect("a chrono UTC timestamp remains valid when truncated to whole seconds")
}

fn sacct_name_lookup_command(job_name: &str, not_before: &DateTime<Utc>) -> String {
    let quoted_name = shell_quote(job_name);
    let quoted_not_before = shell_quote(&not_before.format("%Y-%m-%dT%H:%M:%S").to_string());
    format!(
        "TZ=UTC sacct -n -X --name={quoted_name} --starttime={quoted_not_before} -o 'JobIDRaw,JobName,State,Reason' -P"
    )
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("{query_source} line {line_number}: {message}")]
struct NamedJobParseError {
    query_source: String,
    line_number: usize,
    message: String,
}

impl NamedJobParseError {
    fn new(source: &str, line_number: usize, message: impl Into<String>) -> Self {
        Self {
            query_source: source.into(),
            line_number,
            message: message.into(),
        }
    }
}

fn parse_named_jobs(
    output: &str,
    expected_name: &str,
    source: &str,
    observed_at: DateTime<Utc>,
) -> Result<Vec<SchedulerJobObservation>, NamedJobParseError> {
    let mut observations = Vec::new();
    for (index, raw_line) in output.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() != 4 {
            return Err(NamedJobParseError::new(
                source,
                line_number,
                format!(
                    "expected 4 pipe-delimited fields, received {}",
                    fields.len()
                ),
            ));
        }
        let returned_name = fields[1].trim();
        if returned_name.is_empty() {
            return Err(NamedJobParseError::new(
                source,
                line_number,
                "job name is empty",
            ));
        }
        if returned_name != expected_name {
            continue;
        }
        let external_job_id = fields[0].trim();
        if validate_slurm_job_id(external_job_id).is_err() {
            return Err(NamedJobParseError::new(
                source,
                line_number,
                "exact-name row contains an invalid allocation job ID",
            ));
        }
        let Some(raw_state) = fields[2].split_whitespace().next().map(str::to_string) else {
            return Err(NamedJobParseError::new(
                source,
                line_number,
                "exact-name row contains an empty state",
            ));
        };
        let reason = fields[3].trim();
        let reason = (!reason.is_empty() && reason != "None").then(|| reason.to_string());
        observations.push(SchedulerJobObservation {
            scheduler: SchedulerKind::SlurmRemote,
            external_job_id: external_job_id.to_string(),
            state: SchedulerState::from_normalized(&raw_state),
            raw_state,
            reason,
            source: source.into(),
            exit_code: None,
            observed_at,
            metadata: serde_json::json!({"job_name": expected_name}),
        });
    }
    Ok(observations)
}

fn complete_named_job_lookup(
    job_name: &str,
    accounting_not_before: DateTime<Utc>,
    query_started_at: DateTime<Utc>,
    query_completed_at: DateTime<Utc>,
    squeue_output: &str,
    sacct_output: &str,
) -> Result<SchedulerNameLookup, NamedJobParseError> {
    let squeue = parse_named_jobs(squeue_output, job_name, "squeue", query_completed_at)?;
    let sacct = parse_named_jobs(sacct_output, job_name, "sacct", query_completed_at)?;
    Ok(SchedulerNameLookup {
        job_name: job_name.into(),
        accounting_not_before,
        query_started_at,
        query_completed_at,
        source_completion: SchedulerNameLookupSourceCompletion {
            squeue_completed: true,
            sacct_completed: true,
        },
        matches: merge_named_job_observations(squeue, sacct),
    })
}

fn merge_named_job_observations(
    squeue: Vec<SchedulerJobObservation>,
    sacct: Vec<SchedulerJobObservation>,
) -> Vec<SchedulerJobObservation> {
    let mut observations = BTreeMap::new();
    for observation in sacct {
        observations.insert(observation.external_job_id.clone(), observation);
    }
    // Live queue state is authoritative when accounting has already observed
    // the same allocation during a state transition.
    for observation in squeue {
        observations.insert(observation.external_job_id.clone(), observation);
    }
    observations.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use beampipe_profiles::{DaliugeManagerTopologyConfig, SlurmResourceConfig};
    use chrono::TimeZone;

    fn timestamp(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 22, 10, minute, 0)
            .single()
            .expect("valid timestamp")
    }

    fn profile() -> SlurmRemoteDeploymentConfig {
        SlurmRemoteDeploymentConfig {
            login_node: "login.example".into(),
            ssh_port: 22,
            remote_user: Some("operator".into()),
            ssh_credential: None,
            account: "science".into(),
            home_dir: "/home/operator".into(),
            log_dir: "/logs".into(),
            exec_prefix: "srun".into(),
            dlg_root: "/dlg".into(),
            venv: None,
            modules: Some("module load singularity\nmodule load python".into()),
            facility: "test".into(),
            job_duration_minutes: 30,
            num_nodes: 1,
            num_islands: 1,
            verbose_level: 1,
            max_threads: 0,
            all_nics: false,
            zerorun: false,
            sleepncopy: false,
            check_with_session: false,
            verify_ssl: None,
            slurm_template: None,
            resources: SlurmResourceConfig {
                partition: Some("compute".into()),
                nodes: Some(2),
                tasks: Some(4),
                cpus_per_task: Some(8),
                memory: Some("64G".into()),
                wall_time_minutes: Some(125),
                constraint: None,
                quality_of_service: Some("normal".into()),
            },
            manager_topology: DaliugeManagerTopologyConfig::default(),
            container_runtime: Some("singularity".into()),
            environment_setup: None,
        }
    }

    #[test]
    fn resource_request_renders_final_scheduler_values() {
        let resources = SchedulerResourceRequest::from_slurm_profile(&profile());
        let rendered = resources.render_sbatch_directives();
        assert!(rendered.contains("#SBATCH --partition=compute"));
        assert!(rendered.contains("#SBATCH --nodes=2"));
        assert!(rendered.contains("#SBATCH --time=02:05:00"));
        assert!(rendered.contains("#SBATCH --cpus-per-task=8"));
    }

    #[test]
    fn scheduler_job_ids_are_shell_safe() {
        assert!(validate_job_id("12345_7").is_ok());
        assert!(validate_job_id("123;rm -rf /").is_err());
    }

    #[test]
    fn accounting_lookup_uses_and_reports_its_utc_lower_bound() {
        let requested = timestamp(0) + chrono::Duration::milliseconds(987);
        let accounting_not_before = slurm_accounting_not_before(requested);
        let command = sacct_name_lookup_command("BeampipeExecution-abc", &accounting_not_before);

        assert_eq!(accounting_not_before, timestamp(0));
        assert!(command.starts_with("TZ=UTC sacct "));
        assert!(command.contains("--name='BeampipeExecution-abc'"));
        assert!(command.contains("--starttime='2026-08-22T10:00:00'"));
        assert!(!command.contains("now-7days"));
    }

    #[test]
    fn submission_uncertainty_is_preserved_by_scheduler_adapter() {
        let error = SchedulerAdapterError::backend(
            "submit",
            "operator@login.example:22",
            OrchestrationError::SubmissionUncertain("response lost after dispatch".into()),
        );

        assert_eq!(error.kind, SchedulerErrorKind::SubmissionUncertain);
        assert!(!error.retryable);
        assert_eq!(error.failure_class(), FailureClass::InconsistentState);
    }

    #[test]
    fn named_job_lookup_requires_exact_correlation_name() {
        let observations = parse_named_jobs(
            "123|BeampipeExecution-abc|RUNNING|None\n124|other|PENDING|Resources\n",
            "BeampipeExecution-abc",
            "squeue",
            timestamp(1),
        )
        .expect("valid scheduler output");
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].external_job_id, "123");
        assert_eq!(observations[0].state, SchedulerState::Running);
    }

    #[test]
    fn named_job_lookup_unions_queue_and_accounting_results() {
        let observations = merge_named_job_observations(
            parse_named_jobs(
                "123|BeampipeExecution-abc|RUNNING|None\n",
                "BeampipeExecution-abc",
                "squeue",
                timestamp(1),
            )
            .expect("valid squeue output"),
            parse_named_jobs(
                "124|BeampipeExecution-abc|COMPLETED|None\n",
                "BeampipeExecution-abc",
                "sacct",
                timestamp(1),
            )
            .expect("valid sacct output"),
        );

        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].external_job_id, "123");
        assert_eq!(observations[1].external_job_id, "124");
    }

    #[test]
    fn named_job_lookup_deduplicates_with_live_queue_precedence() {
        let observations = merge_named_job_observations(
            parse_named_jobs(
                "123|BeampipeExecution-abc|RUNNING|None\n",
                "BeampipeExecution-abc",
                "squeue",
                timestamp(1),
            )
            .expect("valid squeue output"),
            parse_named_jobs(
                "123|BeampipeExecution-abc|PENDING|Resources\n",
                "BeampipeExecution-abc",
                "sacct",
                timestamp(1),
            )
            .expect("valid sacct output"),
        );

        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].source, "squeue");
        assert_eq!(observations[0].state, SchedulerState::Running);
    }

    #[test]
    fn named_job_lookup_envelope_proves_complete_union_scope() {
        let lookup = complete_named_job_lookup(
            "BeampipeExecution-abc",
            timestamp(0),
            timestamp(1),
            timestamp(2),
            "123|BeampipeExecution-abc|RUNNING|None\n",
            "123|BeampipeExecution-abc|PENDING|Resources\n124|BeampipeExecution-abc|COMPLETED|None\n",
        )
        .expect("valid scheduler output");

        assert_eq!(lookup.job_name, "BeampipeExecution-abc");
        assert_eq!(lookup.accounting_not_before, timestamp(0));
        assert_eq!(lookup.query_started_at, timestamp(1));
        assert_eq!(lookup.query_completed_at, timestamp(2));
        assert_eq!(
            lookup.source_completion,
            SchedulerNameLookupSourceCompletion {
                squeue_completed: true,
                sacct_completed: true,
            }
        );
        assert_eq!(lookup.matches.len(), 2);
        assert_eq!(lookup.matches[0].external_job_id, "123");
        assert_eq!(lookup.matches[0].source, "squeue");
        assert_eq!(lookup.matches[1].external_job_id, "124");
        assert!(lookup
            .matches
            .iter()
            .all(|observation| observation.observed_at == timestamp(2)));
    }

    #[test]
    fn named_job_lookup_rejects_malformed_nonempty_output() {
        let error = parse_named_jobs(
            "this is not a parsable scheduler row\n",
            "BeampipeExecution-abc",
            "sacct",
            timestamp(1),
        )
        .expect_err("malformed output must not become negative evidence");

        assert_eq!(error.query_source, "sacct");
        assert_eq!(error.line_number, 1);
        assert!(error.message.contains("4 pipe-delimited fields"));
    }

    #[test]
    fn named_job_lookup_rejects_invalid_exact_name_job_id() {
        for invalid_id in ["123.batch", "123_4", "not-a-job", "123;touch"] {
            let output = format!("{invalid_id}|BeampipeExecution-abc|COMPLETED|None\n");
            let error = parse_named_jobs(&output, "BeampipeExecution-abc", "sacct", timestamp(1))
                .expect_err("invalid IDs must not become matches or negative evidence");

            assert!(error.message.contains("invalid allocation job ID"));
        }
    }

    #[test]
    fn named_job_lookup_accepts_complete_empty_results() {
        let lookup = complete_named_job_lookup(
            "BeampipeExecution-abc",
            timestamp(0),
            timestamp(1),
            timestamp(2),
            "\n",
            "",
        )
        .expect("empty output from both completed queries is valid negative evidence");

        assert!(lookup.matches.is_empty());
        assert!(lookup.source_completion.squeue_completed);
        assert!(lookup.source_completion.sacct_completed);
    }
}

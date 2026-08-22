use crate::slurm_deploy::resolve_remote_user;
use crate::slurm_ssh::{scancel_command, validate_slurm_job_id};
use crate::{
    query_slurm_states_batch, BackendPoll, DimClient, HttpClientOptions, HttpDimClient,
    OrchestrationError, SlurmSshSession, SlurmTarget,
};
use async_trait::async_trait;
use beampipe_domain::{slurm, ExecutionStatus};
use beampipe_profiles::{
    DeploymentConfig, RestRemoteDeploymentConfig, SlurmRemoteDeploymentConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const SLURM_CANCEL_MAX_POLLS: usize = 11;
const SLURM_CANCEL_POLL_INTERVAL: Duration = Duration::from_secs(3);
const SLURM_CANCEL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(45);
const SLURM_CANCEL_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct CancelParams {
    pub scheduler_job_id: Option<String>,
    pub daliuge_session_id: Option<String>,
    pub deployment: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResult {
    pub cancelled: bool,
    pub reason: Option<String>,
}

pub async fn cancel_scheduler_session(
    params: CancelParams,
) -> Result<CancelResult, OrchestrationError> {
    let deployment = serde_json::from_value::<DeploymentConfig>(params.deployment)
        .map_err(|e| OrchestrationError::Backend(format!("invalid deployment profile: {e}")))?;
    match deployment {
        DeploymentConfig::RestRemote(rest) => match params.daliuge_session_id {
            Some(session_id) => cancel_rest(&session_id, &rest).await,
            None => Ok(CancelResult {
                cancelled: false,
                reason: Some("no_daliuge_session_id".into()),
            }),
        },
        DeploymentConfig::SlurmRemote(slurm) => match params.scheduler_job_id {
            Some(job_id) => cancel_slurm(&job_id, &slurm).await,
            None => Ok(CancelResult {
                cancelled: false,
                reason: Some("no_scheduler_job_id".into()),
            }),
        },
    }
}

async fn cancel_rest(
    session_id: &str,
    rest: &RestRemoteDeploymentConfig,
) -> Result<CancelResult, OrchestrationError> {
    let Some(dim_base) = rest_endpoint(rest) else {
        return Ok(CancelResult {
            cancelled: false,
            reason: Some("incomplete_profile".into()),
        });
    };
    let client = HttpDimClient::with_options(
        dim_base,
        HttpClientOptions::dim_default().with_verify_ssl(rest.verify_ssl),
    );
    match client.cancel(session_id).await {
        Ok(()) => Ok(CancelResult {
            cancelled: true,
            reason: None,
        }),
        Err(e) => Ok(CancelResult {
            cancelled: false,
            reason: Some(e.to_string()),
        }),
    }
}

async fn cancel_slurm(
    scheduler_job_id: &str,
    slurm: &SlurmRemoteDeploymentConfig,
) -> Result<CancelResult, OrchestrationError> {
    match tokio::time::timeout(
        SLURM_CANCEL_TOTAL_TIMEOUT,
        cancel_slurm_remote(scheduler_job_id, slurm),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Ok(unconfirmed(format!(
            "slurm cancellation remained unconfirmed after the {} second total timeout",
            SLURM_CANCEL_TOTAL_TIMEOUT.as_secs()
        ))),
    }
}

async fn cancel_slurm_remote(
    scheduler_job_id: &str,
    slurm: &SlurmRemoteDeploymentConfig,
) -> Result<CancelResult, OrchestrationError> {
    let parsed = slurm::parse_scheduler_job_id(scheduler_job_id);
    let slurm_id = if parsed.slurm_job_id.is_empty() {
        scheduler_job_id
            .rsplit(':')
            .next()
            .unwrap_or(scheduler_job_id)
            .to_string()
    } else {
        parsed.slurm_job_id
    };
    if let Err(error) = validate_slurm_job_id(&slurm_id) {
        return Ok(unconfirmed(format!(
            "slurm cancellation job identifier is invalid: {error}"
        )));
    }
    let command = match scancel_command(&slurm_id) {
        Ok(command) => command,
        Err(error) => {
            return Ok(unconfirmed(format!(
                "slurm cancellation command could not be built: {error}"
            )))
        }
    };
    let username = resolve_remote_user(slurm);
    let target = SlurmTarget::from_deployment(slurm, &username);
    let mut session = match SlurmSshSession::connect(&target).await {
        Ok(session) => session,
        Err(error) => {
            return Ok(unconfirmed(format!(
                "slurm cancellation connection failed: {error}"
            )))
        }
    };
    if let Err(error) = session.run_command(&command).await {
        let _ = session.close().await;
        return Ok(unconfirmed(format!(
            "slurm cancellation request failed: {error}"
        )));
    }

    let mut poller = SshCancellationPoller {
        session: &mut session,
        scheduler_job_id,
        slurm_id,
    };
    let confirmation = tokio::time::timeout(
        SLURM_CANCEL_CONFIRM_TIMEOUT,
        confirm_slurm_cancellation(
            scheduler_job_id,
            &mut poller,
            SlurmCancelPollPolicy {
                max_polls: SLURM_CANCEL_MAX_POLLS,
                poll_interval: SLURM_CANCEL_POLL_INTERVAL,
            },
        ),
    )
    .await;
    drop(poller);
    let _ = session.close().await;
    Ok(match confirmation {
        Ok(result) => result,
        Err(_) => unconfirmed(format!(
            "slurm cancellation remained unconfirmed after {} seconds",
            SLURM_CANCEL_CONFIRM_TIMEOUT.as_secs()
        )),
    })
}

#[derive(Debug, Clone, Copy)]
struct SlurmCancelPollPolicy {
    max_polls: usize,
    poll_interval: Duration,
}

#[async_trait]
trait SlurmCancellationPoller {
    async fn poll(
        &mut self,
        scheduler_job_id: &str,
    ) -> Result<BackendPoll, OrchestrationError>;
}

struct SshCancellationPoller<'a> {
    session: &'a mut SlurmSshSession,
    scheduler_job_id: &'a str,
    slurm_id: String,
}

#[async_trait]
impl SlurmCancellationPoller for SshCancellationPoller<'_> {
    async fn poll(
        &mut self,
        scheduler_job_id: &str,
    ) -> Result<BackendPoll, OrchestrationError> {
        if scheduler_job_id != self.scheduler_job_id {
            return Err(OrchestrationError::Backend(
                "cancellation poll job identifier changed".into(),
            ));
        }
        let results = query_slurm_states_batch(
            self.session,
            std::slice::from_ref(&self.slurm_id),
        )
        .await?;
        let result = results.get(&self.slurm_id).cloned().ok_or_else(|| {
            OrchestrationError::Backend(format!(
                "no cancellation poll result for slurm job {}",
                self.slurm_id
            ))
        })?;
        let normalized = result.normalized_state.clone();
        let status = match normalized.as_str() {
            "COMPLETED" => ExecutionStatus::Completed,
            "FAILED" | "TIMEOUT" => ExecutionStatus::Failed,
            "CANCELLED" => ExecutionStatus::Cancelled,
            "RUNNING" => ExecutionStatus::Running,
            "PENDING" => ExecutionStatus::AwaitingScheduler,
            _ => ExecutionStatus::AwaitingScheduler,
        };
        Ok(BackendPoll {
            status,
            poll_summary: serde_json::json!({
                "scheduler_job_id": scheduler_job_id,
                "normalized_state": normalized,
                "raw_state": result.raw_state,
                "slurm_job_id": self.slurm_id.clone(),
                "source": result.source,
                "exit_code": result.exit_code,
            }),
        })
    }
}

async fn confirm_slurm_cancellation<P: SlurmCancellationPoller + ?Sized>(
    scheduler_job_id: &str,
    poller: &mut P,
    policy: SlurmCancelPollPolicy,
) -> CancelResult {
    let max_polls = policy.max_polls.max(1);
    let mut last_status = "unobserved".to_string();
    for poll_number in 1..=max_polls {
        let observation = match poller.poll(scheduler_job_id).await {
            Ok(observation) => observation,
            Err(error) => {
                return unconfirmed(format!(
                    "slurm cancellation was accepted but confirmation polling failed: {error}"
                ))
            }
        };
        last_status = observation
            .poll_summary
            .get("normalized_state")
            .and_then(Value::as_str)
            .unwrap_or_else(|| observation.status.as_str())
            .to_string();
        if observation.status == beampipe_domain::ExecutionStatus::Cancelled {
            return CancelResult {
                cancelled: true,
                reason: None,
            };
        }
        if observation.status.is_terminal() {
            return unconfirmed(format!(
                "slurm job reached terminal state {} before cancellation was confirmed",
                observation.status.as_str()
            ));
        }
        if poll_number < max_polls && !policy.poll_interval.is_zero() {
            tokio::time::sleep(policy.poll_interval).await;
        }
    }

    unconfirmed(format!(
        "slurm cancellation remained unconfirmed after {max_polls} polls; last status was {last_status}"
    ))
}

fn unconfirmed(reason: String) -> CancelResult {
    CancelResult {
        cancelled: false,
        reason: Some(reason),
    }
}

pub fn rest_endpoint(rest: &RestRemoteDeploymentConfig) -> Option<String> {
    let host = rest.deploy_host.as_deref()?.trim();
    if host.is_empty() {
        return None;
    }
    let port = rest.deploy_port.unwrap_or(8001);
    Some(crate::dim::dim_rest_base(host, port, rest.use_https))
}

#[cfg(test)]
mod tests {
    use super::{
        confirm_slurm_cancellation, SlurmCancelPollPolicy, SlurmCancellationPoller,
    };
    use crate::{BackendPoll, OrchestrationError};
    use async_trait::async_trait;
    use beampipe_domain::ExecutionStatus;
    use std::{collections::VecDeque, time::Duration};

    enum PollStep {
        Status(ExecutionStatus, &'static str),
        Error(&'static str),
    }

    struct FakeSlurmPoller {
        steps: VecDeque<PollStep>,
        polled_ids: Vec<String>,
    }

    impl FakeSlurmPoller {
        fn new(steps: impl IntoIterator<Item = PollStep>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
                polled_ids: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl SlurmCancellationPoller for FakeSlurmPoller {
        async fn poll(
            &mut self,
            scheduler_job_id: &str,
        ) -> Result<BackendPoll, OrchestrationError> {
            self.polled_ids.push(scheduler_job_id.to_string());
            match self.steps.pop_front() {
                Some(PollStep::Status(status, normalized_state)) => Ok(BackendPoll {
                    status,
                    poll_summary: serde_json::json!({
                        "normalized_state": normalized_state,
                    }),
                }),
                Some(PollStep::Error(message)) => {
                    Err(OrchestrationError::Backend(message.into()))
                }
                None => Err(OrchestrationError::Backend(
                    "fake poll sequence exhausted".into(),
                )),
            }
        }
    }

    fn test_policy(max_polls: usize) -> SlurmCancelPollPolicy {
        SlurmCancelPollPolicy {
            max_polls,
            poll_interval: Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn confirms_only_cancelled_for_the_exact_job_id() {
        let mut poller = FakeSlurmPoller::new([
            PollStep::Status(ExecutionStatus::Running, "RUNNING"),
            PollStep::Status(ExecutionStatus::Cancelled, "CANCELLED"),
        ]);

        let result = confirm_slurm_cancellation(
            "session:12345|/dlg/run",
            &mut poller,
            test_policy(3),
        )
        .await;

        assert!(result.cancelled);
        assert_eq!(result.reason, None);
        assert_eq!(
            poller.polled_ids,
            ["session:12345|/dlg/run", "session:12345|/dlg/run"]
        );
    }

    #[tokio::test]
    async fn completion_race_is_not_reported_as_cancelled() {
        let mut poller = FakeSlurmPoller::new([PollStep::Status(
            ExecutionStatus::Completed,
            "COMPLETED",
        )]);

        let result = confirm_slurm_cancellation("12345", &mut poller, test_policy(3)).await;

        assert!(!result.cancelled);
        assert!(result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("terminal state completed")));
        assert_eq!(poller.polled_ids.len(), 1);
    }

    #[tokio::test]
    async fn unknown_state_times_out_without_confirmation() {
        let mut poller = FakeSlurmPoller::new([
            PollStep::Status(ExecutionStatus::AwaitingScheduler, "UNKNOWN"),
            PollStep::Status(ExecutionStatus::AwaitingScheduler, "UNKNOWN"),
            PollStep::Status(ExecutionStatus::AwaitingScheduler, "UNKNOWN"),
        ]);

        let result = confirm_slurm_cancellation("12345", &mut poller, test_policy(3)).await;

        assert!(!result.cancelled);
        assert!(result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("unconfirmed after 3 polls")
                && reason.contains("UNKNOWN")));
        assert_eq!(poller.polled_ids.len(), 3);
    }

    #[tokio::test]
    async fn poll_error_is_not_reported_as_cancelled() {
        let mut poller = FakeSlurmPoller::new([PollStep::Error("scheduler unavailable")]);

        let result = confirm_slurm_cancellation("12345", &mut poller, test_policy(3)).await;

        assert!(!result.cancelled);
        assert!(result.reason.as_deref().is_some_and(|reason| {
            reason.contains("confirmation polling failed")
                && reason.contains("scheduler unavailable")
        }));
        assert_eq!(poller.polled_ids.len(), 1);
    }
}

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

use crate::{DaliugeState, SchedulerState, SubmissionState, TerminalOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Pending,
    Running,
    AwaitingScheduler,
    NotSubmitted,
    Completed,
    Failed,
    Retrying,
    Cancelled,
}

impl ExecutionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::AwaitingScheduler => "awaiting_scheduler",
            Self::NotSubmitted => "not_submitted",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Retrying => "retrying",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::NotSubmitted
        )
    }

    pub fn is_locked_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }

    pub fn allows(self, next: Self) -> bool {
        use ExecutionStatus::*;
        match self {
            Pending => matches!(next, Running | Cancelled | Failed),
            Running => matches!(
                next,
                AwaitingScheduler | Completed | NotSubmitted | Failed | Cancelled
            ),
            AwaitingScheduler => matches!(next, Running | Completed | Failed | Cancelled),
            NotSubmitted => matches!(next, Running | Cancelled),
            Failed => matches!(next, Retrying | Running | Cancelled),
            Retrying => matches!(next, Running | Failed | Cancelled),
            Completed | Cancelled => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    StageAndManifest,
    Submit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRetryStage {
    StageAndManifest,
    Submit,
}

impl ExecutionRetryStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StageAndManifest => "stage_and_manifest",
            Self::Submit => "submit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionRetryContext {
    pub status: ExecutionStatus,
    pub phase: Option<ExecutionPhase>,
    pub submission: SubmissionState,
    pub scheduler: SchedulerState,
    pub daliuge: DaliugeState,
    pub terminal_outcome: Option<TerminalOutcome>,
    pub has_manifest: bool,
    pub has_scheduler_job_id: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ExecutionRetryPlan {
    pub stage: ExecutionRetryStage,
    pub do_stage: bool,
    pub do_submit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ExecutionRetryRejection {
    pub code: String,
    pub message: String,
}

pub fn plan_execution_retry(
    context: ExecutionRetryContext,
) -> Result<ExecutionRetryPlan, ExecutionRetryRejection> {
    let reject = |code: &str, message: &str| ExecutionRetryRejection {
        code: code.into(),
        message: message.into(),
    };
    if context.status != ExecutionStatus::Failed {
        return Err(reject(
            "execution_not_failed",
            "only a failed execution can be retried in place",
        ));
    }
    if context.terminal_outcome == Some(TerminalOutcome::Inconsistent) {
        return Err(reject(
            "external_state_inconsistent",
            "the external state is inconsistent; reconcile or create a new execution instead of replaying work",
        ));
    }
    if matches!(
        context.submission,
        SubmissionState::Preparing
            | SubmissionState::InFlight
            | SubmissionState::Submitted
            | SubmissionState::Uncertain
    ) {
        return Err(reject(
            "submission_may_exist",
            "submission may have reached an external system; retry is blocked to prevent duplicate work",
        ));
    }
    if context.has_scheduler_job_id || context.scheduler != SchedulerState::NotSubmitted {
        return Err(reject(
            "scheduler_work_exists",
            "a scheduler job or scheduler observation exists; create a new execution after reconciliation",
        ));
    }
    if context.daliuge != DaliugeState::NotCreated {
        return Err(reject(
            "daliuge_session_may_exist",
            "DALiuGE is not definitively in the not-created state; retry is blocked to prevent duplicate sessions",
        ));
    }
    let stage = if context.phase == Some(ExecutionPhase::Submit)
        || (context.submission == SubmissionState::Failed && context.has_manifest)
    {
        if !context.has_manifest {
            return Err(reject(
                "retry_manifest_missing",
                "the submit stage cannot be retried because its pinned manifest is missing",
            ));
        }
        ExecutionRetryStage::Submit
    } else {
        ExecutionRetryStage::StageAndManifest
    };
    Ok(ExecutionRetryPlan {
        stage,
        do_stage: stage == ExecutionRetryStage::StageAndManifest,
        do_submit: true,
    })
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("invalid status transition from {from:?} to {to:?}")]
    Invalid {
        from: ExecutionStatus,
        to: ExecutionStatus,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct LedgerPatch {
    pub status: Option<ExecutionStatus>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub workflow_manifest: Option<serde_json::Value>,
    pub error: Option<String>,
    pub execution_phase: Option<Option<ExecutionPhase>>,
    pub clear_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct LedgerState {
    pub status: ExecutionStatus,
    pub execution_phase: Option<ExecutionPhase>,
    pub retry_count: i32,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub workflow_manifest: Option<serde_json::Value>,
    pub last_error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl LedgerState {
    pub fn apply_patch(
        &mut self,
        patch: LedgerPatch,
        now: DateTime<Utc>,
    ) -> Result<(), TransitionError> {
        let current = self.status;
        let mut effective = patch.status;

        if let Some(next) = effective {
            if current.is_locked_terminal() && next != current {
                effective = None;
            } else if next != current && !current.allows(next) {
                return Err(TransitionError::Invalid {
                    from: current,
                    to: next,
                });
            }
        }

        if let Some(next) = effective {
            self.status = next;
            if next == ExecutionStatus::Running && self.started_at.is_none() {
                self.started_at = Some(now);
            }
            if next.is_terminal() && self.completed_at.is_none() {
                self.completed_at = Some(now);
            }
            if matches!(
                (current, next),
                (ExecutionStatus::Failed, ExecutionStatus::Retrying)
            ) {
                self.retry_count += 1;
            }
            let recovering = matches!(current, ExecutionStatus::Failed | ExecutionStatus::Retrying)
                && matches!(next, ExecutionStatus::Running | ExecutionStatus::Retrying);
            if recovering && patch.error.is_none() {
                self.last_error = None;
            }
            if next.is_terminal() && patch.execution_phase.is_none() {
                self.execution_phase = None;
            }
        }

        if let Some(v) = patch.scheduler_name {
            self.scheduler_name = Some(v);
        }
        if let Some(v) = patch.scheduler_job_id {
            self.scheduler_job_id = Some(v);
        }
        if let Some(v) = patch.workflow_manifest {
            self.workflow_manifest = Some(v);
        }
        if let Some(v) = patch.error {
            self.last_error = Some(v);
        } else if patch.clear_error {
            self.last_error = None;
        }
        if let Some(v) = patch.execution_phase {
            self.execution_phase = v;
        }
        self.updated_at = Some(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(status: ExecutionStatus) -> LedgerState {
        LedgerState {
            status,
            execution_phase: Some(ExecutionPhase::Submit),
            retry_count: 0,
            scheduler_name: None,
            scheduler_job_id: None,
            workflow_manifest: None,
            last_error: None,
            started_at: None,
            completed_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn valid_transitions_match_python_fsm() {
        assert!(ExecutionStatus::Pending.allows(ExecutionStatus::Running));
        assert!(ExecutionStatus::Pending.allows(ExecutionStatus::Failed));
        assert!(ExecutionStatus::Pending.allows(ExecutionStatus::Cancelled));
        assert!(ExecutionStatus::Running.allows(ExecutionStatus::AwaitingScheduler));
        assert!(ExecutionStatus::Running.allows(ExecutionStatus::NotSubmitted));
        assert!(ExecutionStatus::Running.allows(ExecutionStatus::Failed));
        assert!(ExecutionStatus::AwaitingScheduler.allows(ExecutionStatus::Completed));
        assert!(ExecutionStatus::AwaitingScheduler.allows(ExecutionStatus::Cancelled));
        assert!(ExecutionStatus::NotSubmitted.allows(ExecutionStatus::Running));
        assert!(ExecutionStatus::Failed.allows(ExecutionStatus::Retrying));
        assert!(!ExecutionStatus::Completed.allows(ExecutionStatus::Running));
        assert!(!ExecutionStatus::Cancelled.allows(ExecutionStatus::Running));
    }

    #[test]
    fn terminal_states_are_terminal() {
        assert!(ExecutionStatus::Completed.is_terminal());
        assert!(ExecutionStatus::Failed.is_terminal());
        assert!(ExecutionStatus::Cancelled.is_terminal());
        assert!(ExecutionStatus::NotSubmitted.is_terminal());
        assert!(!ExecutionStatus::Running.is_terminal());
    }

    #[test]
    fn locked_terminal_overwrite_is_ignored() {
        let now = Utc::now();
        let mut st = state(ExecutionStatus::Completed);
        st.completed_at = Some(now);
        st.apply_patch(
            LedgerPatch {
                status: Some(ExecutionStatus::Failed),
                ..LedgerPatch::default()
            },
            now,
        )
        .unwrap();
        assert_eq!(st.status, ExecutionStatus::Completed);
    }

    #[test]
    fn retry_count_increments_once_per_retry_attempt() {
        let now = Utc::now();
        let mut st = state(ExecutionStatus::Failed);
        st.last_error = Some("boom".into());
        st.apply_patch(
            LedgerPatch {
                status: Some(ExecutionStatus::Retrying),
                ..LedgerPatch::default()
            },
            now,
        )
        .unwrap();
        assert_eq!(st.retry_count, 1);
        assert!(st.last_error.is_none());
        st.apply_patch(
            LedgerPatch {
                status: Some(ExecutionStatus::Running),
                ..LedgerPatch::default()
            },
            now,
        )
        .unwrap();
        assert_eq!(st.retry_count, 1);
    }

    #[test]
    fn retry_plan_resumes_known_pre_submission_failure() {
        let plan = plan_execution_retry(ExecutionRetryContext {
            status: ExecutionStatus::Failed,
            phase: Some(ExecutionPhase::Submit),
            submission: SubmissionState::Failed,
            scheduler: SchedulerState::NotSubmitted,
            daliuge: DaliugeState::NotCreated,
            terminal_outcome: Some(TerminalOutcome::Failed),
            has_manifest: true,
            has_scheduler_job_id: false,
        })
        .unwrap();
        assert_eq!(plan.stage, ExecutionRetryStage::Submit);
        assert!(!plan.do_stage);
        assert!(plan.do_submit);
    }

    #[test]
    fn retry_plan_blocks_uncertain_submission() {
        let rejection = plan_execution_retry(ExecutionRetryContext {
            status: ExecutionStatus::Failed,
            phase: Some(ExecutionPhase::Submit),
            submission: SubmissionState::Uncertain,
            scheduler: SchedulerState::Unknown,
            daliuge: DaliugeState::Unknown,
            terminal_outcome: Some(TerminalOutcome::Inconsistent),
            has_manifest: true,
            has_scheduler_job_id: false,
        })
        .unwrap_err();
        assert_eq!(rejection.code, "external_state_inconsistent");
    }

    #[test]
    fn retry_plan_restarts_manifest_stage_before_submission() {
        let plan = plan_execution_retry(ExecutionRetryContext {
            status: ExecutionStatus::Failed,
            phase: Some(ExecutionPhase::StageAndManifest),
            submission: SubmissionState::NotStarted,
            scheduler: SchedulerState::NotSubmitted,
            daliuge: DaliugeState::NotCreated,
            terminal_outcome: Some(TerminalOutcome::Failed),
            has_manifest: false,
            has_scheduler_job_id: false,
        })
        .unwrap();
        assert_eq!(plan.stage, ExecutionRetryStage::StageAndManifest);
        assert!(plan.do_stage);
    }
}

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ExecutionStatus;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ControlPhase {
    #[default]
    Discovered,
    AdmissionPending,
    Admitted,
    ManifestGenerated,
    GraphPatched,
    Translated,
    SubmissionPending,
    Submitted,
    Monitoring,
    OutputVerification,
    Terminal,
}

impl ControlPhase {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "discovered" => Self::Discovered,
            "admission_pending" => Self::AdmissionPending,
            "admitted" => Self::Admitted,
            "manifest_generated" => Self::ManifestGenerated,
            "graph_patched" => Self::GraphPatched,
            "translated" => Self::Translated,
            "submission_pending" => Self::SubmissionPending,
            "submitted" => Self::Submitted,
            "monitoring" => Self::Monitoring,
            "output_verification" => Self::OutputVerification,
            "terminal" => Self::Terminal,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::AdmissionPending => "admission_pending",
            Self::Admitted => "admitted",
            Self::ManifestGenerated => "manifest_generated",
            Self::GraphPatched => "graph_patched",
            Self::Translated => "translated",
            Self::SubmissionPending => "submission_pending",
            Self::Submitted => "submitted",
            Self::Monitoring => "monitoring",
            Self::OutputVerification => "output_verification",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    #[default]
    NotStarted,
    Preparing,
    InFlight,
    Submitted,
    Uncertain,
    Failed,
}

impl SubmissionState {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "not_started" => Self::NotStarted,
            "preparing" => Self::Preparing,
            "in_flight" => Self::InFlight,
            "submitted" => Self::Submitted,
            "uncertain" => Self::Uncertain,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Preparing => "preparing",
            Self::InFlight => "in_flight",
            Self::Submitted => "submitted",
            Self::Uncertain => "uncertain",
            Self::Failed => "failed",
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerState {
    #[default]
    NotSubmitted,
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Unknown,
}

impl SchedulerState {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "not_submitted" => Self::NotSubmitted,
            "pending" => Self::Pending,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "timed_out" => Self::TimedOut,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }

    pub fn from_normalized(raw: &str) -> Self {
        match raw.trim().to_ascii_uppercase().as_str() {
            "PENDING" => Self::Pending,
            "RUNNING" => Self::Running,
            "COMPLETED" | "SUCCEEDED" => Self::Succeeded,
            "FAILED" => Self::Failed,
            "CANCELLED" | "CANCELED" => Self::Cancelled,
            "TIMEOUT" | "TIMED_OUT" => Self::TimedOut,
            "NOT_SUBMITTED" => Self::NotSubmitted,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotSubmitted => "not_submitted",
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DaliugeState {
    #[default]
    NotCreated,
    Pristine,
    Building,
    Deploying,
    Running,
    Finished,
    Cancelled,
    Failed,
    Unknown,
    Unreachable,
}

impl DaliugeState {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "not_created" => Self::NotCreated,
            "pristine" => Self::Pristine,
            "building" => Self::Building,
            "deploying" => Self::Deploying,
            "running" => Self::Running,
            "finished" => Self::Finished,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed,
            "unknown" => Self::Unknown,
            "unreachable" => Self::Unreachable,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotCreated => "not_created",
            Self::Pristine => "pristine",
            Self::Building => "building",
            Self::Deploying => "deploying",
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Unreachable => "unreachable",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Pristine | Self::Building | Self::Deploying | Self::Running
        )
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OutputState {
    #[default]
    NotStarted,
    Pending,
    Verifying,
    Verified,
    Failed,
    Unknown,
}

impl OutputState {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "not_started" => Self::NotStarted,
            "pending" => Self::Pending,
            "verifying" => Self::Verifying,
            "verified" => Self::Verified,
            "failed" => Self::Failed,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Pending => "pending",
            Self::Verifying => "verifying",
            Self::Verified => "verified",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Succeeded,
    Failed,
    Cancelled,
    Inconsistent,
}

impl TerminalOutcome {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "inconsistent" => Self::Inconsistent,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Inconsistent => "inconsistent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationAction {
    Wait,
    PollScheduler,
    PollDaliuge,
    VerifyOutputs,
    OperatorReview,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ReconciliationMismatch {
    pub code: String,
    pub message: String,
    pub requires_operator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ReconciliationDecision {
    pub status: ExecutionStatus,
    pub terminal_outcome: Option<TerminalOutcome>,
    pub next_action: ReconciliationAction,
    pub mismatch: Option<ReconciliationMismatch>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ExecutionAxes {
    pub control_phase: ControlPhase,
    pub submission: SubmissionState,
    pub scheduler: SchedulerState,
    pub daliuge: DaliugeState,
    pub outputs: OutputState,
    #[serde(default)]
    pub output_verification_required: bool,
}

impl ExecutionAxes {
    pub fn reconcile(&self) -> ReconciliationDecision {
        use DaliugeState as D;
        use ReconciliationAction as A;
        use SchedulerState as S;

        if self.outputs == OutputState::Failed || self.submission == SubmissionState::Failed {
            return terminal(ExecutionStatus::Failed, TerminalOutcome::Failed);
        }

        if self.daliuge == D::Failed {
            let mismatch = self.scheduler.is_active().then(|| ReconciliationMismatch {
                code: "daliuge_failed_scheduler_active".into(),
                message: "the DALiuGE session failed while its scheduler allocation is active"
                    .into(),
                requires_operator: false,
            });
            return ReconciliationDecision {
                status: ExecutionStatus::Failed,
                terminal_outcome: Some(TerminalOutcome::Failed),
                next_action: mismatch.as_ref().map_or(A::None, |_| A::PollScheduler),
                mismatch,
            };
        }

        if matches!(self.scheduler, S::Failed | S::TimedOut) {
            let mismatch = self.daliuge.is_active().then(|| ReconciliationMismatch {
                code: "scheduler_failed_daliuge_active".into(),
                message: "the scheduler job ended unsuccessfully while DALiuGE still reports an active session"
                    .into(),
                requires_operator: false,
            });
            return ReconciliationDecision {
                status: ExecutionStatus::Failed,
                terminal_outcome: Some(TerminalOutcome::Failed),
                next_action: mismatch.as_ref().map_or(A::None, |_| A::PollDaliuge),
                mismatch,
            };
        }

        if self.scheduler == S::Succeeded && self.daliuge.is_active() {
            return ReconciliationDecision {
                status: ExecutionStatus::Failed,
                terminal_outcome: Some(TerminalOutcome::Inconsistent),
                next_action: A::OperatorReview,
                mismatch: Some(ReconciliationMismatch {
                    code: "scheduler_finished_daliuge_active".into(),
                    message: "the scheduler allocation finished while DALiuGE still reports an active session"
                        .into(),
                    requires_operator: true,
                }),
            };
        }

        if self.scheduler == S::Cancelled || self.daliuge == D::Cancelled {
            let other_active = self.scheduler.is_active() || self.daliuge.is_active();
            return ReconciliationDecision {
                status: ExecutionStatus::Cancelled,
                terminal_outcome: Some(if other_active {
                    TerminalOutcome::Inconsistent
                } else {
                    TerminalOutcome::Cancelled
                }),
                next_action: if other_active {
                    A::OperatorReview
                } else {
                    A::None
                },
                mismatch: other_active.then(|| ReconciliationMismatch {
                    code: "partial_cancellation".into(),
                    message:
                        "one external execution layer is cancelled while another remains active"
                            .into(),
                    requires_operator: true,
                }),
            };
        }

        let external_complete =
            self.daliuge == D::Finished && matches!(self.scheduler, S::Succeeded | S::NotSubmitted);
        if external_complete {
            if self.output_verification_required && self.outputs != OutputState::Verified {
                return ReconciliationDecision {
                    status: ExecutionStatus::Running,
                    terminal_outcome: None,
                    next_action: A::VerifyOutputs,
                    mismatch: None,
                };
            }
            return terminal(ExecutionStatus::Completed, TerminalOutcome::Succeeded);
        }

        if self.submission == SubmissionState::Uncertain {
            return ReconciliationDecision {
                status: ExecutionStatus::AwaitingScheduler,
                terminal_outcome: None,
                next_action: A::PollScheduler,
                mismatch: Some(ReconciliationMismatch {
                    code: "submission_outcome_uncertain".into(),
                    message: "submission may have succeeded but its response was not observed"
                        .into(),
                    requires_operator: false,
                }),
            };
        }

        if self.scheduler == S::Pending {
            return active(ExecutionStatus::AwaitingScheduler, A::PollScheduler);
        }
        if self.scheduler == S::Running || self.daliuge.is_active() {
            return active(ExecutionStatus::Running, A::PollDaliuge);
        }
        if self.scheduler == S::Succeeded
            && matches!(self.daliuge, D::Unknown | D::Unreachable | D::NotCreated)
        {
            return ReconciliationDecision {
                status: ExecutionStatus::AwaitingScheduler,
                terminal_outcome: None,
                next_action: A::PollDaliuge,
                mismatch: Some(ReconciliationMismatch {
                    code: "scheduler_finished_daliuge_unconfirmed".into(),
                    message: "the scheduler job finished but DALiuGE completion is not confirmed"
                        .into(),
                    requires_operator: false,
                }),
            };
        }
        if matches!(self.scheduler, S::Unknown) {
            return active(ExecutionStatus::AwaitingScheduler, A::PollScheduler);
        }

        active(ExecutionStatus::Pending, A::Wait)
    }
}

fn active(status: ExecutionStatus, next_action: ReconciliationAction) -> ReconciliationDecision {
    ReconciliationDecision {
        status,
        terminal_outcome: None,
        next_action,
        mismatch: None,
    }
}

fn terminal(status: ExecutionStatus, outcome: TerminalOutcome) -> ReconciliationDecision {
    ReconciliationDecision {
        status,
        terminal_outcome: Some(outcome),
        next_action: ReconciliationAction::None,
        mismatch: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_execution_completes_without_scheduler_job() {
        let axes = ExecutionAxes {
            submission: SubmissionState::Submitted,
            daliuge: DaliugeState::Finished,
            ..Default::default()
        };
        let decision = axes.reconcile();
        assert_eq!(decision.status, ExecutionStatus::Completed);
        assert_eq!(decision.terminal_outcome, Some(TerminalOutcome::Succeeded));
    }

    #[test]
    fn uncertain_submission_is_reconciled_before_retry() {
        let axes = ExecutionAxes {
            submission: SubmissionState::Uncertain,
            scheduler: SchedulerState::Unknown,
            ..Default::default()
        };
        let decision = axes.reconcile();
        assert_eq!(decision.next_action, ReconciliationAction::PollScheduler);
        assert_eq!(
            decision.mismatch.unwrap().code,
            "submission_outcome_uncertain"
        );
    }

    #[test]
    fn finished_allocation_with_active_session_requires_review() {
        let axes = ExecutionAxes {
            scheduler: SchedulerState::Succeeded,
            daliuge: DaliugeState::Running,
            ..Default::default()
        };
        let decision = axes.reconcile();
        assert_eq!(decision.status, ExecutionStatus::Failed);
        assert_eq!(
            decision.terminal_outcome,
            Some(TerminalOutcome::Inconsistent)
        );
        assert!(decision.mismatch.unwrap().requires_operator);
    }

    #[test]
    fn daliuge_failure_preserves_active_scheduler_mismatch() {
        let axes = ExecutionAxes {
            scheduler: SchedulerState::Running,
            daliuge: DaliugeState::Failed,
            ..Default::default()
        };
        let decision = axes.reconcile();
        assert_eq!(decision.status, ExecutionStatus::Failed);
        assert_eq!(
            decision.mismatch.unwrap().code,
            "daliuge_failed_scheduler_active"
        );
    }
}

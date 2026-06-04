use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

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
                    | (ExecutionStatus::Retrying, ExecutionStatus::Running)
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
    fn retry_count_increments_on_recovery_edges() {
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
        assert_eq!(st.retry_count, 2);
    }
}

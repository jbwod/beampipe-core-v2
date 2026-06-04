use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Why a scheduler tick skipped or blocked admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    Disabled,
    QueueFull,
    TapUnreachable,
    InFlightCap,
    ProjectInFlightCap,
    RateLimited,
    ThresholdNotMet,
    MaxBatchesPerTick,
    SourcesSkippedNotReady,
    NoPendingSources,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::QueueFull => "queue_full",
            Self::TapUnreachable => "tap_unreachable",
            Self::InFlightCap => "in_flight_cap",
            Self::ProjectInFlightCap => "project_in_flight_cap",
            Self::RateLimited => "rate_limited",
            Self::ThresholdNotMet => "threshold_not_met",
            Self::MaxBatchesPerTick => "max_batches_per_tick",
            Self::SourcesSkippedNotReady => "sources_skipped_not_ready",
            Self::NoPendingSources => "no_pending_sources",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    Admit,
    Skip(SkipReason),
}

/// Aggregated outcome of a scheduler tick for structured telemetry.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SchedulerTickResult {
    pub project_module: String,
    pub total_sources: u64,
    pub total_jobs: u64,
    pub batches_this_tick: u64,
    #[serde(serialize_with = "serialize_reason_counts")]
    pub reason_counts: BTreeMap<SkipReason, u32>,
    pub skipped_due_to_queue_full: bool,
    pub skipped_due_to_tap_unreachable: bool,
    pub skipped_due_to_max_batches_per_tick: bool,
    pub tap_unreachable: Vec<String>,
}

fn serialize_reason_counts<S>(
    counts: &BTreeMap<SkipReason, u32>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(counts.len()))?;
    for (reason, count) in counts {
        map.serialize_entry(reason.as_str(), count)?;
    }
    map.end()
}

impl SchedulerTickResult {
    pub fn new(project_module: impl Into<String>) -> Self {
        Self {
            project_module: project_module.into(),
            ..Default::default()
        }
    }

    pub fn bump(&mut self, reason: SkipReason) {
        *self.reason_counts.entry(reason).or_insert(0) += 1;
        match reason {
            SkipReason::QueueFull => self.skipped_due_to_queue_full = true,
            SkipReason::TapUnreachable => self.skipped_due_to_tap_unreachable = true,
            SkipReason::MaxBatchesPerTick => self.skipped_due_to_max_batches_per_tick = true,
            _ => {}
        }
    }
}

/// Returns true when `current` is strictly below `cap`.
pub fn can_admit_by_in_flight(current: i64, cap: i64) -> bool {
    current < cap
}

/// Pass-through rate budget (matches Python stub).
pub fn discovery_admission_budget(desired_sources: i64) -> i64 {
    desired_sources.max(0)
}

/// Pass-through rate budget (matches Python stub).
pub fn execute_admission_budget(desired_runs: i64) -> i64 {
    desired_runs.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_admit_when_below_cap() {
        assert!(can_admit_by_in_flight(0, 4));
        assert!(can_admit_by_in_flight(3, 4));
        assert!(!can_admit_by_in_flight(4, 4));
        assert!(!can_admit_by_in_flight(5, 4));
    }

    #[test]
    fn admission_budget_passes_through() {
        assert_eq!(discovery_admission_budget(100), 100);
        assert_eq!(discovery_admission_budget(-1), 0);
        assert_eq!(execute_admission_budget(5), 5);
    }

    #[test]
    fn reason_counts_bump_and_flags() {
        let mut result = SchedulerTickResult::new("wallaby_hires");
        result.bump(SkipReason::QueueFull);
        result.bump(SkipReason::QueueFull);
        result.bump(SkipReason::InFlightCap);
        assert_eq!(result.reason_counts.get(&SkipReason::QueueFull), Some(&2));
        assert!(result.skipped_due_to_queue_full);
        assert!(!result.skipped_due_to_tap_unreachable);
    }

    #[test]
    fn awaiting_scheduler_excluded_from_in_flight_logic() {
        // In-flight counts exclude awaiting_scheduler at the SQL layer;
        // admission helper only compares numeric caps.
        assert!(can_admit_by_in_flight(1, 2));
        assert!(!can_admit_by_in_flight(2, 2));
    }
}

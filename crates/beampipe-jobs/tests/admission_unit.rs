//! Unit tests for admission cap helpers used by scheduler ticks.

use beampipe_domain::{
    can_admit_by_in_flight, discovery_admission_budget, SchedulerTickResult, SkipReason,
};

#[test]
fn discovery_budget_matches_python_stub() {
    assert_eq!(discovery_admission_budget(200), 200);
    assert_eq!(discovery_admission_budget(0), 0);
}

#[test]
fn in_flight_cap_blocks_at_limit() {
    assert!(!can_admit_by_in_flight(4, 4));
    assert!(can_admit_by_in_flight(3, 4));
}

#[test]
fn scheduler_tick_result_aggregates_reasons() {
    let mut tick = SchedulerTickResult::new("wallaby_hires");
    tick.bump(SkipReason::MaxBatchesPerTick);
    tick.bump(SkipReason::QueueFull);
    assert!(tick.skipped_due_to_max_batches_per_tick);
    assert_eq!(tick.reason_counts.len(), 2);
}

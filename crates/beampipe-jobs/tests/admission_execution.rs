use beampipe_domain::{can_admit_by_in_flight, execute_admission_budget, SkipReason};

#[test]
fn execute_admission_budget_passes_through() {
    assert_eq!(execute_admission_budget(5), 5);
    assert_eq!(execute_admission_budget(-1), 0);
}

#[test]
fn project_in_flight_cap_semantics() {
    assert!(!can_admit_by_in_flight(5, 5));
    assert!(can_admit_by_in_flight(4, 5));
    assert_eq!(
        SkipReason::ProjectInFlightCap.as_str(),
        "project_in_flight_cap"
    );
}

//! Golden tests ported from beampipe-core/tests/test_dim_status_classification.py

use beampipe_domain::ExecutionStatus;
use beampipe_orchestration::{classify_dim_session_status, dim_graph_status_error_uids};
use serde_json::json;

#[test]
fn classify_dim_session_status_ints() {
    assert_eq!(
        classify_dim_session_status(&json!(4)),
        ExecutionStatus::Completed
    );
    assert_eq!(
        classify_dim_session_status(&json!(5)),
        ExecutionStatus::Cancelled
    );
    assert_eq!(
        classify_dim_session_status(&json!(6)),
        ExecutionStatus::Failed
    );
    assert_eq!(
        classify_dim_session_status(&json!(3)),
        ExecutionStatus::Running
    );
    assert_eq!(
        classify_dim_session_status(&json!(2)),
        ExecutionStatus::Running
    );
}

#[test]
fn classify_dim_session_status_strings() {
    assert_eq!(
        classify_dim_session_status(&json!("FINISHED")),
        ExecutionStatus::Completed
    );
    assert_eq!(
        classify_dim_session_status(&json!("Finished")),
        ExecutionStatus::Completed
    );
    assert_eq!(
        classify_dim_session_status(&json!("CANCELLED")),
        ExecutionStatus::Cancelled
    );
    assert_eq!(
        classify_dim_session_status(&json!("FAILED")),
        ExecutionStatus::Failed
    );
    assert_eq!(
        classify_dim_session_status(&json!("ERROR")),
        ExecutionStatus::Failed
    );
    assert_eq!(
        classify_dim_session_status(&json!("Running")),
        ExecutionStatus::Running
    );
}

#[test]
fn classify_dim_session_status_dict_status_key() {
    assert_eq!(
        classify_dim_session_status(&json!({"status": 4})),
        ExecutionStatus::Completed
    );
    assert_eq!(
        classify_dim_session_status(&json!({"status": "FINISHED"})),
        ExecutionStatus::Completed
    );
    assert_eq!(
        classify_dim_session_status(&json!({"status": 5})),
        ExecutionStatus::Cancelled
    );
    assert_eq!(
        classify_dim_session_status(&json!({"status": 6})),
        ExecutionStatus::Failed
    );
}

#[test]
fn classify_dim_session_status_per_node_map() {
    assert_eq!(
        classify_dim_session_status(&json!({
            "dlg-nm1:8000:5555:6666": 4,
            "dlg-nm2:8000:5555:6666": 4,
        })),
        ExecutionStatus::Completed
    );
    assert_eq!(
        classify_dim_session_status(&json!({
            "dlg-nm1:8000:5555:6666": 4,
            "dlg-nm2:8000:5555:6666": 6,
        })),
        ExecutionStatus::Failed
    );
    assert_eq!(
        classify_dim_session_status(&json!({
            "dlg-nm1:8000:5555:6666": 5,
            "dlg-nm2:8000:5555:6666": 5,
        })),
        ExecutionStatus::Cancelled
    );
    assert_eq!(
        classify_dim_session_status(&json!({
            "dlg-nm1:8000:5555:6666": 2,
            "dlg-nm2:8000:5555:6666": 4,
        })),
        ExecutionStatus::Running
    );
}

#[test]
fn dim_graph_status_error_uids_detects_drop_error_int() {
    let graph = json!({
        "drop_a": 3,
        "drop_b": 2,
    });
    let errors = dim_graph_status_error_uids(&graph);
    assert_eq!(errors, vec!["drop_a".to_string()]);
}

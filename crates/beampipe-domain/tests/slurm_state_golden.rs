//! Slurm state normalization golden vectors (Python slurm_client/state.py parity).

use beampipe_domain::slurm::{normalize_state, parse_sacct_exit_code, state_rank};

#[test]
fn normalize_slurm_states() {
    assert_eq!(normalize_state("COMPLETED"), "COMPLETED");
    assert_eq!(normalize_state("COMPLETING"), "RUNNING");
    assert_eq!(normalize_state("CANCELLED+"), "CANCELLED");
    assert_eq!(normalize_state("FAILED"), "FAILED");
    assert_eq!(normalize_state("OUT_OF_MEMORY"), "FAILED");
    assert_eq!(normalize_state("TIMEOUT"), "TIMEOUT");
    assert_eq!(normalize_state("RUNNING"), "RUNNING");
    assert_eq!(normalize_state("CONFIGURING"), "RUNNING");
    assert_eq!(normalize_state("PENDING"), "PENDING");
    assert_eq!(normalize_state("SUSPENDED"), "PENDING");
    assert_eq!(normalize_state(""), "UNKNOWN");
    assert_eq!(normalize_state("WEIRD"), "UNKNOWN");
    assert_eq!(normalize_state("PENDING+REQUEUE_HOLD"), "PENDING");
}

#[test]
fn state_rank_golden() {
    assert!(state_rank("UNKNOWN") < state_rank("PENDING"));
    assert!(state_rank("FAILED") > state_rank("TIMEOUT"));
}

#[test]
fn sacct_exit_code_golden() {
    assert_eq!(parse_sacct_exit_code("0:0"), Some(0));
    assert_eq!(parse_sacct_exit_code("1:0"), Some(1));
}

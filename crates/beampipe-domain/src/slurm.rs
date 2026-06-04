use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

pub const SCHEDULER_JOB_ID_MAX_LEN: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SlurmSchedulerJobId {
    pub session_id: String,
    pub slurm_job_id: String,
    pub session_dir: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SlurmJobIdError {
    #[error("scheduler_job_id exceeds max length 512: len={0}")]
    TooLong(usize),
}

pub fn compose_scheduler_job_id(
    session_id: &str,
    slurm_job_id: &str,
    session_dir: Option<&str>,
) -> Result<String, SlurmJobIdError> {
    let mut body = format!("{session_id}:{slurm_job_id}");
    if let Some(dir) = session_dir.filter(|v| !v.trim().is_empty()) {
        body.push('|');
        body.push_str(dir);
    }
    if body.len() > SCHEDULER_JOB_ID_MAX_LEN {
        return Err(SlurmJobIdError::TooLong(body.len()));
    }
    Ok(body)
}

pub fn parse_scheduler_job_id(raw: &str) -> SlurmSchedulerJobId {
    let raw = raw.trim();
    let (head, session_dir) = raw
        .split_once('|')
        .map(|(h, d)| (h, Some(d.trim().to_string()).filter(|v| !v.is_empty())))
        .unwrap_or((raw, None));
    let (session_id, slurm_job_id) = head
        .split_once(':')
        .map(|(s, j)| (s.to_string(), j.trim().to_string()))
        .unwrap_or(("".to_string(), head.trim().to_string()));
    SlurmSchedulerJobId {
        session_id,
        slurm_job_id,
        session_dir,
    }
}

/// Python `state_rank` parity (uppercase normalized states).
pub fn state_rank(normalized: &str) -> i32 {
    match normalized {
        "UNKNOWN" => 0,
        "PENDING" => 1,
        "RUNNING" => 2,
        "COMPLETED" => 3,
        "CANCELLED" => 4,
        "TIMEOUT" => 5,
        "FAILED" => 6,
        _ => 0,
    }
}

fn normalize_one_token(u: &str) -> &'static str {
    let u = u.trim();
    if u.is_empty() {
        return "UNKNOWN";
    }
    if u.starts_with("CANCELLED") {
        return "CANCELLED";
    }
    match u {
        "COMPLETED" => "COMPLETED",
        "PENDING" => "PENDING",
        "RUNNING" => "RUNNING",
        "BOOT_FAIL" | "DEADLINE" | "FAILED" | "LAUNCH_FAILED" | "NODE_FAIL" | "OUT_OF_MEMORY"
        | "PREEMPTED" | "RECONFIG_FAIL" => "FAILED",
        "TIMEOUT" => "TIMEOUT",
        "CANCELLED" | "REVOKED" => "CANCELLED",
        "COMPLETING" | "CONFIGURING" | "EXPEDITING" | "POWER_UP_NODE" | "REQUEUED"
        | "REQUEUE_FED" | "RESIZING" | "SIGNALING" | "STAGE_OUT" | "UPDATE_DB" => "RUNNING",
        "RESV_DEL_HOLD" | "REQUEUE_HOLD" | "SPECIAL_EXIT" | "STOPPED" | "SUSPENDED" => "PENDING",
        _ => "UNKNOWN",
    }
}

/// Normalize Slurm state strings to v1 uppercase tokens (Python `slurm_client/state.py` parity).
pub fn normalize_state(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return "UNKNOWN".into();
    }
    let u = raw.to_uppercase();
    if u.contains('+') {
        let parts: Vec<&str> = u
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() > 1 {
            return parts
                .iter()
                .map(|p| normalize_one_token(p))
                .max_by_key(|n| state_rank(n))
                .unwrap_or("UNKNOWN")
                .to_string();
        }
    }
    let head = u.split_whitespace().next().unwrap_or(&u);
    normalize_one_token(head).to_string()
}

pub fn parse_sacct_exit_code(raw: &str) -> Option<i32> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let head = raw.split(':').next().unwrap_or(raw).trim();
    head.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_rank_orders_terminal_states() {
        assert!(state_rank("COMPLETED") < state_rank("TIMEOUT"));
        assert!(state_rank("TIMEOUT") < state_rank("FAILED"));
        assert!(state_rank("RUNNING") < state_rank("COMPLETED"));
    }

    #[test]
    fn compound_state_uses_max_rank() {
        assert_eq!(normalize_state("PENDING+REQUEUE_HOLD"), "PENDING");
        assert_eq!(normalize_state("RUNNING+COMPLETING"), "RUNNING");
        assert_eq!(normalize_state("PENDING+RUNNING"), "RUNNING");
    }

    #[test]
    fn timeout_is_distinct() {
        assert_eq!(normalize_state("TIMEOUT"), "TIMEOUT");
    }

    #[test]
    fn parse_sacct_exit_code_handles_colon_form() {
        assert_eq!(parse_sacct_exit_code("1:0"), Some(1));
        assert_eq!(parse_sacct_exit_code(""), None);
    }

    #[test]
    fn scheduler_id_round_trip() {
        let raw = compose_scheduler_job_id("session", "123", Some("/tmp/session")).unwrap();
        assert_eq!(raw, "session:123|/tmp/session");
        assert_eq!(
            parse_scheduler_job_id(&raw),
            SlurmSchedulerJobId {
                session_id: "session".into(),
                slurm_job_id: "123".into(),
                session_dir: Some("/tmp/session".into())
            }
        );
    }

    #[test]
    fn bare_job_id_is_supported() {
        let parsed = parse_scheduler_job_id("12345");
        assert_eq!(parsed.session_id, "");
        assert_eq!(parsed.slurm_job_id, "12345");
        assert_eq!(parsed.session_dir, None);
    }

    #[test]
    fn too_long_scheduler_id_errors() {
        let dir = "x".repeat(600);
        assert!(matches!(
            compose_scheduler_job_id("s", "j", Some(&dir)),
            Err(SlurmJobIdError::TooLong(_))
        ));
    }
}

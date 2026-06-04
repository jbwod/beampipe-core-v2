//! Parse batched `squeue` / `sacct` output for Slurm job state polling.

use beampipe_domain::slurm::{normalize_state, parse_sacct_exit_code, state_rank};
use std::collections::HashMap;

pub const BATCH_JOB_ID_CHUNK: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlurmJobPollResult {
    pub raw_state: String,
    pub normalized_state: String,
    pub source: &'static str,
    pub exit_code: Option<i32>,
    /// Full observation line (`state|reason` or sacct row).
    pub raw_line: Option<String>,
}

#[derive(Debug, Clone)]
struct SqueueRow {
    state: String,
    reason: Option<String>,
}

/// Parse `squeue -h -o %i|%T|%R` batch output into poll results.
pub fn parse_squeue_batch(stdout: &str) -> HashMap<String, SlurmJobPollResult> {
    parse_squeue_rows(stdout)
        .into_iter()
        .map(|(id, row)| {
            (
                id,
                SlurmJobPollResult {
                    raw_state: row.state.clone(),
                    normalized_state: normalize_state(&row.state),
                    source: "squeue",
                    exit_code: None,
                    raw_line: Some(format_squeue_raw(&row)),
                },
            )
        })
        .collect()
}

fn parse_squeue_rows(stdout: &str) -> HashMap<String, SqueueRow> {
    let mut map = HashMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if parts.is_empty() {
            continue;
        }
        let (job_id, state, reason) = match parts.as_slice() {
            [id, state] => (*id, *state, None),
            [id, state, reason] => (*id, *state, Some(*reason)),
            [id, rest @ ..] if !rest.is_empty() => (*id, rest[0], rest.get(1).copied()),
            _ => continue,
        };
        if job_id.is_empty() || state.is_empty() {
            continue;
        }
        map.insert(
            job_id.to_string(),
            SqueueRow {
                state: state.to_string(),
                reason: reason.map(str::to_string),
            },
        );
    }
    map
}

fn sacct_base_job_id(job_id: &str) -> &str {
    job_id.split('.').next().unwrap_or(job_id)
}

fn format_squeue_raw(row: &SqueueRow) -> String {
    match &row.reason {
        Some(r) if !r.is_empty() => format!("{}|{}", row.state, r),
        _ => row.state.clone(),
    }
}

/// Parse `sacct -j … --format=JobID,State,ExitCode -P -n` and fold step rows by base job id.
pub fn parse_sacct_batch(stdout: &str) -> HashMap<String, SlurmJobPollResult> {
    let mut by_base: HashMap<String, (i32, SlurmJobPollResult)> = HashMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        let (job_id, state_str, exit_code_str) = match parts.as_slice() {
            [id, state] => (*id, *state, ""),
            [id, state, exit] => (*id, *state, *exit),
            _ => continue,
        };
        let state_token = state_str
            .split_whitespace()
            .next()
            .unwrap_or(state_str)
            .trim();
        if job_id.is_empty() || state_token.is_empty() {
            continue;
        }
        let normalized = normalize_state(state_token);
        let rank = state_rank(&normalized);
        let base = sacct_base_job_id(job_id).to_string();
        let candidate = SlurmJobPollResult {
            raw_state: state_token.to_string(),
            normalized_state: normalized,
            source: "sacct",
            exit_code: parse_sacct_exit_code(exit_code_str),
            raw_line: Some(line.to_string()),
        };
        match by_base.get(&base) {
            Some((best_rank, _)) if *best_rank >= rank => {}
            _ => {
                by_base.insert(base, (rank, candidate));
            }
        }
    }
    by_base
        .into_iter()
        .map(|(id, (_, result))| (id, result))
        .collect()
}

/// Merge squeue hits with sacct fallbacks for jobs missing from squeue.
pub fn merge_squeue_sacct_batch(
    job_ids: &[String],
    squeue: &HashMap<String, SlurmJobPollResult>,
    sacct: &HashMap<String, SlurmJobPollResult>,
) -> HashMap<String, SlurmJobPollResult> {
    let mut out = HashMap::new();
    for id in job_ids {
        if let Some(result) = squeue.get(id) {
            out.insert(id.clone(), result.clone());
        } else if let Some(result) = sacct.get(id) {
            out.insert(id.clone(), result.clone());
        } else {
            out.insert(
                id.clone(),
                SlurmJobPollResult {
                    raw_state: String::new(),
                    normalized_state: "UNKNOWN".into(),
                    source: "none",
                    exit_code: None,
                    raw_line: None,
                },
            );
        }
    }
    out
}

pub fn chunk_job_ids(job_ids: &[String]) -> Vec<Vec<String>> {
    if job_ids.is_empty() {
        return Vec::new();
    }
    job_ids
        .chunks(BATCH_JOB_ID_CHUNK)
        .map(|c| c.to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_squeue_batch_multiple_jobs() {
        let stdout = "1001|RUNNING|None\n1002|PENDING|Resources\n";
        let map = parse_squeue_batch(stdout);
        assert_eq!(
            map.get("1001").map(|r| r.normalized_state.as_str()),
            Some("RUNNING")
        );
        assert_eq!(
            map.get("1002").map(|r| r.raw_line.as_deref()),
            Some(Some("PENDING|Resources"))
        );
    }

    #[test]
    fn parse_sacct_picks_highest_rank_step() {
        let stdout = "42|RUNNING|0:0\n42.batch|COMPLETED|0:0\n42.extern|COMPLETED|0:0\n";
        let map = parse_sacct_batch(stdout);
        let r = map.get("42").expect("42");
        assert_eq!(r.normalized_state, "COMPLETED");
        assert_eq!(r.exit_code, Some(0));
    }

    #[test]
    fn parse_sacct_exit_code_from_row() {
        let stdout = "99|FAILED|1:0\n";
        let map = parse_sacct_batch(stdout);
        assert_eq!(map["99"].exit_code, Some(1));
    }

    #[test]
    fn merge_prefers_squeue_over_sacct() {
        let ids = vec!["1".into()];
        let mut squeue = HashMap::new();
        squeue.insert(
            "1".into(),
            SlurmJobPollResult {
                raw_state: "RUNNING".into(),
                normalized_state: "RUNNING".into(),
                source: "squeue",
                exit_code: None,
                raw_line: None,
            },
        );
        let mut sacct = HashMap::new();
        sacct.insert(
            "1".into(),
            SlurmJobPollResult {
                raw_state: "COMPLETED".into(),
                normalized_state: "COMPLETED".into(),
                source: "sacct",
                exit_code: None,
                raw_line: None,
            },
        );
        let merged = merge_squeue_sacct_batch(&ids, &squeue, &sacct);
        assert_eq!(merged["1"].source, "squeue");
        assert_eq!(merged["1"].normalized_state, "RUNNING");
    }

    #[test]
    fn unknown_when_missing_everywhere() {
        let ids = vec!["9".into()];
        let merged = merge_squeue_sacct_batch(&ids, &HashMap::new(), &HashMap::new());
        assert_eq!(merged["9"].normalized_state, "UNKNOWN");
        assert_eq!(merged["9"].source, "none");
    }
}

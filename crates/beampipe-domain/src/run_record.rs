use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

pub const BEAMPIPE_RUN_RECORD_KEY: &str = "beampipe_run_record";
const RAW_LINE_MAX: usize = 512;

fn now_iso_z() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn trim_raw(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.is_empty() {
        None
    } else if s.len() <= RAW_LINE_MAX {
        Some(s.to_string())
    } else {
        Some(format!("{}...", &s[..RAW_LINE_MAX]))
    }
}

fn object(v: Option<Value>) -> Map<String, Value> {
    match v {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn rr_mut(base: &mut Map<String, Value>) -> Map<String, Value> {
    object(base.remove(BEAMPIPE_RUN_RECORD_KEY))
}

pub fn merge_execution_request_into_run_record(
    existing: Option<Value>,
    sources: &[Value],
) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let captured: Vec<Value> = sources
        .iter()
        .filter_map(|spec| {
            let obj = spec.as_object()?;
            let sid = obj.get("source_identifier")?.as_str()?;
            let mut entry = Map::new();
            entry.insert("source_identifier".into(), Value::String(sid.to_string()));
            if let Some(Value::Array(sbids)) = obj.get("sbids") {
                if !sbids.is_empty() {
                    entry.insert(
                        "sbids".into(),
                        Value::Array(
                            sbids
                                .iter()
                                .map(|s| Value::String(s.to_string().trim_matches('"').to_string()))
                                .collect(),
                        ),
                    );
                }
            }
            Some(Value::Object(entry))
        })
        .collect();
    let ids: Vec<Value> = captured
        .iter()
        .filter_map(|v| v.get("source_identifier").cloned())
        .collect();
    rr.insert(
        "requested_sources".into(),
        json!({
            "count": captured.len(),
            "source_identifiers": ids,
            "sources": captured,
        }),
    );
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

pub fn merge_slurm_submit_into_manifest(
    existing: Option<Value>,
    session_id: &str,
    slurm_job_id: &str,
    composite_scheduler_job_id: &str,
    login_node: Option<&str>,
    remote_user: Option<&str>,
) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let mut slurm = object(rr.remove("slurm"));
    slurm.remove("observations");
    slurm.insert("session_id".into(), json!(session_id));
    slurm.insert("slurm_job_id".into(), json!(slurm_job_id));
    slurm.insert(
        "composite_scheduler_job_id".into(),
        json!(composite_scheduler_job_id),
    );
    slurm.insert("submitted_at".into(), json!(now_iso_z()));
    if let Some(v) = login_node.filter(|v| !v.trim().is_empty()) {
        slurm.insert("login_node".into(), json!(v.trim()));
    }
    if let Some(v) = remote_user.filter(|v| !v.trim().is_empty()) {
        slurm.insert("remote_user".into(), json!(v.trim()));
    }
    rr.insert("slurm".into(), Value::Object(slurm));
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

pub fn merge_scheduler_timeout_into_manifest(existing: Option<Value>, error: &str) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let mut scheduler = object(rr.remove("scheduler"));
    scheduler.insert(
        "slurm_completion".into(),
        json!({
            "reason": "poll_timeout",
            "error": trim_raw(Some(error)).unwrap_or_default(),
            "observed_at": now_iso_z(),
        }),
    );
    rr.insert("scheduler".into(), Value::Object(scheduler));
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

#[derive(Debug, Clone, Default)]
pub struct SlurmPollManifestOpts<'a> {
    pub exit_code: Option<i32>,
    pub remote_session_dir: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub diagnostics: Option<Value>,
}

fn merge_slurm_paths(slurm: &mut Map<String, Value>, session_dir: &str) {
    let mut paths = object(slurm.remove("paths"));
    paths.insert("session_dir".into(), json!(session_dir.trim()));
    slurm.insert("paths".into(), Value::Object(paths));
}

#[allow(clippy::too_many_arguments)]
pub fn merge_slurm_poll_into_manifest(
    existing: Option<Value>,
    composite_scheduler_job_id: &str,
    slurm_job_id: &str,
    state: &str,
    source: &str,
    raw_line: Option<&str>,
    record_terminal: bool,
    terminal_ledger_status: Option<&str>,
    opts: SlurmPollManifestOpts<'_>,
) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let mut slurm = object(rr.remove("slurm"));
    slurm.remove("observations");
    slurm.insert("slurm_job_id".into(), json!(slurm_job_id));
    slurm.insert(
        "composite_scheduler_job_id".into(),
        json!(composite_scheduler_job_id),
    );
    if let Some(dir) = opts.remote_session_dir.filter(|d| !d.trim().is_empty()) {
        merge_slurm_paths(&mut slurm, dir);
    }
    let mut obs = Map::new();
    obs.insert("state".into(), json!(state));
    obs.insert("source".into(), json!(source));
    obs.insert("observed_at".into(), json!(now_iso_z()));
    if let Some(code) = opts.exit_code {
        obs.insert("exit_code".into(), json!(code));
    }
    if let Some(raw) = trim_raw(raw_line) {
        obs.insert("raw".into(), json!(raw));
    }
    slurm.insert("last_observation".into(), Value::Object(obs.clone()));
    if record_terminal {
        if let Some(status) = terminal_ledger_status {
            let comp = composite_scheduler_job_id;
            let frozen = slurm
                .get("terminal")
                .and_then(Value::as_object)
                .is_some_and(|t| {
                    t.get("ledger_status").and_then(Value::as_str) == Some(status)
                        && t.get("composite_scheduler_job_id").and_then(Value::as_str) == Some(comp)
                });
            if !frozen {
                let mut term = obs.clone();
                term.insert("ledger_status".into(), json!(status));
                term.insert(
                    "composite_scheduler_job_id".into(),
                    json!(composite_scheduler_job_id),
                );
                term.insert("slurm_job_id_ref".into(), json!(slurm_job_id));
                if let Some(reason) = opts.reason.filter(|r| !r.is_empty()) {
                    term.insert("reason".into(), json!(reason));
                }
                if let Some(diag) = opts.diagnostics.filter(|d| d.is_object()) {
                    term.insert("diagnostics".into(), diag);
                }
                slurm.insert("terminal".into(), Value::Object(term));
            }
        }
    }
    rr.insert("slurm".into(), Value::Object(slurm));
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

pub fn merge_dim_deploy_into_manifest(
    existing: Option<Value>,
    session_id: &str,
    dim_rest_base: &str,
    verify_ssl: bool,
    operator_urls: Option<std::collections::HashMap<String, String>>,
) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let mut dim = object(rr.remove("dim"));
    dim.remove("observations");
    dim.insert("session_id".into(), json!(session_id));
    let mut deploy = object(dim.remove("deploy"));
    deploy.insert("deployed_at".into(), json!(now_iso_z()));
    deploy.insert(
        "dim_rest_base".into(),
        json!(dim_rest_base.trim_end_matches('/')),
    );
    deploy.insert("verify_ssl".into(), json!(verify_ssl));
    if let Some(urls) = operator_urls {
        let filtered: Map<String, Value> = urls
            .into_iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (k, json!(v)))
            .collect();
        if !filtered.is_empty() {
            deploy.insert("operator_urls".into(), Value::Object(filtered));
        }
    }
    dim.insert("deploy".into(), Value::Object(deploy));
    rr.insert("dim".into(), Value::Object(dim));
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

pub fn merge_dim_poll_into_manifest(
    existing: Option<Value>,
    session_id: &str,
    session_state: &str,
    record_terminal: bool,
    terminal_ledger_status: Option<&str>,
    error: Option<&str>,
    error_drops_count: Option<i64>,
) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let mut dim = object(rr.remove("dim"));
    dim.remove("observations");
    dim.insert("session_id".into(), json!(session_id));
    let mut obs = Map::new();
    obs.insert("session_state".into(), json!(session_state));
    obs.insert("observed_at".into(), json!(now_iso_z()));
    dim.insert("last_observation".into(), Value::Object(obs.clone()));
    if record_terminal {
        if let Some(status) = terminal_ledger_status {
            let mut term = obs;
            term.insert("ledger_status".into(), json!(status));
            term.insert("session_id".into(), json!(session_id));
            if let Some(err) = error.and_then(|e| trim_raw(Some(e))) {
                term.insert("error".into(), json!(err));
            }
            if let Some(count) = error_drops_count {
                term.insert("error_drops_count".into(), json!(count));
            }
            dim.insert("terminal".into(), Value::Object(term));
        }
    }
    rr.insert("dim".into(), Value::Object(dim));
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

/// Operator-facing summary of recent scheduler phases from run record.
pub fn summarize_run_record_phases(run_record: Option<&Value>) -> Value {
    let Some(rr) = run_record.and_then(|v| v.as_object()) else {
        return json!({});
    };
    let mut out = Map::new();
    for key in ["slurm", "dim", "scheduler", "slurm_poll", "dim_poll"] {
        if let Some(obj) = rr.get(key).and_then(Value::as_object) {
            let mut phase = Map::new();
            if let Some(t) = obj.get("started_at").or(obj.get("submitted_at")) {
                phase.insert("last_at".into(), t.clone());
            }
            if let Some(s) = obj.get("state").or(obj.get("terminal")) {
                phase.insert("state".into(), s.clone());
            }
            if let Some(r) = obj.get("round") {
                phase.insert("round".into(), r.clone());
            }
            if !phase.is_empty() {
                out.insert(key.into(), Value::Object(phase));
            }
        }
    }
    if let Some(req) = rr.get("requested_sources") {
        out.insert("requested_sources".into(), req.clone());
    }
    Value::Object(out)
}

pub fn extract_beampipe_run_record(workflow_manifest: &Value) -> Option<Value> {
    workflow_manifest
        .get(BEAMPIPE_RUN_RECORD_KEY)
        .cloned()
        .filter(|v| v.is_object())
}

pub fn parse_observed_at(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// Poll round counter for batched `slurm_poll_tick` (stored under `beampipe_run_record.slurm_poll`).
pub fn slurm_poll_round_from_manifest(existing: Option<&Value>) -> i64 {
    existing
        .and_then(|m| m.get(BEAMPIPE_RUN_RECORD_KEY))
        .and_then(|rr| rr.get("slurm_poll"))
        .and_then(|p| p.get("round"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

pub fn merge_slurm_poll_tick_round(existing: Option<Value>, round: i64) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let mut tick = object(rr.remove("slurm_poll"));
    tick.insert("round".into(), json!(round));
    if !tick.contains_key("started_at") {
        tick.insert("started_at".into(), json!(now_iso_z()));
    }
    rr.insert("slurm_poll".into(), Value::Object(tick));
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

/// Poll round counter for batched `dim_poll_tick` (stored under `beampipe_run_record.dim_poll`).
pub fn dim_poll_round_from_manifest(existing: Option<&Value>) -> i64 {
    existing
        .and_then(|m| m.get(BEAMPIPE_RUN_RECORD_KEY))
        .and_then(|rr| rr.get("dim_poll"))
        .and_then(|p| p.get("round"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

pub fn merge_dim_poll_tick_round(existing: Option<Value>, round: i64) -> Value {
    let mut base = object(existing);
    let mut rr = rr_mut(&mut base);
    let mut tick = object(rr.remove("dim_poll"));
    tick.insert("round".into(), json!(round));
    if !tick.contains_key("started_at") {
        tick.insert("started_at".into(), json!(now_iso_z()));
    }
    rr.insert("dim_poll".into(), Value::Object(tick));
    base.insert(BEAMPIPE_RUN_RECORD_KEY.into(), Value::Object(rr));
    Value::Object(base)
}

pub fn dim_logs_url(dim_base: &str, session_id: &str) -> String {
    let base = dim_base.trim_end_matches('/');
    format!("{base}/api/sessions/{session_id}/logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_sources_are_captured() {
        let out = merge_execution_request_into_run_record(
            None,
            &[json!({"source_identifier": "HIPASSJ1318-21", "sbids": ["1"]})],
        );
        assert_eq!(
            out["beampipe_run_record"]["requested_sources"]["source_identifiers"][0],
            "HIPASSJ1318-21"
        );
    }

    #[test]
    fn slurm_poll_terminal_freeze_preserves_first_terminal() {
        let first = merge_slurm_poll_into_manifest(
            None,
            "sid:1|/dir",
            "1",
            "COMPLETED",
            "sacct",
            Some("COMPLETED|0:0"),
            true,
            Some("completed"),
            SlurmPollManifestOpts {
                exit_code: Some(0),
                remote_session_dir: Some("/dir"),
                reason: None,
                diagnostics: None,
            },
        );
        let second = merge_slurm_poll_into_manifest(
            Some(first),
            "sid:1|/dir",
            "1",
            "COMPLETING",
            "sacct",
            Some("COMPLETING"),
            true,
            Some("completed"),
            SlurmPollManifestOpts::default(),
        );
        assert_eq!(
            second["beampipe_run_record"]["slurm"]["terminal"]["state"],
            "COMPLETED"
        );
        assert_eq!(
            second["beampipe_run_record"]["slurm"]["terminal"]["ledger_status"],
            "completed"
        );
    }

    #[test]
    fn slurm_poll_tick_round_is_stored() {
        let out = merge_slurm_poll_tick_round(None, 3);
        assert_eq!(out["beampipe_run_record"]["slurm_poll"]["round"], 3);
        assert!(out["beampipe_run_record"]["slurm_poll"]["started_at"].is_string());
    }

    #[test]
    fn slurm_submit_strips_legacy_observations() {
        let existing = json!({"beampipe_run_record": {"slurm": {"observations": []}}});
        let out = merge_slurm_submit_into_manifest(Some(existing), "s", "1", "s:1", None, None);
        assert!(out["beampipe_run_record"]["slurm"]
            .get("observations")
            .is_none());
        assert_eq!(out["beampipe_run_record"]["slurm"]["slurm_job_id"], "1");
    }
}

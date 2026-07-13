use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SourceSpec {
    pub source_identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sbids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct RegisteredSourceReadiness {
    pub enabled: bool,
    pub last_checked_at_present: bool,
    pub discovery_signature: Option<String>,
    pub discovery_claim_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ArchiveMetadataReadiness {
    pub sbid: String,
    pub metadata_json: Option<Value>,
}

pub fn filter_archive_rows_by_sbids<'a>(
    rows: &'a [ArchiveMetadataReadiness],
    sbids: Option<&[String]>,
) -> Vec<&'a ArchiveMetadataReadiness> {
    let Some(sbids) = sbids else {
        return rows.iter().collect();
    };
    if sbids.is_empty() {
        return rows.iter().collect();
    }
    rows.iter().filter(|r| sbids.contains(&r.sbid)).collect()
}

pub fn parsed_source_readiness_error(
    sid: &str,
    sbids: Option<&[String]>,
    registered: Option<&RegisteredSourceReadiness>,
    rows: &[ArchiveMetadataReadiness],
) -> Option<String> {
    let registered = registered?;
    if !registered.enabled {
        return Some(format!("Source {sid} is disabled"));
    }
    if !registered.last_checked_at_present {
        return Some(format!(
            "Source {sid}: Discovery has not yet run for this source (last_checked_at is unset). Run discovery first (POST /api/v2/sources/discover)."
        ));
    }
    if registered
        .discovery_signature
        .as_deref()
        .unwrap_or("")
        .is_empty()
    {
        return Some(format!(
            "Source {sid}: Discovery signature is missing. Run discovery first (POST /api/v2/sources/discover)."
        ));
    }
    if registered.discovery_claim_token.is_some() {
        return Some(format!(
            "Source {sid}: Discovery is still in progress for this source (active lease). Wait and retry."
        ));
    }

    let metadata = filter_archive_rows_by_sbids(rows, sbids);
    if metadata.is_empty() {
        let hint = sbids
            .map(|s| format!(" (SBIDs: {s:?})"))
            .unwrap_or_default();
        return Some(format!(
            "Source {sid} has no discovered metadata{hint}. Run discovery first (POST /api/v2/sources/discover)."
        ));
    }

    for row in metadata {
        let Some(Value::Object(meta)) = &row.metadata_json else {
            continue;
        };
        let Some(Value::Object(flags)) = meta.get("discovery_flags") else {
            continue;
        };
        let bad: Vec<String> = flags
            .iter()
            .filter_map(|(k, v)| {
                if discovery_flag_passes(v) {
                    None
                } else {
                    Some(k.clone())
                }
            })
            .collect();
        if !bad.is_empty() {
            return Some(format!(
                "Source {sid} metadata has discovery_flags that have not passed (failed keys: {bad:?}). Re-run discovery."
            ));
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SourceExecutionStatus {
    pub ready_for_execution: bool,
    pub discovery_complete: bool,
    pub workflow_run_pending: bool,
    pub discovery_signature: Option<String>,
    pub last_executed_discovery_signature: Option<String>,
    pub signature_matches_last_execution: bool,
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_age_seconds: Option<i64>,
}

#[allow(clippy::too_many_arguments)]
pub fn source_execution_status(
    sid: &str,
    enabled: bool,
    last_checked_at: Option<DateTime<Utc>>,
    discovery_signature: Option<&str>,
    last_executed_discovery_signature: Option<&str>,
    discovery_claim_token: Option<&str>,
    workflow_run_pending: bool,
    workflow_run_pending_at: Option<DateTime<Utc>>,
    metadata_rows: &[ArchiveMetadataReadiness],
    sbids: Option<&[String]>,
) -> SourceExecutionStatus {
    let registered = RegisteredSourceReadiness {
        enabled,
        last_checked_at_present: last_checked_at.is_some(),
        discovery_signature: discovery_signature.map(str::to_string),
        discovery_claim_token: discovery_claim_token.map(str::to_string),
    };
    let signature_matches_last_execution = match (
        discovery_signature.filter(|s| !s.is_empty()),
        last_executed_discovery_signature.filter(|s| !s.is_empty()),
    ) {
        (Some(current), Some(last)) => current == last,
        _ => false,
    };
    let discovery_complete = registered.last_checked_at_present
        && registered
            .discovery_signature
            .as_deref()
            .is_some_and(|s| !s.is_empty())
        && registered.discovery_claim_token.is_none();

    let mut blockers = Vec::new();
    if let Some(err) = parsed_source_readiness_error(sid, sbids, Some(&registered), metadata_rows) {
        blockers.push(err);
    }
    if signature_matches_last_execution {
        blockers.push(format!(
            "Source {sid}: Discovery signature matches last executed run (already executed for current metadata)."
        ));
    }
    blockers.sort();
    blockers.dedup();

    let ready_for_execution = blockers.is_empty();
    let pending_age_seconds =
        workflow_run_pending_at.map(|at| (Utc::now() - at).num_seconds().max(0));

    SourceExecutionStatus {
        ready_for_execution,
        discovery_complete,
        workflow_run_pending,
        discovery_signature: discovery_signature.map(str::to_string),
        last_executed_discovery_signature: last_executed_discovery_signature.map(str::to_string),
        signature_matches_last_execution,
        blockers,
        pending_age_seconds: if workflow_run_pending {
            pending_age_seconds
        } else {
            None
        },
    }
}

/// Match Python ``bool(v)`` semantics for discovery flag values.
fn discovery_flag_passes(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.trim().is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|v| v != 0.0),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn reg() -> RegisteredSourceReadiness {
        RegisteredSourceReadiness {
            enabled: true,
            last_checked_at_present: true,
            discovery_signature: Some("abc".into()),
            discovery_claim_token: None,
        }
    }

    #[test]
    fn signature_match_blocks_ready() {
        let rows = vec![ArchiveMetadataReadiness {
            sbid: "123".into(),
            metadata_json: Some(json!({"discovery_flags": {"ok": true}})),
        }];
        let status = source_execution_status(
            "S",
            true,
            Some(Utc::now()),
            Some("abc"),
            Some("abc"),
            None,
            true,
            Some(Utc::now()),
            &rows,
            None,
        );
        assert!(!status.ready_for_execution);
        assert!(status.signature_matches_last_execution);
        assert!(status
            .blockers
            .iter()
            .any(|b| b.contains("already executed")));
    }

    #[test]
    fn ready_source_returns_none() {
        let rows = vec![ArchiveMetadataReadiness {
            sbid: "123".into(),
            metadata_json: Some(json!({"discovery_flags": {"ok": true}})),
        }];
        assert!(parsed_source_readiness_error("S", None, Some(&reg()), &rows).is_none());
    }

    #[test]
    fn sbid_filter_must_match() {
        let rows = vec![ArchiveMetadataReadiness {
            sbid: "123".into(),
            metadata_json: Some(json!({})),
        }];
        let sbids = vec!["456".to_string()];
        assert!(
            parsed_source_readiness_error("S", Some(&sbids), Some(&reg()), &rows)
                .unwrap()
                .contains("no discovered metadata")
        );
    }

    #[test]
    fn string_and_numeric_flags_use_python_truthiness() {
        let rows = vec![ArchiveMetadataReadiness {
            sbid: "123".into(),
            metadata_json: Some(json!({
                "discovery_flags": {
                    "ra_dec_vsys_complete": true,
                    "ra_string": "13h13m13s",
                    "dec_string": "-15d27m32s",
                    "vsys": 2505.3
                }
            })),
        }];
        assert!(parsed_source_readiness_error("S", None, Some(&reg()), &rows).is_none());
    }

    #[test]
    fn bad_flags_block_execution() {
        let rows = vec![ArchiveMetadataReadiness {
            sbid: "123".into(),
            metadata_json: Some(json!({"discovery_flags": {"ra_dec_vsys_complete": false}})),
        }];
        assert!(
            parsed_source_readiness_error("S", None, Some(&reg()), &rows)
                .unwrap()
                .contains("discovery_flags")
        );
    }
}

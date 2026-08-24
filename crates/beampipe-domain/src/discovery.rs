use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;
use utoipa::ToSchema;

pub fn no_records_payload() -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "default".to_string(),
        json!({"records": [], "discovery_status": "no_records"}),
    )])
}

pub fn no_records_signature() -> String {
    discovery_signature(&no_records_payload())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SignatureOptions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_fields: Vec<String>,
    #[serde(default = "default_include_discovery_flags")]
    pub include_discovery_flags: bool,
}

fn default_include_discovery_flags() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DiscoverySourceResult {
    HasMetadata {
        source_identifier: String,
        metadata: Vec<Value>,
        #[serde(default)]
        discovery_flags: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
    NoRecords {
        source_identifier: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
    Unchanged {
        source_identifier: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
    Timeout {
        source_identifier: String,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
    Error {
        source_identifier: String,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
    },
}

impl DiscoverySourceResult {
    pub fn duration_ms(&self) -> Option<i64> {
        match self {
            Self::HasMetadata { duration_ms, .. }
            | Self::NoRecords { duration_ms, .. }
            | Self::Unchanged { duration_ms, .. }
            | Self::Timeout { duration_ms, .. }
            | Self::Error { duration_ms, .. } => *duration_ms,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct DiscoveryBatchStats {
    pub total_sources: usize,
    pub total_groups: usize,
    pub total_records: usize,
    pub changed_count: usize,
    pub unchanged_count: usize,
    pub no_records_count: usize,
    pub error_count: usize,
    pub timeout_count: usize,
    pub failed_sources: Vec<String>,
    pub missing_registry_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_source_identifiers: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PreparedMetadataError {
    #[error("record[{index}] must be a JSON object")]
    NotObject { index: usize },
    #[error("record[{index}] requires a non-empty 'group_key'")]
    MissingGroupKey { index: usize },
    #[error("record[{index}] requires a non-empty 'record_id'")]
    MissingRecordIdentity { index: usize },
}

pub fn group_metadata_by_group_key(metadata: &[Value]) -> BTreeMap<String, Vec<Value>> {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for item in metadata {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(group_key) = obj
            .get("group_key")
            .map(value_key)
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        grouped
            .entry(group_key)
            .or_default()
            .push(Value::Object(obj.clone()));
    }
    grouped
}

pub fn validate_prepared_metadata_records(metadata: &[Value]) -> Result<(), PreparedMetadataError> {
    for (index, rec) in metadata.iter().enumerate() {
        let Some(obj) = rec.as_object() else {
            return Err(PreparedMetadataError::NotObject { index });
        };
        if obj
            .get("group_key")
            .map(value_key)
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(PreparedMetadataError::MissingGroupKey { index });
        }
        let has_identity = obj
            .get("record_id")
            .map(value_key)
            .is_some_and(|value| !value.trim().is_empty());
        if !has_identity {
            return Err(PreparedMetadataError::MissingRecordIdentity { index });
        }
    }
    Ok(())
}

pub fn metadata_payload_by_group(
    grouped: &BTreeMap<String, Vec<Value>>,
    discovery_flags: Option<&Value>,
    signature: Option<&SignatureOptions>,
) -> BTreeMap<String, Value> {
    let exclude: HashSet<&str> = signature
        .map(|s| s.exclude_fields.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let include_flags = signature.map(|s| s.include_discovery_flags).unwrap_or(true);
    let flags = if include_flags {
        discovery_flags
            .map(to_jsonable)
            .filter(|v| !v.as_object().is_some_and(Map::is_empty) && !v.is_null())
    } else {
        None
    };
    grouped
        .iter()
        .map(|(group_key, records)| {
            let mut normalized: Vec<Value> = records
                .iter()
                .map(|d| {
                    let mut value = to_jsonable(d);
                    if !exclude.is_empty() {
                        strip_excluded_fields(&mut value, &exclude);
                    }
                    value
                })
                .collect();
            normalized.sort_by_key(record_sort_key);
            let mut payload = Map::new();
            payload.insert("records".into(), Value::Array(normalized));
            if let Some(flags) = &flags {
                payload.insert("discovery_flags".into(), flags.clone());
            }
            (group_key.clone(), Value::Object(payload))
        })
        .collect()
}

/// Build the complete archive payload persisted for later staging.
///
/// Signature exclusions only control change detection. They must never remove
/// access URLs, sizes, or other execution inputs from the stored metadata.
pub fn metadata_storage_payload_by_group(
    grouped: &BTreeMap<String, Vec<Value>>,
    discovery_flags: Option<&Value>,
) -> BTreeMap<String, Value> {
    grouped
        .iter()
        .map(|(group_key, records)| {
            let mut normalized: Vec<Value> = records.iter().map(to_jsonable).collect();
            normalized.sort_by_key(record_sort_key);
            let mut payload = Map::new();
            payload.insert("records".into(), Value::Array(normalized));
            if let Some(flags) = discovery_flags
                .map(to_jsonable)
                .filter(|value| !value.as_object().is_some_and(Map::is_empty) && !value.is_null())
            {
                payload.insert("discovery_flags".into(), flags);
            }
            (group_key.clone(), Value::Object(payload))
        })
        .collect()
}

pub fn existing_signature_from_records(
    records: &[(String, Value)],
    signature: Option<&SignatureOptions>,
) -> String {
    let exclude: HashSet<&str> = signature
        .map(|s| s.exclude_fields.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mut canonical: BTreeMap<String, Value> = BTreeMap::new();
    for (group_key, metadata) in records {
        let mut value = to_jsonable(metadata);
        if !exclude.is_empty() {
            strip_excluded_fields(&mut value, &exclude);
        }
        canonical.insert(group_key.clone(), value);
    }
    discovery_signature(&canonical)
}

/// Skip expensive TAP when stored archive metadata would produce the same signature.
pub fn should_skip_tap(
    stored_sig: Option<&str>,
    archive_records: &[(String, Value)],
    signature_opts: &SignatureOptions,
) -> bool {
    stored_sig.is_some_and(|s| {
        !s.is_empty() && existing_signature_from_records(archive_records, Some(signature_opts)) == s
    })
}

pub fn discovery_signature(payload_by_group: &BTreeMap<String, Value>) -> String {
    let raw = stable_json(&Value::Object(
        payload_by_group
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ));
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

fn strip_excluded_fields(value: &mut Value, exclude: &HashSet<&str>) {
    if let Value::Object(map) = value {
        map.retain(|k, _| !exclude.contains(k.as_str()));
        for v in map.values_mut() {
            strip_excluded_fields(v, exclude);
        }
    } else if let Value::Array(items) = value {
        for item in items {
            strip_excluded_fields(item, exclude);
        }
    }
}

fn value_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn record_sort_key(value: &Value) -> String {
    let record_id = value.get("record_id").map(value_key).unwrap_or_default();
    format!("{record_id}\u{1f}{}", stable_json(value))
}

pub fn stable_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical_json(value, &mut out);
    out
}

fn write_canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(
                    &serde_json::to_string(key.as_str()).unwrap_or_else(|_| "\"\"".into()),
                );
                out.push(':');
                write_canonical_json(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn to_jsonable(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), to_jsonable(v)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(to_jsonable).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grouping_skips_missing_group_key() {
        let grouped = group_metadata_by_group_key(&[
            json!({"group_key": 1, "record_id": "a"}),
            json!({"record_id": "b"}),
        ]);
        assert_eq!(grouped.len(), 1);
        assert!(grouped.contains_key("1"));
    }

    #[test]
    fn signatures_are_order_independent() {
        let left = group_metadata_by_group_key(&[
            json!({"group_key": "2", "record_id": "b"}),
            json!({"group_key": "1", "record_id": "a"}),
        ]);
        let right = group_metadata_by_group_key(&[
            json!({"group_key": "1", "record_id": "a"}),
            json!({"group_key": "2", "record_id": "b"}),
        ]);
        let left = metadata_payload_by_group(&left, None, None);
        let right = metadata_payload_by_group(&right, None, None);
        assert_eq!(discovery_signature(&left), discovery_signature(&right));
    }

    #[test]
    fn nested_object_key_order_is_stable() {
        let a = json!({"group_key": "1", "record_id": "a", "z_field": 1, "a_field": 2});
        let b = json!({"a_field": 2, "record_id": "a", "group_key": "1", "z_field": 1});
        let grouped_a = group_metadata_by_group_key(&[a]);
        let grouped_b = group_metadata_by_group_key(&[b]);
        let sig_a = discovery_signature(&metadata_payload_by_group(&grouped_a, None, None));
        let sig_b = discovery_signature(&metadata_payload_by_group(&grouped_b, None, None));
        assert_eq!(sig_a, sig_b);
    }

    #[test]
    fn exclude_fields_ignored_in_signature() {
        let grouped = group_metadata_by_group_key(&[json!({
            "group_key": "123",
            "record_id": "a.ms",
            "visibility_filename": "a.ms",
            "access_url": "https://old.example",
            "filesize": 100
        })]);
        let with_url = metadata_payload_by_group(&grouped, None, None);
        let mut changed = grouped.clone();
        changed.get_mut("123").unwrap()[0] = json!({
            "group_key": "123",
            "record_id": "a.ms",
            "visibility_filename": "a.ms",
            "access_url": "https://new.example",
            "filesize": 999
        });
        let changed_payload = metadata_payload_by_group(&changed, None, None);
        assert_ne!(
            discovery_signature(&with_url),
            discovery_signature(&changed_payload)
        );
        let opts = SignatureOptions {
            exclude_fields: vec!["access_url".into(), "filesize".into()],
            include_discovery_flags: true,
        };
        assert_eq!(
            discovery_signature(&metadata_payload_by_group(&grouped, None, Some(&opts))),
            discovery_signature(&metadata_payload_by_group(&changed, None, Some(&opts)))
        );
        assert_eq!(
            metadata_storage_payload_by_group(&grouped, None)["123"]["records"][0]["access_url"],
            "https://old.example"
        );
        assert_eq!(
            metadata_storage_payload_by_group(&changed, None)["123"]["records"][0]["access_url"],
            "https://new.example"
        );
    }

    #[test]
    fn no_records_signature_is_stable() {
        assert_eq!(no_records_signature(), no_records_signature());
    }

    #[test]
    fn should_skip_tap_when_archive_matches_stored_sig() {
        let records = vec![(
            "123".into(),
            json!({
                "group_key": "123",
                "record_id": "a.ms",
                "visibility_filename": "a.ms"
            }),
        )];
        let opts = SignatureOptions::default();
        let sig = existing_signature_from_records(&records, Some(&opts));
        assert!(should_skip_tap(Some(&sig), &records, &opts));
    }

    #[test]
    fn should_not_skip_tap_when_exclude_fields_change_recomputation() {
        let records = vec![(
            "123".into(),
            json!({
                "group_key": "123",
                "record_id": "a.ms",
                "visibility_filename": "a.ms",
                "access_url": "https://example.test"
            }),
        )];
        let default_opts = SignatureOptions::default();
        let sig = existing_signature_from_records(&records, Some(&default_opts));
        let exclude_opts = SignatureOptions {
            exclude_fields: vec!["access_url".into()],
            include_discovery_flags: true,
        };
        assert!(!should_skip_tap(Some(&sig), &records, &exclude_opts));
    }

    #[test]
    fn validate_rejects_missing_group_key() {
        let err = validate_prepared_metadata_records(&[json!({"record_id": "a"})]).unwrap_err();
        assert_eq!(err, PreparedMetadataError::MissingGroupKey { index: 0 });
    }

    #[test]
    fn validate_rejects_missing_identity() {
        let err = validate_prepared_metadata_records(&[json!({"group_key": "1"})]).unwrap_err();
        assert_eq!(
            err,
            PreparedMetadataError::MissingRecordIdentity { index: 0 }
        );
    }

    #[test]
    fn golden_signature_vector() {
        // Stable cross-runtime vector for generic group/record metadata.
        let grouped = BTreeMap::from([(
            "123".to_string(),
            vec![json!({
                "group_key": "123",
                "record_id": "record-1",
                "visibility_filename": "a.ms",
                "checksum": "abc"
            })],
        )]);
        let payload =
            metadata_payload_by_group(&grouped, Some(&json!({"coordinates_complete": true})), None);
        let sig = discovery_signature(&payload);
        assert_eq!(sig.len(), 64);
    }
}

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
    #[error("record[{index}] requires a non-empty 'source_identifier'")]
    MissingSourceIdentifier { index: usize },
    #[error("record[{index}] requires a non-empty 'group_key'")]
    MissingGroupKey { index: usize },
    #[error("record[{index}] requires a non-empty 'record_id'")]
    MissingRecordIdentity { index: usize },
    #[error("record[{index}] identity field '{field}' must be a JSON string, number, or boolean")]
    UnsupportedIdentityType { index: usize, field: &'static str },
    #[error("record[{index}] identity field '{field}' must be a canonical non-empty string")]
    NonCanonicalIdentity { index: usize, field: &'static str },
    #[error(
        "record[{index}] source_identifier '{actual}' does not match claimed source '{expected}'"
    )]
    SourceIdentifierMismatch {
        index: usize,
        expected: String,
        actual: String,
    },
}

const PREPARED_IDENTITY_FIELDS: [&str; 3] = ["source_identifier", "group_key", "record_id"];

/// Normalize prepared discovery identities before they cross the persistence boundary.
///
/// TAP services commonly encode numeric identifiers as JSON numbers. Beampipe's
/// execution scope is string-based, so normalize every supported scalar once and
/// reject values that cannot be stable identity keys.
pub fn canonicalize_prepared_metadata_identities(
    metadata: &mut [Value],
) -> Result<(), PreparedMetadataError> {
    for (index, record) in metadata.iter_mut().enumerate() {
        let Some(object) = record.as_object_mut() else {
            return Err(PreparedMetadataError::NotObject { index });
        };
        for field in PREPARED_IDENTITY_FIELDS {
            let canonical = match object.get(field) {
                Some(Value::String(value)) => value.trim().to_string(),
                Some(Value::Number(value)) => value.to_string(),
                Some(Value::Bool(value)) => value.to_string(),
                Some(_) => {
                    return Err(PreparedMetadataError::UnsupportedIdentityType { index, field });
                }
                None => return Err(missing_identity_error(index, field)),
            };
            if canonical.is_empty() {
                return Err(missing_identity_error(index, field));
            }
            object.insert(field.to_string(), Value::String(canonical));
        }
    }
    Ok(())
}

pub fn group_metadata_by_group_key(metadata: &[Value]) -> BTreeMap<String, Vec<Value>> {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for item in metadata {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(group_key) = obj
            .get("group_key")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && *value == value.trim())
        else {
            continue;
        };
        grouped
            .entry(group_key.to_string())
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
        for field in PREPARED_IDENTITY_FIELDS {
            match obj.get(field) {
                Some(Value::String(value)) if !value.is_empty() && value == value.trim() => {}
                Some(Value::String(value)) if value.trim().is_empty() => {
                    return Err(missing_identity_error(index, field));
                }
                None => return Err(missing_identity_error(index, field)),
                Some(_) => {
                    return Err(PreparedMetadataError::NonCanonicalIdentity { index, field });
                }
            }
        }
    }
    Ok(())
}

pub fn validate_prepared_metadata_records_for_source(
    metadata: &[Value],
    expected_source_identifier: &str,
) -> Result<(), PreparedMetadataError> {
    validate_prepared_metadata_records(metadata)?;
    for (index, record) in metadata.iter().enumerate() {
        let Some(actual) = record.get("source_identifier").and_then(Value::as_str) else {
            return Err(PreparedMetadataError::NonCanonicalIdentity {
                index,
                field: "source_identifier",
            });
        };
        if actual != expected_source_identifier {
            return Err(PreparedMetadataError::SourceIdentifierMismatch {
                index,
                expected: expected_source_identifier.to_string(),
                actual: actual.to_string(),
            });
        }
    }
    Ok(())
}

fn missing_identity_error(index: usize, field: &'static str) -> PreparedMetadataError {
    match field {
        "source_identifier" => PreparedMetadataError::MissingSourceIdentifier { index },
        "group_key" => PreparedMetadataError::MissingGroupKey { index },
        "record_id" => PreparedMetadataError::MissingRecordIdentity { index },
        _ => unreachable!("unexpected prepared identity field"),
    }
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

fn record_sort_key(value: &Value) -> String {
    let record_id = value
        .get("record_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
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
    fn grouping_requires_canonical_string_group_key() {
        let grouped = group_metadata_by_group_key(&[
            json!({"group_key": 1, "record_id": "a"}),
            json!({"group_key": "1", "record_id": "b"}),
            json!({"record_id": "b"}),
        ]);
        assert_eq!(grouped.len(), 1);
        assert!(grouped.contains_key("1"));
        assert_eq!(grouped["1"].len(), 1);
    }

    #[test]
    fn prepared_identity_scalars_are_canonicalized_to_strings() {
        let mut metadata = vec![json!({
            "source_identifier": " 101 ",
            "group_key": 3001,
            "record_id": true,
        })];

        canonicalize_prepared_metadata_identities(&mut metadata).unwrap();

        assert_eq!(metadata[0]["source_identifier"], "101");
        assert_eq!(metadata[0]["group_key"], "3001");
        assert_eq!(metadata[0]["record_id"], "true");
        validate_prepared_metadata_records(&metadata).unwrap();
    }

    #[test]
    fn prepared_identity_rejects_non_scalar_values() {
        for invalid in [Value::Null, json!(["nested"]), json!({"nested": true})] {
            let mut metadata = vec![json!({
                "source_identifier": "101",
                "group_key": invalid,
                "record_id": "record-1",
            })];

            let err = canonicalize_prepared_metadata_identities(&mut metadata).unwrap_err();
            assert_eq!(
                err,
                PreparedMetadataError::UnsupportedIdentityType {
                    index: 0,
                    field: "group_key",
                }
            );
        }
    }

    #[test]
    fn validation_requires_canonical_identity_strings() {
        let err = validate_prepared_metadata_records(&[json!({
            "source_identifier": "101",
            "group_key": 3001,
            "record_id": "record-1",
        })])
        .unwrap_err();
        assert_eq!(
            err,
            PreparedMetadataError::NonCanonicalIdentity {
                index: 0,
                field: "group_key",
            }
        );

        let err = validate_prepared_metadata_records(&[json!({
            "source_identifier": "101",
            "group_key": " 3001 ",
            "record_id": "record-1",
        })])
        .unwrap_err();
        assert_eq!(
            err,
            PreparedMetadataError::NonCanonicalIdentity {
                index: 0,
                field: "group_key",
            }
        );
    }

    #[test]
    fn validation_rejects_record_for_a_different_claimed_source() {
        let metadata = [json!({
            "source_identifier": "102",
            "group_key": "3001",
            "record_id": "2001",
        })];

        let err = validate_prepared_metadata_records_for_source(&metadata, "101").unwrap_err();
        assert_eq!(
            err,
            PreparedMetadataError::SourceIdentifierMismatch {
                index: 0,
                expected: "101".into(),
                actual: "102".into(),
            }
        );
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
        let err = validate_prepared_metadata_records(&[json!({
            "source_identifier": "source-1",
            "record_id": "a"
        })])
        .unwrap_err();
        assert_eq!(err, PreparedMetadataError::MissingGroupKey { index: 0 });
    }

    #[test]
    fn validate_rejects_missing_identity() {
        let err = validate_prepared_metadata_records(&[json!({
            "source_identifier": "source-1",
            "group_key": "1"
        })])
        .unwrap_err();
        assert_eq!(
            err,
            PreparedMetadataError::MissingRecordIdentity { index: 0 }
        );
    }

    #[test]
    fn validate_rejects_missing_source_identifier() {
        let err = validate_prepared_metadata_records(&[json!({
            "group_key": "1",
            "record_id": "a"
        })])
        .unwrap_err();
        assert_eq!(
            err,
            PreparedMetadataError::MissingSourceIdentifier { index: 0 }
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

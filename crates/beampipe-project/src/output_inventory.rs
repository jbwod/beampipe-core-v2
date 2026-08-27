use crate::{output_inventory_media_type, OutputGlob};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use utoipa::ToSchema;
use uuid::Uuid;

pub const MAX_OUTPUT_INVENTORY_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_OUTPUT_PRODUCTS: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputInventoryProduct {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputPublicationAcknowledgement {
    pub acknowledged: bool,
    pub publisher: String,
    pub receipt_id: String,
    pub published_at: DateTime<Utc>,
}

/// Canonical durable-output receipt retrieved by Core from an isolated remote
/// execution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionOutputVerificationRequest {
    /// Exact Core execution identity that owns this immutable receipt.
    pub execution_id: Uuid,
    /// Must equal the execution's current retry generation.
    #[schema(example = 0, minimum = 0)]
    pub execution_attempt: i32,
    /// Must match the schema pinned when the execution was admitted.
    #[schema(example = "beampipe-output-inventory/v1")]
    pub schema: String,
    /// Project-defined relative patterns summarized by this inventory.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Positive match count for every supplied pattern.
    #[serde(default)]
    pub pattern_counts: BTreeMap<String, u64>,
    pub products: Vec<OutputInventoryProduct>,
    pub inventory_sha256: String,
    pub durable_destination_uri: String,
    pub publication: OutputPublicationAcknowledgement,
}

/// Storage-neutral fields needed to persist a canonical inventory artifact.
/// Database code maps this descriptor into its persisted record type.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputInventoryArtifactDescriptor {
    pub uri: String,
    pub report: Value,
    pub media_type: String,
    pub report_sha256: String,
    pub report_size_bytes: i64,
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputInventoryValidationKind {
    InvalidReport,
    PolicyConflict,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct OutputInventoryValidationError {
    pub kind: OutputInventoryValidationKind,
    pub message: String,
}

impl OutputInventoryValidationError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: OutputInventoryValidationKind::InvalidReport,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            kind: OutputInventoryValidationKind::PolicyConflict,
            message: message.into(),
        }
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    fn write(value: &Value, output: &mut String) -> Result<(), serde_json::Error> {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => output.push_str(&value.to_string()),
            Value::String(value) => output.push_str(&serde_json::to_string(value)?),
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write(value, output)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                let mut keys: Vec<_> = values.keys().collect();
                keys.sort_unstable();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(&serde_json::to_string(key)?);
                    output.push(':');
                    write(&values[key], output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }

    let mut output = String::new();
    write(value, &mut output)?;
    Ok(output.into_bytes())
}

pub fn canonical_products_sha256(
    products: &[OutputInventoryProduct],
) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(products)?;
    let bytes = canonical_json_bytes(&value)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Parse the fixed SSH handoff and require byte-for-byte canonical JSON. This
/// prevents audit digests from silently describing a reserialization rather
/// than the immutable remote evidence Core actually retrieved.
pub fn parse_canonical_output_inventory(
    bytes: &[u8],
) -> Result<ExecutionOutputVerificationRequest, OutputInventoryValidationError> {
    if bytes.is_empty() || bytes.len() > MAX_OUTPUT_INVENTORY_BYTES {
        return Err(OutputInventoryValidationError::invalid(format!(
            "output inventory receipt must contain 1-{MAX_OUTPUT_INVENTORY_BYTES} bytes"
        )));
    }
    let report: ExecutionOutputVerificationRequest = serde_json::from_slice(bytes).map_err(|error| {
        OutputInventoryValidationError::invalid(format!(
            "remote output inventory is not valid JSON: {error}"
        ))
    })?;
    let value = serde_json::to_value(&report)
        .map_err(|error| OutputInventoryValidationError::invalid(error.to_string()))?;
    let canonical = canonical_json_bytes(&value)
        .map_err(|error| OutputInventoryValidationError::invalid(error.to_string()))?;
    if canonical != bytes {
        return Err(OutputInventoryValidationError::invalid(
            "remote output inventory is not canonical JSON",
        ));
    }
    Ok(report)
}

pub fn build_output_inventory_artifact(
    request: &ExecutionOutputVerificationRequest,
    total_product_bytes: u64,
) -> Result<OutputInventoryArtifactDescriptor, OutputInventoryValidationError> {
    let report = serde_json::to_value(request)
        .map_err(|error| OutputInventoryValidationError::invalid(error.to_string()))?;
    let report_bytes = canonical_json_bytes(&report)
        .map_err(|error| OutputInventoryValidationError::invalid(error.to_string()))?;
    if report_bytes.len() > MAX_OUTPUT_INVENTORY_BYTES {
        return Err(OutputInventoryValidationError::invalid(format!(
            "output inventory report exceeds {MAX_OUTPUT_INVENTORY_BYTES} bytes"
        )));
    }
    let report_size_bytes = i64::try_from(report_bytes.len()).map_err(|_| {
        OutputInventoryValidationError::invalid("output inventory report is too large")
    })?;
    let media_type = output_inventory_media_type(&request.schema)
        .ok_or_else(|| {
            OutputInventoryValidationError::conflict(format!(
                "output inventory schema '{}' is unsupported",
                request.schema
            ))
        })?
        .to_owned();
    Ok(OutputInventoryArtifactDescriptor {
        uri: request.durable_destination_uri.clone(),
        report,
        media_type,
        report_sha256: format!("{:x}", Sha256::digest(&report_bytes)),
        report_size_bytes,
        metadata: serde_json::json!({
            "execution_id": request.execution_id,
            "execution_attempt": request.execution_attempt,
            "inventory_schema": request.schema,
            "inventory_sha256": request.inventory_sha256,
            "product_count": request.products.len(),
            "total_product_bytes": total_product_bytes,
            "publication": request.publication,
        }),
    })
}

fn validate_durable_destination_uri(value: &str) -> Result<(), OutputInventoryValidationError> {
    let parsed = url::Url::parse(value).map_err(|_| {
        OutputInventoryValidationError::invalid(
            "durable_destination_uri must be an absolute URI",
        )
    })?;
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(OutputInventoryValidationError::invalid(
            "durable_destination_uri must not contain credentials, a query, or a fragment",
        ));
    }
    match parsed.scheme() {
        "s3" | "gs" if parsed.host_str().is_some() => Ok(()),
        "https" if parsed.host_str().is_some() => Ok(()),
        "file" if parsed.path().starts_with('/') && parsed.path().len() > 1 => Ok(()),
        _ => Err(OutputInventoryValidationError::invalid(
            "durable_destination_uri must use s3, gs, https, or an absolute file URI",
        )),
    }
}

/// Validate a pulled receipt against the execution's immutable output policy.
/// The caller supplies `now` so polling tests are deterministic.
pub fn validate_output_verification_request_at(
    request: &ExecutionOutputVerificationRequest,
    output_verification_required: bool,
    output_verification_policy: &Value,
    expected_execution_id: Uuid,
    expected_execution_attempt: i32,
    now: DateTime<Utc>,
) -> Result<u64, OutputInventoryValidationError> {
    if !output_verification_required {
        return Err(OutputInventoryValidationError::conflict(
            "this execution explicitly opts out of output verification",
        ));
    }
    if request.execution_id != expected_execution_id {
        return Err(OutputInventoryValidationError::conflict(format!(
            "inventory execution_id {} does not match execution {expected_execution_id}",
            request.execution_id
        )));
    }
    if request.execution_attempt != expected_execution_attempt {
        return Err(OutputInventoryValidationError::conflict(format!(
            "inventory execution_attempt {} does not match the current execution attempt {expected_execution_attempt}",
            request.execution_attempt
        )));
    }
    let expected_schema = output_verification_policy
        .get("inventory_schema")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OutputInventoryValidationError::conflict("pinned output policy is invalid")
        })?;
    if request.schema != expected_schema {
        return Err(OutputInventoryValidationError::conflict(format!(
            "inventory schema '{}' does not match pinned schema '{expected_schema}'",
            request.schema
        )));
    }
    if output_inventory_media_type(expected_schema).is_none() {
        return Err(OutputInventoryValidationError::conflict(format!(
            "pinned output inventory schema '{expected_schema}' is unsupported"
        )));
    }
    let expected_patterns = output_verification_policy
        .get("expected_patterns")
        .and_then(Value::as_array)
        .and_then(|patterns| patterns.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        .ok_or_else(|| {
            OutputInventoryValidationError::conflict(
                "pinned output policy has no valid expected_patterns",
            )
        })?;
    if expected_patterns.is_empty() {
        return Err(OutputInventoryValidationError::conflict(
            "required output policy has no expected patterns",
        ));
    }
    if request.patterns.iter().map(String::as_str).collect::<Vec<_>>() != expected_patterns {
        return Err(OutputInventoryValidationError::conflict(
            "inventory patterns do not exactly match the execution's pinned expected_patterns",
        ));
    }
    let unique_patterns: BTreeSet<_> = request.patterns.iter().collect();
    if unique_patterns.len() != request.patterns.len()
        || request.pattern_counts.len() != request.patterns.len()
        || request.patterns.iter().any(|pattern| {
            !request.pattern_counts.contains_key(pattern) || OutputGlob::compile(pattern).is_err()
        })
        || request.pattern_counts.values().any(|count| *count == 0)
    {
        return Err(OutputInventoryValidationError::invalid(
            "patterns must be unique safe relative patterns and pattern_counts must contain one positive count for each pattern",
        ));
    }
    if request.products.is_empty() || request.products.len() > MAX_OUTPUT_PRODUCTS {
        return Err(OutputInventoryValidationError::invalid(format!(
            "products must contain between 1 and {MAX_OUTPUT_PRODUCTS} entries"
        )));
    }
    let mut paths = BTreeSet::new();
    let mut total_bytes = 0_u64;
    for product in &request.products {
        let path = product.path.trim();
        let unsafe_path = path.is_empty()
            || path.len() > 4096
            || path.starts_with('/')
            || path.starts_with('\\')
            || path.contains('\\')
            || path.contains('\0')
            || path
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."));
        if unsafe_path {
            return Err(OutputInventoryValidationError::invalid(
                "every output product requires a safe relative path without '.', '..', or empty components",
            ));
        }
        if product.bytes == 0 {
            return Err(OutputInventoryValidationError::invalid(format!(
                "output product '{}' must be non-empty",
                product.path
            )));
        }
        if !paths.insert(product.path.as_str()) {
            return Err(OutputInventoryValidationError::invalid(format!(
                "output product path is duplicated: {}",
                product.path
            )));
        }
        if !valid_sha256(&product.sha256) {
            return Err(OutputInventoryValidationError::invalid(format!(
                "output product '{}' has an invalid lowercase SHA-256",
                product.path
            )));
        }
        total_bytes = total_bytes.checked_add(product.bytes).ok_or_else(|| {
            OutputInventoryValidationError::invalid("total output product size overflows u64")
        })?;
    }
    for pattern in &expected_patterns {
        let matcher = OutputGlob::compile(pattern).map_err(|_| {
            OutputInventoryValidationError::conflict(
                "pinned output policy has an invalid glob",
            )
        })?;
        let actual_count = request
            .products
            .iter()
            .filter(|product| matcher.is_match(&product.path))
            .count() as u64;
        let claimed_count = request.pattern_counts.get(*pattern).copied().unwrap_or(0);
        if actual_count == 0 || claimed_count != actual_count {
            return Err(OutputInventoryValidationError::invalid(format!(
                "pattern_counts['{pattern}'] must equal the positive match count {actual_count} computed from products"
            )));
        }
    }
    if !valid_sha256(&request.inventory_sha256) {
        return Err(OutputInventoryValidationError::invalid(
            "inventory_sha256 must be 64 lowercase hexadecimal characters",
        ));
    }
    let calculated = canonical_products_sha256(&request.products)
        .map_err(|error| OutputInventoryValidationError::invalid(error.to_string()))?;
    if request.inventory_sha256 != calculated {
        return Err(OutputInventoryValidationError::invalid(format!(
            "inventory_sha256 does not match canonical products JSON (expected {calculated})"
        )));
    }
    validate_durable_destination_uri(&request.durable_destination_uri)?;
    if !request.publication.acknowledged {
        return Err(OutputInventoryValidationError::invalid(
            "publication.acknowledged must be true",
        ));
    }
    for (field, value) in [
        ("publication.publisher", request.publication.publisher.as_str()),
        (
            "publication.receipt_id",
            request.publication.receipt_id.as_str(),
        ),
    ] {
        if value.trim().is_empty() || value.len() > 256 {
            return Err(OutputInventoryValidationError::invalid(format!(
                "{field} must contain 1-256 characters"
            )));
        }
    }
    if request.publication.published_at > now + chrono::Duration::minutes(5) {
        return Err(OutputInventoryValidationError::invalid(
            "publication.published_at cannot be more than five minutes in the future",
        ));
    }
    Ok(total_bytes)
}

pub fn validate_output_verification_request(
    request: &ExecutionOutputVerificationRequest,
    output_verification_required: bool,
    output_verification_policy: &Value,
    expected_execution_id: Uuid,
    expected_execution_attempt: i32,
) -> Result<u64, OutputInventoryValidationError> {
    validate_output_verification_request_at(
        request,
        output_verification_required,
        output_verification_policy,
        expected_execution_id,
        expected_execution_attempt,
        Utc::now(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_report() -> ExecutionOutputVerificationRequest {
        let products = vec![OutputInventoryProduct {
            path: "image/example.fits".into(),
            bytes: 42,
            sha256: "a".repeat(64),
        }];
        ExecutionOutputVerificationRequest {
            execution_id: Uuid::parse_str("019d3cf7-53c0-7f8a-98f1-0f7d69db74be").unwrap(),
            execution_attempt: 2,
            schema: "beampipe-output-inventory/v1".into(),
            patterns: vec!["image/*.fits".into()],
            pattern_counts: BTreeMap::from([("image/*.fits".into(), 1)]),
            inventory_sha256: canonical_products_sha256(&products).unwrap(),
            products,
            durable_destination_uri: "s3://outputs/execution/attempt-2".into(),
            publication: OutputPublicationAcknowledgement {
                acknowledged: true,
                publisher: "beampipe-publish".into(),
                receipt_id: "receipt-2".into(),
                published_at: "2026-08-27T00:00:00Z".parse().unwrap(),
            },
        }
    }

    #[test]
    fn validates_remote_inventory_against_the_pinned_contract() {
        let report = valid_report();
        assert_eq!(
            validate_output_verification_request_at(
                &report,
                true,
                &json!({
                    "inventory_schema": "beampipe-output-inventory/v1",
                    "expected_patterns": ["image/*.fits"],
                }),
                report.execution_id,
                2,
                "2026-08-27T00:01:00Z".parse().unwrap(),
            )
            .unwrap(),
            42
        );
    }

    #[test]
    fn attempt_and_policy_mismatch_are_conflicts() {
        let report = valid_report();
        let error = validate_output_verification_request_at(
            &report,
            true,
            &json!({
                "inventory_schema": "beampipe-output-inventory/v1",
                "expected_patterns": ["image/*.fits"],
            }),
            report.execution_id,
            3,
            "2026-08-27T00:01:00Z".parse().unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.kind, OutputInventoryValidationKind::PolicyConflict);
    }

    #[test]
    fn canonical_json_orders_object_keys() {
        assert_eq!(
            canonical_json_bytes(&json!({"z": 1, "a": [true, null]})).unwrap(),
            br#"{"a":[true,null],"z":1}"#
        );
    }

    #[test]
    fn remote_handoff_rejects_noncanonical_json() {
        let report = valid_report();
        let canonical = canonical_json_bytes(&serde_json::to_value(&report).unwrap()).unwrap();
        assert_eq!(parse_canonical_output_inventory(&canonical).unwrap().execution_id, report.execution_id);

        let mut with_newline = canonical;
        with_newline.push(b'\n');
        assert!(parse_canonical_output_inventory(&with_newline)
            .unwrap_err()
            .to_string()
            .contains("not canonical JSON"));
    }

    #[test]
    fn cross_execution_receipt_is_rejected() {
        let report = valid_report();
        let other = Uuid::parse_str("019d3cf7-53c0-7f8a-98f1-0f7d69db74bf").unwrap();
        let error = validate_output_verification_request_at(
            &report,
            true,
            &json!({
                "inventory_schema": "beampipe-output-inventory/v1",
                "expected_patterns": ["image/*.fits"],
            }),
            other,
            2,
            "2026-08-27T00:01:00Z".parse().unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.kind, OutputInventoryValidationKind::PolicyConflict);
        assert!(error.to_string().contains("execution_id"));
    }

    #[test]
    fn durable_destination_rejects_query_material() {
        let mut report = valid_report();
        report.durable_destination_uri = "https://outputs.example/result?token=secret".into();
        let error = validate_output_verification_request_at(
            &report,
            true,
            &json!({
                "inventory_schema": "beampipe-output-inventory/v1",
                "expected_patterns": ["image/*.fits"],
            }),
            report.execution_id,
            2,
            "2026-08-27T00:01:00Z".parse().unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("query"));
    }
}

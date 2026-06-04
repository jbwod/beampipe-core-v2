use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use utoipa::ToSchema;

pub mod expressions;
pub mod transforms;
pub mod wasm;

pub use expressions::evaluate_expression;
pub use transforms::{
    apply_field_transform, apply_transform_spec, build_template_context, validate_transform_refs,
    TransformRegistry,
};
pub use wasm::{shared_host, HookKind, WasmHost, WasmHostError};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct DefinitionsConfig {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub transforms: std::collections::BTreeMap<String, TransformSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SourceIdentityConfig {
    #[serde(default = "default_canonical_field")]
    pub canonical: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub template_vars: std::collections::BTreeMap<String, TemplateVarSpec>,
}

fn default_canonical_field() -> String {
    "source_identifier".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct TemplateVarSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct TransformSpec {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separators: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SignatureConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_fields: Vec<String>,
    #[serde(default = "default_include_discovery_flags")]
    pub include_discovery_flags: bool,
}

fn default_include_discovery_flags() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ProjectConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: ProjectMetadata,
    #[serde(default)]
    pub adapters: AdapterConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<GraphConfig>,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<ManifestConfig>,
    #[serde(default)]
    pub graph_patches: Vec<GraphPatch>,
    #[serde(default)]
    pub automation: AutomationConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<ExtensionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definitions: Option<DefinitionsConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<SourceIdentityConfig>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            api_version: "beampipe.dev/v1".into(),
            kind: "ProjectConfig".into(),
            metadata: ProjectMetadata {
                id: String::new(),
                description: None,
            },
            adapters: AdapterConfig::default(),
            graph: None,
            discovery: DiscoveryConfig::default(),
            manifest: None,
            graph_patches: Vec::new(),
            automation: AutomationConfig::default(),
            extension: None,
            definitions: None,
            source_identity: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ProjectMetadata {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct AdapterConfig {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub casda_tap_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vizier_tap_url: Option<String>,
    #[serde(default)]
    pub tap: TapConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct TapConfig {
    #[serde(default = "default_tap_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_tap_retries")]
    pub retries: u32,
    #[serde(default)]
    pub fail_open: bool,
}

impl Default for TapConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_tap_timeout_seconds(),
            retries: default_tap_retries(),
            fail_open: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct GraphConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct DiscoveryConfig {
    #[serde(default)]
    pub queries: Vec<DiscoveryQuery>,
    #[serde(default)]
    pub enrichments: Vec<DiscoveryQuery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare_metadata: Option<PrepareMetadataConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct DiscoveryQuery {
    pub name: String,
    pub adapter: String,
    pub template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id_transform: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct PrepareMetadataConfig {
    #[serde(default)]
    pub field_map: serde_json::Value,
    #[serde(default)]
    pub discovery_flags: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ManifestConfig {
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub source_template: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_template: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand_from: Option<String>,
    #[serde(default = "default_manifest_path")]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct GraphPatch {
    pub r#match: serde_json::Value,
    pub set: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct AutomationConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionAutomationConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery: Option<DiscoveryAutomationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct DiscoveryAutomationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tick_discovery_source_limit")]
    pub tick_discovery_source_limit: i64,
    #[serde(default = "default_discovery_batch_size")]
    pub batch_size: i64,
    #[serde(default = "default_stale_after_hours")]
    pub stale_after_hours: i32,
    #[serde(default = "default_claim_ttl_minutes")]
    pub claim_ttl_minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_max_depth: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_discovery_batch_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrent_discovery_batch_limit: Option<i64>,
}

impl Default for DiscoveryAutomationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tick_discovery_source_limit: default_tick_discovery_source_limit(),
            batch_size: default_discovery_batch_size(),
            stale_after_hours: default_stale_after_hours(),
            claim_ttl_minutes: default_claim_ttl_minutes(),
            queue_max_depth: None,
            tick_discovery_batch_limit: None,
            concurrent_discovery_batch_limit: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ExecutionAutomationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_archive_name")]
    pub archive_name: String,
    #[serde(default = "default_max_sources_per_execution")]
    pub max_sources_per_execution: i64,
    #[serde(default = "default_tick_execution_source_limit")]
    pub tick_execution_source_limit: i64,
    #[serde(default = "default_tick_execution_run_limit")]
    pub tick_execution_run_limit: i64,
    #[serde(default = "default_min_sources_to_trigger")]
    pub min_sources_to_trigger: i64,
    #[serde(default = "default_max_wait_minutes")]
    pub max_wait_minutes: i64,
    #[serde(default = "default_claim_ttl_minutes")]
    pub claim_ttl_minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrent_execution_run_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_slurm_remote_poll_max_rounds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_rest_remote_poll_max_rounds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_rest_remote_poll_interval_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_slurm_remote_poll_interval_seconds: Option<f64>,
}

impl Default for ExecutionAutomationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            archive_name: default_archive_name(),
            max_sources_per_execution: default_max_sources_per_execution(),
            tick_execution_source_limit: default_tick_execution_source_limit(),
            tick_execution_run_limit: default_tick_execution_run_limit(),
            min_sources_to_trigger: default_min_sources_to_trigger(),
            max_wait_minutes: default_max_wait_minutes(),
            claim_ttl_minutes: default_claim_ttl_minutes(),
            concurrent_execution_run_limit: None,
            deployment_profile_name: None,
            execution_slurm_remote_poll_max_rounds: None,
            execution_rest_remote_poll_max_rounds: None,
            execution_rest_remote_poll_interval_seconds: None,
            execution_slurm_remote_poll_interval_seconds: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ExtensionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm_sha256: Option<String>,
    #[serde(default)]
    pub hooks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ValidationReport {
    pub project_id: String,
    pub valid: bool,
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub spec_sha256: String,
}

#[derive(Debug, Error)]
pub enum ProjectConfigError {
    #[error("invalid YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl ProjectConfig {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, ProjectConfigError> {
        match serde_yaml::from_slice(bytes) {
            Ok(v) => Ok(v),
            Err(yaml_err) => match serde_json::from_slice(bytes) {
                Ok(v) => Ok(v),
                Err(_) => Err(ProjectConfigError::Yaml(yaml_err)),
            },
        }
    }

    pub fn validate_report(&self) -> ValidationReport {
        self.validate_report_against(None)
    }

    pub fn validate_report_against(&self, previous: Option<&ProjectConfig>) -> ValidationReport {
        let mut errors = Vec::new();
        if let Err(schema_errors) = validate_against_json_schema(self) {
            errors.extend(schema_errors);
        }
        if self.api_version != "beampipe.dev/v1" {
            errors.push("apiVersion must be beampipe.dev/v1".into());
        }
        if self.kind != "ProjectConfig" {
            errors.push("kind must be ProjectConfig".into());
        }
        if self.metadata.id.trim().is_empty() {
            errors.push("metadata.id is required".into());
        }
        if self.adapters.required.is_empty() {
            errors.push("adapters.required must include at least one adapter".into());
        }
        if self.adapters.tap.timeout_seconds == 0 {
            errors.push("adapters.tap.timeout_seconds must be > 0".into());
        }
        if let Some(discovery) = &self.automation.discovery {
            if discovery.batch_size <= 0 {
                errors.push("automation.discovery.batch_size must be > 0".into());
            }
            if discovery.tick_discovery_source_limit <= 0 {
                errors.push("automation.discovery.tick_discovery_source_limit must be > 0".into());
            }
        }
        if let Some(execution) = &self.automation.execution {
            if execution.max_sources_per_execution <= 0 {
                errors.push("automation.execution.max_sources_per_execution must be > 0".into());
            }
            if execution.tick_execution_run_limit <= 0 {
                errors.push("automation.execution.tick_execution_run_limit must be > 0".into());
            }
        }
        if let Some(graph) = &self.graph {
            if graph.url.is_some() && graph.path.is_some() {
                errors.push("graph must use only one of url or path".into());
            }
        }
        if let Some(ext) = &self.extension {
            const ALLOWED: &[&str] = &["prepare_metadata", "manifest", "graph_patches"];
            for hook in &ext.hooks {
                if !ALLOWED.contains(&hook.as_str()) {
                    errors.push(format!(
                        "extension.hooks contains unknown hook '{hook}'; allowed: {ALLOWED:?}"
                    ));
                }
            }
        }
        errors.extend(validate_transform_refs(self));
        if let Some(prepare) = &self.discovery.prepare_metadata {
            if let Some(sig) = &prepare.signature {
                for (i, field) in sig.exclude_fields.iter().enumerate() {
                    if field.trim().is_empty() {
                        errors.push(format!(
                            "discovery.prepare_metadata.signature.exclude_fields[{i}] must be non-empty"
                        ));
                    }
                }
            }
        }
        let warnings = collect_config_warnings(self, previous);
        ValidationReport {
            project_id: self.metadata.id.clone(),
            valid: errors.is_empty(),
            errors,
            warnings,
            spec_sha256: self.sha256(),
        }
    }

    pub fn sha256(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        format!("{:x}", Sha256::digest(bytes))
    }
}

fn collect_config_warnings(
    config: &ProjectConfig,
    previous: Option<&ProjectConfig>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(prev) = previous else {
        return warnings;
    };
    let old_sig = prev
        .discovery
        .prepare_metadata
        .as_ref()
        .and_then(|p| p.signature.as_ref());
    let new_sig = config
        .discovery
        .prepare_metadata
        .as_ref()
        .and_then(|p| p.signature.as_ref());
    if json_value_fingerprint(&old_sig) != json_value_fingerprint(&new_sig) {
        warnings.push(
            "discovery.prepare_metadata.signature added or changed; expect discovery re-signatures and a workflow_run_pending wave".into(),
        );
    }
    let old_field_map = prev
        .discovery
        .prepare_metadata
        .as_ref()
        .map(|p| &p.field_map);
    let new_field_map = config
        .discovery
        .prepare_metadata
        .as_ref()
        .map(|p| &p.field_map);
    if json_value_fingerprint(&old_field_map) != json_value_fingerprint(&new_field_map) {
        warnings.push(
            "discovery.prepare_metadata.field_map changed; prepared metadata shape and discovery signatures may change".into(),
        );
    }
    if json_value_fingerprint(&prev.definitions) != json_value_fingerprint(&config.definitions) {
        warnings.push(
            "definitions.transforms changed; prepared metadata shape and discovery signatures may change".into(),
        );
    }
    warnings
}

fn json_value_fingerprint<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn validate_against_json_schema(config: &ProjectConfig) -> Result<(), Vec<String>> {
    let schema = schemars::schema_for!(ProjectConfig);
    let schema_value = serde_json::to_value(&schema).unwrap_or(Value::Null);
    let compiled = match jsonschema::JSONSchema::compile(&schema_value) {
        Ok(v) => v,
        Err(e) => return Err(vec![format!("internal JSON Schema build failed: {e}")]),
    };
    let value = serde_json::to_value(config).unwrap_or(Value::Null);
    if compiled.is_valid(&value) {
        return Ok(());
    }
    let mut msgs = Vec::new();
    if let Err(errors) = compiled.validate(&value) {
        for e in errors {
            msgs.push(format!("schema: {e}"));
        }
    }
    Err(msgs)
}

fn default_tap_timeout_seconds() -> u64 {
    30
}

fn default_tap_retries() -> u32 {
    1
}

fn default_manifest_path() -> String {
    "manifest.json".into()
}

fn default_tick_discovery_source_limit() -> i64 {
    200
}

fn default_discovery_batch_size() -> i64 {
    5
}

fn default_stale_after_hours() -> i32 {
    24
}

fn default_claim_ttl_minutes() -> i64 {
    180
}

fn default_archive_name() -> String {
    "casda".into()
}

fn default_max_sources_per_execution() -> i64 {
    20
}

fn default_tick_execution_source_limit() -> i64 {
    500
}

fn default_tick_execution_run_limit() -> i64 {
    20
}

fn default_min_sources_to_trigger() -> i64 {
    1
}

fn default_max_wait_minutes() -> i64 {
    24 * 60
}

#[cfg(test)]
mod config_golden_tests {
    use super::*;

    #[test]
    fn wallaby_reference_config_validates() {
        let config =
            ProjectConfig::from_slice(include_bytes!("../../../config/wallaby_hires.v1.yaml"))
                .expect("parse wallaby yaml");
        let report = config.validate_report();
        assert!(report.valid, "wallaby config invalid: {:?}", report.errors);
    }

    #[test]
    fn minimal_survey_example_validates() {
        let config = ProjectConfig::from_slice(include_bytes!(
            "../../../config/examples/minimal_survey.v1.yaml"
        ))
        .expect("parse minimal survey yaml");
        let report = config.validate_report();
        assert!(
            report.valid,
            "minimal survey config invalid: {:?}",
            report.errors
        );
    }

    #[test]
    fn legacy_field_map_transform_aliases_resolve() {
        let yaml = r#"
apiVersion: beampipe.dev/v1
kind: ProjectConfig
metadata:
  id: legacy
adapters:
  required: [casda]
discovery:
  prepare_metadata:
    field_map:
      sbid:
        from: obs_id
        transform: extract_askap_sbid
"#;
        let config = ProjectConfig::from_slice(yaml.as_bytes()).unwrap();
        assert!(config.validate_report().valid);
    }

    #[test]
    fn inline_chain_field_map_validates() {
        let yaml = r#"
apiVersion: beampipe.dev/v1
kind: ProjectConfig
metadata:
  id: chain-test
adapters:
  required: [casda]
definitions:
  transforms:
    askap_sbid:
      kind: extract_digits
    trim:
      kind: trim
discovery:
  prepare_metadata:
    field_map:
      sbid:
        from: obs_id
        transform: [askap_sbid, trim]
"#;
        let config = ProjectConfig::from_slice(yaml.as_bytes()).unwrap();
        assert!(config.validate_report().valid);
    }
}

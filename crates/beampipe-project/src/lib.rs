use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub transforms: BTreeMap<String, TransformSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct SourceIdentityConfig {
    #[serde(default = "default_canonical_field")]
    pub canonical: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub template_vars: BTreeMap<String, TemplateVarSpec>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransformKind {
    Identity,
    Trim,
    Lowercase,
    Uppercase,
    Replace,
    AddPrefix,
    AddSuffix,
    DefaultIfEmpty,
    Chain,
    StripPrefix,
    ExtractDigits,
    SplitLast,
    IsPresent,
    SelectEvalFileBySize,
    RegexExtract,
    #[serde(other)]
    Unknown,
}

impl TransformKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Trim => "trim",
            Self::Lowercase => "lowercase",
            Self::Uppercase => "uppercase",
            Self::Replace => "replace",
            Self::AddPrefix => "add_prefix",
            Self::AddSuffix => "add_suffix",
            Self::DefaultIfEmpty => "default_if_empty",
            Self::Chain => "chain",
            Self::StripPrefix => "strip_prefix",
            Self::ExtractDigits => "extract_digits",
            Self::SplitLast => "split_last",
            Self::IsPresent => "is_present",
            Self::SelectEvalFileBySize => "select_eval_file_by_size",
            Self::RegexExtract => "regex_extract",
            Self::Unknown => "unknown",
        }
    }
}

impl From<&str> for TransformKind {
    fn from(value: &str) -> Self {
        match value {
            "identity" => Self::Identity,
            "trim" => Self::Trim,
            "lowercase" => Self::Lowercase,
            "uppercase" => Self::Uppercase,
            "replace" => Self::Replace,
            "add_prefix" => Self::AddPrefix,
            "add_suffix" => Self::AddSuffix,
            "default_if_empty" => Self::DefaultIfEmpty,
            "chain" => Self::Chain,
            "strip_prefix" => Self::StripPrefix,
            "extract_digits" => Self::ExtractDigits,
            "split_last" => Self::SplitLast,
            "is_present" => Self::IsPresent,
            "select_eval_file_by_size" => Self::SelectEvalFileBySize,
            "regex_extract" => Self::RegexExtract,
            _ => Self::Unknown,
        }
    }
}

impl From<String> for TransformKind {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct TransformSpec {
    pub kind: TransformKind,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(untagged)]
pub enum TransformRef {
    Name(String),
    Chain(Vec<String>),
}

impl TransformRef {
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::Name(name) => vec![name.as_str()],
            Self::Chain(steps) => steps.iter().map(String::as_str).collect(),
        }
    }
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
            api_version: "beampipe.dev/v2".into(),
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
    pub field_map: BTreeMap<String, MappingSpec>,
    #[serde(default)]
    pub discovery_flags: BTreeMap<String, MappingSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct MappingSpec {
    #[serde(default)]
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformRef>,
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
    #[serde(default)]
    pub r#match: GraphPatchMatch,
    #[serde(default)]
    pub set: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct GraphPatchMatch {
    #[serde(default = "default_graph_patch_match_kind")]
    pub kind: GraphPatchMatchKind,
    #[serde(default)]
    pub equals: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum GraphPatchMatchKind {
    NodeName,
    #[serde(other)]
    #[default]
    Unknown,
}

fn default_graph_patch_match_kind() -> GraphPatchMatchKind {
    GraphPatchMatchKind::Unknown
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
    pub hooks: Vec<ExtensionHook>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionHook {
    PrepareMetadata,
    Manifest,
    GraphPatches,
    #[serde(other)]
    Unknown,
}

impl ExtensionHook {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PrepareMetadata => "prepare_metadata",
            Self::Manifest => "manifest",
            Self::GraphPatches => "graph_patches",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ValidationReport {
    pub project_id: String,
    pub valid: bool,
    pub errors: Vec<ValidationDiagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ValidationDiagnostic>,
    pub spec_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct ValidationDiagnostic {
    pub path: String,
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ValidationDiagnostic {
    pub fn error(
        path: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            severity: DiagnosticSeverity::Error,
            code: code.into(),
            message: message.into(),
            hint: None,
        }
    }

    pub fn warning(
        path: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            severity: DiagnosticSeverity::Warning,
            code: code.into(),
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
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
        if self.api_version != "beampipe.dev/v2" {
            let mut diag = ValidationDiagnostic::error(
                "apiVersion",
                "legacy_api_version",
                "apiVersion must be beampipe.dev/v2",
            );
            if self.api_version == "beampipe.dev/v1" {
                diag = diag.with_hint("v1 project configs are legacy; convert the document to the v2 typed shape before upload");
            }
            errors.push(diag);
        }
        if self.kind != "ProjectConfig" {
            errors.push(ValidationDiagnostic::error(
                "kind",
                "invalid_kind",
                "kind must be ProjectConfig",
            ));
        }
        if self.metadata.id.trim().is_empty() {
            errors.push(ValidationDiagnostic::error(
                "metadata.id",
                "required",
                "metadata.id is required",
            ));
        }
        if self.adapters.required.is_empty() {
            errors.push(ValidationDiagnostic::error(
                "adapters.required",
                "required",
                "adapters.required must include at least one adapter",
            ));
        }
        if self.adapters.tap.timeout_seconds == 0 {
            errors.push(ValidationDiagnostic::error(
                "adapters.tap.timeout_seconds",
                "invalid_limit",
                "adapters.tap.timeout_seconds must be > 0",
            ));
        }
        if let Some(discovery) = &self.automation.discovery {
            if discovery.batch_size <= 0 {
                errors.push(ValidationDiagnostic::error(
                    "automation.discovery.batch_size",
                    "invalid_limit",
                    "automation.discovery.batch_size must be > 0",
                ));
            }
            if discovery.tick_discovery_source_limit <= 0 {
                errors.push(ValidationDiagnostic::error(
                    "automation.discovery.tick_discovery_source_limit",
                    "invalid_limit",
                    "automation.discovery.tick_discovery_source_limit must be > 0",
                ));
            }
        }
        if let Some(execution) = &self.automation.execution {
            if execution.max_sources_per_execution <= 0 {
                errors.push(ValidationDiagnostic::error(
                    "automation.execution.max_sources_per_execution",
                    "invalid_limit",
                    "automation.execution.max_sources_per_execution must be > 0",
                ));
            }
            if execution.tick_execution_run_limit <= 0 {
                errors.push(ValidationDiagnostic::error(
                    "automation.execution.tick_execution_run_limit",
                    "invalid_limit",
                    "automation.execution.tick_execution_run_limit must be > 0",
                ));
            }
        }
        if let Some(graph) = &self.graph {
            if graph.url.is_some() && graph.path.is_some() {
                errors.push(ValidationDiagnostic::error(
                    "graph",
                    "mutually_exclusive",
                    "graph must use only one of url or path",
                ));
            }
        }
        if let Some(ext) = &self.extension {
            for (i, hook) in ext.hooks.iter().enumerate() {
                if hook == &ExtensionHook::Unknown {
                    errors.push(
                        ValidationDiagnostic::error(
                            format!("extension.hooks[{i}]"),
                            "unknown_extension_hook",
                            "extension.hooks contains an unknown hook",
                        )
                        .with_hint(
                            "allowed hooks are prepare_metadata, manifest, and graph_patches",
                        ),
                    );
                }
            }
        }
        errors.extend(validate_transform_refs(self));
        errors.extend(validate_graph_patches(self));
        if let Some(prepare) = &self.discovery.prepare_metadata {
            if let Some(sig) = &prepare.signature {
                for (i, field) in sig.exclude_fields.iter().enumerate() {
                    if field.trim().is_empty() {
                        errors.push(ValidationDiagnostic::error(
                            format!("discovery.prepare_metadata.signature.exclude_fields[{i}]"),
                            "required",
                            "signature exclude fields must be non-empty",
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
) -> Vec<ValidationDiagnostic> {
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
        warnings.push(ValidationDiagnostic::warning(
            "discovery.prepare_metadata.signature",
            "signature_changed",
            "discovery signature config changed; expect discovery re-signatures and a workflow_run_pending wave",
        ));
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
        warnings.push(ValidationDiagnostic::warning(
            "discovery.prepare_metadata.field_map",
            "field_map_changed",
            "field_map changed; prepared metadata shape and discovery signatures may change",
        ));
    }
    if json_value_fingerprint(&prev.definitions) != json_value_fingerprint(&config.definitions) {
        warnings.push(ValidationDiagnostic::warning(
            "definitions.transforms",
            "definitions_changed",
            "definitions.transforms changed; prepared metadata shape and discovery signatures may change",
        ));
    }
    warnings
}

fn json_value_fingerprint<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn validate_against_json_schema(config: &ProjectConfig) -> Result<(), Vec<ValidationDiagnostic>> {
    let schema = schemars::schema_for!(ProjectConfig);
    let schema_value = serde_json::to_value(&schema).unwrap_or(Value::Null);
    let compiled = match jsonschema::JSONSchema::compile(&schema_value) {
        Ok(v) => v,
        Err(e) => {
            return Err(vec![ValidationDiagnostic::error(
                "$",
                "internal_schema_error",
                format!("internal JSON Schema build failed: {e}"),
            )])
        }
    };
    let value = serde_json::to_value(config).unwrap_or(Value::Null);
    if compiled.is_valid(&value) {
        return Ok(());
    }
    let mut msgs = Vec::new();
    if let Err(errors) = compiled.validate(&value) {
        for e in errors {
            msgs.push(ValidationDiagnostic::error(
                e.instance_path.to_string(),
                "schema",
                e.to_string(),
            ));
        }
    }
    Err(msgs)
}

fn validate_graph_patches(config: &ProjectConfig) -> Vec<ValidationDiagnostic> {
    let mut errors = Vec::new();
    for (i, patch) in config.graph_patches.iter().enumerate() {
        if patch.r#match.kind == GraphPatchMatchKind::Unknown {
            errors.push(
                ValidationDiagnostic::error(
                    format!("graph_patches[{i}].match.kind"),
                    "unknown_graph_patch_match_kind",
                    "graph patch match kind is unknown",
                )
                .with_hint("allowed match kind is node_name"),
            );
        }
        if patch.r#match.equals.trim().is_empty() {
            errors.push(ValidationDiagnostic::error(
                format!("graph_patches[{i}].match.equals"),
                "required",
                "graph patch match equals must be non-empty",
            ));
        }
        if patch.set.is_empty() {
            errors.push(ValidationDiagnostic::error(
                format!("graph_patches[{i}].set"),
                "required",
                "graph patch set must include at least one field",
            ));
        }
        for (field, value) in &patch.set {
            if field.trim().is_empty() {
                errors.push(ValidationDiagnostic::error(
                    format!("graph_patches[{i}].set"),
                    "required",
                    "graph patch set field names must be non-empty",
                ));
            }
            if let Some(expr) = value.as_str().filter(|s| s.starts_with('$')) {
                let valid = expr.starts_with("$count(") && expr.ends_with(')')
                    || expr.starts_with("$sum(") && expr.ends_with(')');
                if !valid {
                    errors.push(ValidationDiagnostic::error(
                        format!("graph_patches[{i}].set.{field}"),
                        "invalid_expression",
                        "graph patch expressions must use existing $count(...) or $sum(...) forms",
                    ));
                }
            }
        }
    }
    errors
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
            ProjectConfig::from_slice(include_bytes!("../../../config/wallaby_hires.v2.yaml"))
                .expect("parse wallaby yaml");
        let report = config.validate_report();
        assert!(report.valid, "wallaby config invalid: {:?}", report.errors);
    }

    #[test]
    fn minimal_survey_example_validates() {
        let config = ProjectConfig::from_slice(include_bytes!(
            "../../../config/examples/minimal_survey.v2.yaml"
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
    fn legacy_v1_config_parses_but_does_not_validate() {
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
        let report = config.validate_report();
        assert!(!report.valid);
        assert!(report.errors.iter().any(|e| e.code == "legacy_api_version"));
    }

    #[test]
    fn inline_chain_field_map_validates() {
        let yaml = r#"
apiVersion: beampipe.dev/v2
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

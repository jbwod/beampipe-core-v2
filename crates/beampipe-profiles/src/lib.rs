use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Component, Path};
use thiserror::Error;
use utoipa::ToSchema;

const RESERVED_SLURM_RUNTIME_ENVIRONMENT: [&str; 7] = [
    "BEAMPIPE_SLURM_ACCOUNT",
    "PYTHONPATH",
    "BEAMPIPE_EXECUTION_ID",
    "BEAMPIPE_EXECUTION_ATTEMPT",
    "BEAMPIPE_OUTPUT_DESTINATION_URI",
    "BEAMPIPE_OUTPUT_ROOT",
    "BEAMPIPE_OUTPUT_INVENTORY_HANDOFF_PATH",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DaliugeAlgo {
    #[default]
    Metis,
    Mysarkar,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DaliugeTranslationConfig {
    #[serde(default)]
    pub algo: DaliugeAlgo,
    #[serde(default = "one")]
    pub num_par: i32,
    #[serde(default)]
    pub num_islands: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tm_url: Option<String>,
}

impl Default for DaliugeTranslationConfig {
    fn default() -> Self {
        Self {
            algo: DaliugeAlgo::default(),
            num_par: one(),
            num_islands: 0,
            tm_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeploymentConfig {
    RestRemote(RestRemoteDeploymentConfig),
    SlurmRemote(SlurmRemoteDeploymentConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestRemoteDeploymentConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim_host_for_tm: Option<String>,
    #[serde(default = "default_dim_port", skip_serializing_if = "Option::is_none")]
    pub dim_port_for_tm: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_host: Option<String>,
    #[serde(default = "default_dim_port", skip_serializing_if = "Option::is_none")]
    pub deploy_port: Option<i32>,
    #[serde(default)]
    pub use_https: bool,
    #[serde(default = "default_true")]
    pub verify_ssl: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicationRuntimeConfig {
    /// Worker environment variable containing the durable destination base
    /// URI. The publisher adds its execution/attempt namespace.
    pub durable_destination_uri_environment: String,
}

impl PublicationRuntimeConfig {
    pub fn validate(&self) -> Result<(), ProfileValidationError> {
        validate_publication_environment_name(
            &self.durable_destination_uri_environment,
            "deployment.publication.durable_destination_uri_environment",
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SlurmResourceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus_per_task: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_minutes: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_of_service: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DaliugeManagerTopologyConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub islands: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlurmRuntimeEnvironmentKind {
    #[default]
    NonEmpty,
    ReadableFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SlurmRuntimeEnvironmentRequirement {
    /// Environment variable read from the Beampipe process and forwarded into
    /// the remote login shell and outer allocation. Profiles contain the name,
    /// never the value.
    pub name: String,
    #[serde(default)]
    pub kind: SlurmRuntimeEnvironmentKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SlurmRuntimeContractConfig {
    /// Project/runtime commands in addition to Core's Slurm and Python tools.
    #[serde(default)]
    pub required_commands: Vec<String>,
    /// Python modules in addition to `dlg.deploy.create_dlg_job`.
    #[serde(default)]
    pub required_python_modules: Vec<String>,
    /// Non-secret environment values that must be present on the Beampipe
    /// process and are forwarded to the outer allocation.
    #[serde(default)]
    pub required_environment: Vec<SlurmRuntimeEnvironmentRequirement>,
    /// Run-scoped directory beneath the generated DALiuGE session directory.
    pub output_subdirectory: String,
    /// Reusable directory beneath `dlg_root`, shared by executions using the
    /// profile.
    pub shared_staging_subdirectory: String,
    /// Optional variable exported to the output directory path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_environment_variable: Option<String>,
    /// Optional variable exported to the shared staging directory path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_staging_environment_variable: Option<String>,
}

impl Default for SlurmRuntimeContractConfig {
    fn default() -> Self {
        Self {
            required_commands: Vec::new(),
            required_python_modules: Vec::new(),
            required_environment: Vec::new(),
            output_subdirectory: "outputs".into(),
            shared_staging_subdirectory: "shared_staging".into(),
            output_environment_variable: None,
            shared_staging_environment_variable: None,
        }
    }
}

impl SlurmRuntimeContractConfig {
    pub fn validate(&self) -> Result<(), ProfileValidationError> {
        validate_slurm_runtime_contract(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SlurmRemoteDeploymentConfig {
    pub login_node: String,
    #[serde(default = "ssh_port")]
    pub ssh_port: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_user: Option<String>,
    /// Non-secret slot name for the SSH private key and known_hosts files.
    /// Profiles never store key material; workers resolve files from
    /// `BEAMPIPE_SSH_CREDENTIALS_DIR/<ssh_credential>/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_credential: Option<String>,
    pub account: String,
    pub home_dir: String,
    pub log_dir: String,
    #[serde(default = "exec_prefix")]
    pub exec_prefix: String,
    pub dlg_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modules: Option<String>,
    pub facility: String,
    #[serde(default = "job_duration")]
    pub job_duration_minutes: i32,
    #[serde(default = "one")]
    pub num_nodes: i32,
    #[serde(default = "one")]
    pub num_islands: i32,
    #[serde(default = "one")]
    pub verbose_level: i32,
    #[serde(default)]
    pub max_threads: i32,
    #[serde(default)]
    pub all_nics: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_ssl: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slurm_template: Option<String>,
    #[serde(default)]
    pub resources: SlurmResourceConfig,
    #[serde(default)]
    pub manager_topology: DaliugeManagerTopologyConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_setup: Option<String>,
    /// Typed, project-specific runtime requirements. This field is required so
    /// a Slurm profile cannot silently inherit a bundled project's runtime.
    pub runtime_contract: SlurmRuntimeContractConfig,
    /// Non-secret runtime inputs for the terminal output publisher. Slurm
    /// publishers hand their canonical receipt back through the authenticated
    /// SSH/SFTP control plane; no Core callback credential enters the graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<PublicationRuntimeConfig>,
}

impl SlurmRemoteDeploymentConfig {
    pub fn effective_nodes(&self) -> i32 {
        self.resources.nodes.unwrap_or(self.num_nodes)
    }

    pub fn effective_islands(&self) -> i32 {
        self.manager_topology.islands.unwrap_or(self.num_islands)
    }

    pub fn effective_wall_time_minutes(&self) -> i32 {
        self.resources
            .wall_time_minutes
            .unwrap_or(self.job_duration_minutes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeploymentProfile {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_module: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_executions: Option<i32>,
    pub translation: DaliugeTranslationConfig,
    pub deployment: DeploymentConfig,
}

#[derive(Debug, Error)]
pub enum ProfileValidationError {
    #[error("{0}")]
    Message(String),
}

impl DeploymentProfile {
    pub fn validate(&self) -> Result<(), ProfileValidationError> {
        if self.name.trim().is_empty()
            || self.name.len() > 50
            || !self
                .name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        {
            return Err(ProfileValidationError::Message(
                "name must be 1-50 ASCII letters, digits, '.', '_', or '-'".into(),
            ));
        }
        if self.translation.num_par < 1 {
            return Err(ProfileValidationError::Message(
                "translation.num_par must be >= 1".into(),
            ));
        }
        if self
            .max_concurrent_executions
            .is_some_and(|limit| limit < 1)
        {
            return Err(ProfileValidationError::Message(
                "max_concurrent_executions must be >= 1 when set".into(),
            ));
        }
        if self.translation.num_islands < 0 {
            return Err(ProfileValidationError::Message(
                "translation.num_islands must be >= 0".into(),
            ));
        }
        match &self.deployment {
            DeploymentConfig::RestRemote(dep) => {
                validate_port(dep.dim_port_for_tm, "deployment.dim_port_for_tm")?;
                validate_port(dep.deploy_port, "deployment.deploy_port")?;
                if dep
                    .deploy_host
                    .as_deref()
                    .is_none_or(|host| host.trim().is_empty())
                {
                    return Err(ProfileValidationError::Message(
                        "deployment.deploy_host is required".into(),
                    ));
                }
            }
            DeploymentConfig::SlurmRemote(dep) => {
                if dep.login_node.trim().is_empty() {
                    return Err(ProfileValidationError::Message(
                        "deployment.login_node is required".into(),
                    ));
                }
                if let Some(slot) = dep.ssh_credential.as_deref() {
                    validate_ssh_credential_name(slot)?;
                }
                validate_port(Some(dep.ssh_port), "deployment.ssh_port")?;
                if dep.dlg_root.trim().is_empty() {
                    return Err(ProfileValidationError::Message(
                        "deployment.dlg_root is required".into(),
                    ));
                }
                if dep.dlg_root.trim().trim_matches('/').is_empty() {
                    return Err(ProfileValidationError::Message(
                        "deployment.dlg_root must be a dedicated directory, not the remote filesystem root"
                            .into(),
                    ));
                }
                if dep.account.trim().is_empty() {
                    return Err(ProfileValidationError::Message(
                        "deployment.account is required".into(),
                    ));
                }
                if dep.facility.trim().is_empty() {
                    return Err(ProfileValidationError::Message(
                        "deployment.facility is required".into(),
                    ));
                }
                for (name, path) in [
                    ("deployment.home_dir", dep.home_dir.as_str()),
                    ("deployment.log_dir", dep.log_dir.as_str()),
                    ("deployment.dlg_root", dep.dlg_root.as_str()),
                ] {
                    if !path.starts_with('/') {
                        return Err(ProfileValidationError::Message(format!(
                            "{name} must be an absolute remote path"
                        )));
                    }
                }
                validate_positive(dep.effective_nodes(), "deployment.resources.nodes")?;
                validate_positive(
                    dep.effective_islands(),
                    "deployment.manager_topology.islands",
                )?;
                validate_positive(
                    dep.effective_wall_time_minutes(),
                    "deployment.resources.wall_time_minutes",
                )?;
                for (value, name) in [
                    (dep.resources.tasks, "deployment.resources.tasks"),
                    (
                        dep.resources.cpus_per_task,
                        "deployment.resources.cpus_per_task",
                    ),
                ] {
                    if let Some(value) = value {
                        validate_positive(value, name)?;
                    }
                }
                validate_optional_text(
                    dep.resources.partition.as_deref(),
                    "deployment.resources.partition",
                )?;
                validate_optional_text(
                    dep.resources.memory.as_deref(),
                    "deployment.resources.memory",
                )?;
                validate_optional_text(
                    dep.resources.constraint.as_deref(),
                    "deployment.resources.constraint",
                )?;
                validate_optional_text(
                    dep.resources.quality_of_service.as_deref(),
                    "deployment.resources.quality_of_service",
                )?;
                dep.runtime_contract.validate()?;
                if let Some(publication) = dep.publication.as_ref() {
                    publication.validate()?;
                }
            }
        }
        Ok(())
    }
}

fn validate_positive(value: i32, name: &str) -> Result<(), ProfileValidationError> {
    if value < 1 {
        return Err(ProfileValidationError::Message(format!(
            "{name} must be >= 1"
        )));
    }
    Ok(())
}

pub fn validate_ssh_credential_name(name: &str) -> Result<(), ProfileValidationError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 50
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || !trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(ProfileValidationError::Message(
            "deployment.ssh_credential must be 1-50 ASCII letters, digits, '.', '_', or '-'".into(),
        ));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, name: &str) -> Result<(), ProfileValidationError> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        return Err(ProfileValidationError::Message(format!(
            "{name} must not be empty when set"
        )));
    }
    Ok(())
}

fn validate_slurm_runtime_contract(
    contract: &SlurmRuntimeContractConfig,
) -> Result<(), ProfileValidationError> {
    validate_relative_subdirectory(
        &contract.output_subdirectory,
        "deployment.runtime_contract.output_subdirectory",
    )?;
    validate_relative_subdirectory(
        &contract.shared_staging_subdirectory,
        "deployment.runtime_contract.shared_staging_subdirectory",
    )?;
    if contract.output_subdirectory == contract.shared_staging_subdirectory {
        return Err(ProfileValidationError::Message(
            "deployment.runtime_contract output and shared staging subdirectories must differ"
                .into(),
        ));
    }

    validate_unique_values(
        &contract.required_commands,
        "deployment.runtime_contract.required_commands",
        |value| {
            !value.is_empty()
                && value.len() <= 255
                && !value.chars().any(char::is_control)
                && !value.chars().any(char::is_whitespace)
        },
    )?;
    validate_unique_values(
        &contract.required_python_modules,
        "deployment.runtime_contract.required_python_modules",
        valid_python_module,
    )?;

    let mut environment_names = HashSet::new();
    for requirement in &contract.required_environment {
        validate_environment_name(
            &requirement.name,
            "deployment.runtime_contract.required_environment.name",
        )?;
        if !environment_names.insert(requirement.name.as_str()) {
            return Err(ProfileValidationError::Message(format!(
                "deployment.runtime_contract.required_environment contains duplicate variable '{}'",
                requirement.name
            )));
        }
    }
    for (value, name) in [
        (
            contract.output_environment_variable.as_deref(),
            "deployment.runtime_contract.output_environment_variable",
        ),
        (
            contract.shared_staging_environment_variable.as_deref(),
            "deployment.runtime_contract.shared_staging_environment_variable",
        ),
    ] {
        if let Some(value) = value {
            validate_environment_name(value, name)?;
            if environment_names.contains(value) {
                return Err(ProfileValidationError::Message(format!(
                    "{name} must not duplicate a required environment variable"
                )));
            }
        }
    }
    if contract.output_environment_variable == contract.shared_staging_environment_variable
        && contract.output_environment_variable.is_some()
    {
        return Err(ProfileValidationError::Message(
            "deployment.runtime_contract output and shared staging environment variables must differ"
                .into(),
        ));
    }
    Ok(())
}

fn validate_relative_subdirectory(value: &str, name: &str) -> Result<(), ProfileValidationError> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || value != value.trim()
        || value.chars().any(char::is_control)
        || value.contains([',', ':'])
        || path.is_absolute()
        || path.components().any(|component| {
            !matches!(component, Component::Normal(_))
                || matches!(component, Component::Normal(part) if part.is_empty())
        })
    {
        return Err(ProfileValidationError::Message(format!(
            "{name} must be a non-empty relative path without traversal components"
        )));
    }
    Ok(())
}

fn validate_unique_values<F>(
    values: &[String],
    name: &str,
    valid: F,
) -> Result<(), ProfileValidationError>
where
    F: Fn(&str) -> bool,
{
    let mut seen = HashSet::new();
    for value in values {
        if !valid(value) {
            return Err(ProfileValidationError::Message(format!(
                "{name} contains invalid value '{value}'"
            )));
        }
        if !seen.insert(value.as_str()) {
            return Err(ProfileValidationError::Message(format!(
                "{name} contains duplicate value '{value}'"
            )));
        }
    }
    Ok(())
}

fn valid_python_module(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .next()
                    .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
                && part
                    .chars()
                    .all(|character| character == '_' || character.is_ascii_alphanumeric())
        })
}

fn validate_environment_name(value: &str, name: &str) -> Result<(), ProfileValidationError> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !value
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(ProfileValidationError::Message(format!(
            "{name} must be a POSIX environment variable name"
        )));
    }
    if RESERVED_SLURM_RUNTIME_ENVIRONMENT.contains(&value) {
        return Err(ProfileValidationError::Message(format!(
            "{name} uses reserved Core variable '{value}'"
        )));
    }
    Ok(())
}

fn validate_publication_environment_name(
    value: &str,
    name: &str,
) -> Result<(), ProfileValidationError> {
    if value.is_empty()
        || value.len() > 255
        || !value
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !value
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(ProfileValidationError::Message(format!(
            "{name} must be a POSIX environment variable name"
        )));
    }
    Ok(())
}

fn validate_port(v: Option<i32>, name: &str) -> Result<(), ProfileValidationError> {
    if let Some(port) = v {
        if !(1..=65535).contains(&port) {
            return Err(ProfileValidationError::Message(format!(
                "{name} must be 1-65535"
            )));
        }
    }
    Ok(())
}

fn one() -> i32 {
    1
}

fn default_true() -> bool {
    true
}
fn default_dim_port() -> Option<i32> {
    Some(8001)
}
fn ssh_port() -> i32 {
    22
}
fn exec_prefix() -> String {
    "srun -l".into()
}
fn job_duration() -> i32 {
    30
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rest_profiles_verify_tls_by_default() {
        let deployment: DeploymentConfig = serde_json::from_value(json!({
            "kind": "rest_remote",
            "deploy_host": "dim.example.org",
            "deploy_port": 8001
        }))
        .unwrap();
        let DeploymentConfig::RestRemote(rest) = deployment else {
            panic!("expected rest profile");
        };
        assert!(rest.verify_ssl);
    }

    #[test]
    fn rust_translation_default_matches_the_deserialization_default() {
        let rust_default = DaliugeTranslationConfig::default();
        let parsed: DaliugeTranslationConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(rust_default.num_par, 1);
        assert_eq!(rust_default.num_par, parsed.num_par);
        assert_eq!(rust_default.num_islands, parsed.num_islands);
    }

    #[test]
    fn rest_profile_requires_a_deployment_host() {
        let profile: DeploymentProfile = serde_json::from_value(json!({
            "name": "rest",
            "translation": {"num_par": 1},
            "deployment": {"kind": "rest_remote"}
        }))
        .unwrap();
        assert!(profile
            .validate()
            .unwrap_err()
            .to_string()
            .contains("deployment.deploy_host"));
    }

    #[test]
    fn rest_profile_rejects_unsupported_publication_delivery() {
        let error = serde_json::from_value::<DeploymentProfile>(json!({
            "name": "rest",
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "rest_remote",
                "deploy_host": "dim.example.org",
                "publication": {
                    "durable_destination_uri_environment": "BEAMPIPE_OUTPUT_DESTINATION_URI"
                }
            }
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `publication`"));
    }

    #[test]
    fn profile_rejects_zero_concurrency() {
        let profile: DeploymentProfile = serde_json::from_value(json!({
            "name": "setonix",
            "max_concurrent_executions": 0,
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "slurm_remote",
                "login_node": "setonix.example.org",
                "facility": "setonix",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/logs",
                "dlg_root": "/scratch/project/dlg",
                "runtime_contract": {
                    "output_subdirectory": "outputs",
                    "shared_staging_subdirectory": "shared_staging"
                }
            }
        }))
        .unwrap();
        assert!(profile
            .validate()
            .unwrap_err()
            .to_string()
            .contains("max_concurrent_executions"));
    }

    #[test]
    fn slurm_profile_rejects_unsafe_ssh_credential_names() {
        let profile: DeploymentProfile = serde_json::from_value(json!({
            "name": "setonix",
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "slurm_remote",
                "login_node": "setonix.example.org",
                "facility": "setonix",
                "ssh_credential": "../etc",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/logs",
                "dlg_root": "/scratch/project/dlg",
                "runtime_contract": {
                    "output_subdirectory": "outputs",
                    "shared_staging_subdirectory": "shared_staging"
                }
            }
        }))
        .unwrap();
        assert!(profile
            .validate()
            .unwrap_err()
            .to_string()
            .contains("ssh_credential"));
    }

    #[test]
    fn slurm_profile_rejects_remote_filesystem_root_as_dlg_root() {
        let profile: DeploymentProfile = serde_json::from_value(json!({
            "name": "setonix",
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "slurm_remote",
                "login_node": "setonix.example.org",
                "facility": "setonix",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/logs",
                "dlg_root": "////",
                "runtime_contract": {
                    "output_subdirectory": "outputs",
                    "shared_staging_subdirectory": "shared_staging"
                }
            }
        }))
        .unwrap();
        assert!(profile
            .validate()
            .unwrap_err()
            .to_string()
            .contains("dedicated directory"));
    }

    #[test]
    fn profile_schema_rejects_unknown_fields() {
        let error = serde_json::from_value::<DeploymentProfile>(json!({
            "name": "rest",
            "translation": {"num_par": 1, "typo": true},
            "deployment": {"kind": "rest_remote", "deploy_host": "dim"}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `typo`"));
    }

    #[test]
    fn slurm_profile_requires_an_explicit_runtime_contract() {
        let error = serde_json::from_value::<DeploymentProfile>(json!({
            "name": "generic-slurm",
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "slurm_remote",
                "login_node": "login.example.org",
                "facility": "generic",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/logs",
                "dlg_root": "/scratch/project/dlg"
            }
        }))
        .unwrap_err();
        assert!(error.to_string().contains("runtime_contract"));
    }

    #[test]
    fn generic_slurm_runtime_contract_has_no_project_requirements() {
        let profile: DeploymentProfile = serde_json::from_value(json!({
            "name": "generic-slurm",
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "slurm_remote",
                "login_node": "login.example.org",
                "facility": "generic",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/logs",
                "dlg_root": "/scratch/project/dlg",
                "runtime_contract": {
                    "output_subdirectory": "science-products",
                    "shared_staging_subdirectory": "archive-cache"
                }
            }
        }))
        .unwrap();
        profile.validate().unwrap();
        let DeploymentConfig::SlurmRemote(deployment) = profile.deployment else {
            panic!("expected Slurm profile");
        };
        assert!(deployment.runtime_contract.required_commands.is_empty());
        assert!(deployment
            .runtime_contract
            .required_python_modules
            .is_empty());
        assert!(deployment.runtime_contract.required_environment.is_empty());
    }

    #[test]
    fn runtime_contract_rejects_unsafe_names_and_paths() {
        let contract = SlurmRuntimeContractConfig {
            output_subdirectory: "../outside".into(),
            ..Default::default()
        };
        assert!(contract.validate().is_err());

        let contract = SlurmRuntimeContractConfig {
            required_python_modules: vec!["science; os.system('bad')".into()],
            ..Default::default()
        };
        assert!(contract.validate().is_err());

        let contract = SlurmRuntimeContractConfig {
            required_environment: vec![SlurmRuntimeEnvironmentRequirement {
                name: "NOT-A-VARIABLE".into(),
                kind: SlurmRuntimeEnvironmentKind::NonEmpty,
            }],
            ..Default::default()
        };
        assert!(contract.validate().is_err());

        let contract = SlurmRuntimeContractConfig {
            output_environment_variable: Some("BEAMPIPE_SLURM_ACCOUNT".into()),
            ..Default::default()
        };
        assert!(contract.validate().is_err());
    }

    #[test]
    fn publication_contract_names_only_the_non_secret_destination_input() {
        let profile: DeploymentProfile = serde_json::from_value(json!({
            "name": "generic-slurm",
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "slurm_remote",
                "login_node": "login.example.org",
                "facility": "generic",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/logs",
                "dlg_root": "/scratch/project/dlg",
                "runtime_contract": {
                    "output_subdirectory": "science-products",
                    "shared_staging_subdirectory": "archive-cache"
                },
                "publication": {
                    "durable_destination_uri_environment": "BEAMPIPE_OUTPUT_DESTINATION_URI"
                }
            }
        }))
        .unwrap();
        profile.validate().unwrap();
    }
    #[test]
    fn project_runtime_cannot_override_core_publisher_environment() {
        for name in [
            "BEAMPIPE_EXECUTION_ID",
            "BEAMPIPE_OUTPUT_DESTINATION_URI",
            "BEAMPIPE_OUTPUT_INVENTORY_HANDOFF_PATH",
        ] {
            let contract = SlurmRuntimeContractConfig {
                required_environment: vec![SlurmRuntimeEnvironmentRequirement {
                    name: name.into(),
                    kind: SlurmRuntimeEnvironmentKind::NonEmpty,
                }],
                ..Default::default()
            };
            assert!(contract
                .validate()
                .unwrap_err()
                .to_string()
                .contains("reserved Core variable"));
        }
    }
}

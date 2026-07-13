use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

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
    pub nodes: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub islands: Option<i32>,
    #[serde(default)]
    pub co_host_dim: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SlurmRemoteDeploymentConfig {
    pub login_node: String,
    #[serde(default = "ssh_port")]
    pub ssh_port: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_user: Option<String>,
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
    #[serde(default = "facility")]
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
    #[serde(default)]
    pub zerorun: bool,
    #[serde(default)]
    pub sleepncopy: bool,
    #[serde(default)]
    pub check_with_session: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_ssl: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slurm_template: Option<String>,
    #[serde(default)]
    pub resources: SlurmResourceConfig,
    #[serde(default)]
    pub manager_topology: DaliugeManagerTopologyConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_setup: Option<String>,
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
                validate_port(Some(dep.ssh_port), "deployment.ssh_port")?;
                if dep.dlg_root.trim().is_empty() {
                    return Err(ProfileValidationError::Message(
                        "deployment.dlg_root is required".into(),
                    ));
                }
                if dep.account.trim().is_empty() {
                    return Err(ProfileValidationError::Message(
                        "deployment.account is required".into(),
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
                if let Some(nodes) = dep.manager_topology.nodes {
                    validate_positive(nodes, "deployment.manager_topology.nodes")?;
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

fn validate_optional_text(value: Option<&str>, name: &str) -> Result<(), ProfileValidationError> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        return Err(ProfileValidationError::Message(format!(
            "{name} must not be empty when set"
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
fn facility() -> String {
    "setonix".into()
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
    fn profile_rejects_zero_concurrency() {
        let profile: DeploymentProfile = serde_json::from_value(json!({
            "name": "setonix",
            "max_concurrent_executions": 0,
            "translation": {"num_par": 1},
            "deployment": {
                "kind": "slurm_remote",
                "login_node": "setonix.example.org",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/logs",
                "dlg_root": "/scratch/project/dlg"
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
    fn profile_schema_rejects_unknown_fields() {
        let error = serde_json::from_value::<DeploymentProfile>(json!({
            "name": "rest",
            "translation": {"num_par": 1, "typo": true},
            "deployment": {"kind": "rest_remote", "deploy_host": "dim"}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `typo`"));
    }
}

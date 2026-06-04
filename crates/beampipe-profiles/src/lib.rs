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

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, ToSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeploymentConfig {
    RestRemote(RestRemoteDeploymentConfig),
    SlurmRemote(SlurmRemoteDeploymentConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
    pub verify_ssl: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, ToSchema)]
pub struct DeploymentProfile {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_module: Option<String>,
    #[serde(default)]
    pub is_default: bool,
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
        if self.name.trim().is_empty() || self.name.len() > 50 {
            return Err(ProfileValidationError::Message(
                "name must be 1-50 characters".into(),
            ));
        }
        if self.translation.num_par < 1 {
            return Err(ProfileValidationError::Message(
                "translation.num_par must be >= 1".into(),
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
            }
        }
        Ok(())
    }
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

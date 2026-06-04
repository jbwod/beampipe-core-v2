use crate::{DimClient, HttpDimClient, OrchestrationError, SlurmClient, SshSlurmClient};
use beampipe_profiles::{
    DeploymentConfig, RestRemoteDeploymentConfig, SlurmRemoteDeploymentConfig,
};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct CancelParams {
    pub scheduler_job_id: Option<String>,
    pub deployment: Value,
}

#[derive(Debug, Clone)]
pub struct CancelResult {
    pub cancelled: bool,
    pub reason: Option<String>,
}

pub async fn cancel_scheduler_session(
    params: CancelParams,
) -> Result<CancelResult, OrchestrationError> {
    let Some(scheduler_job_id) = params.scheduler_job_id else {
        return Ok(CancelResult {
            cancelled: false,
            reason: Some("no_scheduler_job_id".into()),
        });
    };
    let deployment = serde_json::from_value::<DeploymentConfig>(params.deployment)
        .map_err(|e| OrchestrationError::Backend(format!("invalid deployment profile: {e}")))?;
    match deployment {
        DeploymentConfig::RestRemote(rest) => cancel_rest(&scheduler_job_id, &rest).await,
        DeploymentConfig::SlurmRemote(slurm) => cancel_slurm(&scheduler_job_id, &slurm).await,
    }
}

async fn cancel_rest(
    session_id: &str,
    rest: &RestRemoteDeploymentConfig,
) -> Result<CancelResult, OrchestrationError> {
    let Some(dim_base) = rest_endpoint(rest) else {
        return Ok(CancelResult {
            cancelled: false,
            reason: Some("incomplete_profile".into()),
        });
    };
    let client = HttpDimClient::new(dim_base);
    match client.cancel(session_id).await {
        Ok(()) => Ok(CancelResult {
            cancelled: true,
            reason: None,
        }),
        Err(e) => Ok(CancelResult {
            cancelled: false,
            reason: Some(e.to_string()),
        }),
    }
}

async fn cancel_slurm(
    scheduler_job_id: &str,
    slurm: &SlurmRemoteDeploymentConfig,
) -> Result<CancelResult, OrchestrationError> {
    let client = SshSlurmClient {
        login_node: slurm.login_node.clone(),
        remote_user: slurm.remote_user.clone(),
        session_dir: slurm.log_dir.clone(),
        account: Some(slurm.account.clone()),
        ssh_port: slurm.ssh_port,
        dlg_root: slurm.dlg_root.clone(),
        deployment: Some(slurm.clone()),
    };
    match client.cancel(scheduler_job_id).await {
        Ok(()) => Ok(CancelResult {
            cancelled: true,
            reason: None,
        }),
        Err(e) => Ok(CancelResult {
            cancelled: false,
            reason: Some(e.to_string()),
        }),
    }
}

pub fn rest_endpoint(rest: &RestRemoteDeploymentConfig) -> Option<String> {
    let host = rest.deploy_host.as_deref()?.trim();
    if host.is_empty() {
        return None;
    }
    let port = rest.deploy_port.unwrap_or(8001);
    Some(crate::dim::dim_rest_http_base(host, port))
}

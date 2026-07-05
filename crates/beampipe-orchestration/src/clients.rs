use crate::dim::get_roots;
use crate::http_client::{build_http_client, HttpClientOptions};
use crate::slurm_deploy::{resolve_remote_user, submit_slurm_session, SlurmSubmitParams};
use crate::slurm_ssh::{query_slurm_states_batch, SlurmSshSession, SlurmTarget};
use crate::translator::{default_lg_name, partitioned_pgt_for_dlg_deploy};
use crate::{BackendPoll, DimClient, OrchestrationError, SlurmClient, TranslatorClient};
use async_trait::async_trait;
use beampipe_domain::{slurm, ExecutionStatus};
use beampipe_profiles::{DaliugeTranslationConfig, SlurmRemoteDeploymentConfig};
use serde_json::Value;
use std::time::Duration;

const DIM_TIMEOUT_CREATE_SECS: u64 = 30;
const DIM_TIMEOUT_APPEND_SECS: u64 = 60;
const DIM_TIMEOUT_DEPLOY_SECS: u64 = 30;
const DIM_TIMEOUT_POLL_SECS: u64 = 10;

#[derive(Debug, Clone, Default)]
pub struct TranslateConfig {
    pub algo: String,
    pub num_par: i32,
    pub num_islands: i32,
    pub dim_host: String,
    pub dim_port: i32,
    pub slurm_path: bool,
}

#[derive(Debug, Clone)]
pub struct HttpTranslatorClient {
    pub base_url: String,
    pub client: reqwest::Client,
}

impl HttpTranslatorClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_options(base_url, HttpClientOptions::translator_default())
    }

    pub fn with_options(base_url: impl Into<String>, options: HttpClientOptions) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: build_http_client(&options),
        }
    }

    pub fn from_translation(tm_url: Option<String>) -> Self {
        Self::new(tm_url.unwrap_or_else(|| "http://localhost:9000".into()))
    }
}

#[async_trait]
impl TranslatorClient for HttpTranslatorClient {
    async fn translate(
        &self,
        graph: Value,
        config: &TranslateConfig,
    ) -> Result<TranslatedGraph, OrchestrationError> {
        if config.slurm_path {
            return self.translate_slurm(graph, config).await;
        }
        self.translate_rest(graph, config).await
    }
}

impl HttpTranslatorClient {
    async fn translate_rest(
        &self,
        graph: Value,
        config: &TranslateConfig,
    ) -> Result<TranslatedGraph, OrchestrationError> {
        let lg_name = "beampipe.graph";
        let form = [
            ("lg_name", lg_name),
            ("json_data", &graph.to_string()),
            ("algo", config.algo.as_str()),
            ("num_par", &config.num_par.to_string()),
            ("num_islands", &config.num_islands.to_string()),
        ];
        let resp = self
            .client
            .post(format!("{}/gen_pgt", self.base_url))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                OrchestrationError::Backend(crate::format_service_request_error(
                    "TM",
                    &self.base_url,
                    "/gen_pgt",
                    e,
                ))
            })?;
        if !resp.status().is_success() {
            return Err(OrchestrationError::Backend(format!(
                "TM gen_pgt failed: HTTP {}",
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        let pgt_id = body
            .split("pgtName = \"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("beampipe1_pgt.graph")
            .to_string();
        let resp = self
            .client
            .get(format!("{}/gen_pg", self.base_url))
            .query(&[
                ("pgt_id", pgt_id.as_str()),
                ("dlg_mgr_host", config.dim_host.as_str()),
                ("dlg_mgr_port", &config.dim_port.to_string()),
            ])
            .send()
            .await
            .map_err(|e| {
                OrchestrationError::Backend(crate::format_service_request_error(
                    "TM",
                    &self.base_url,
                    "/gen_pg",
                    e,
                ))
            })?;
        if !resp.status().is_success() {
            return Err(OrchestrationError::Backend(format!(
                "TM gen_pg failed: HTTP {}",
                resp.status()
            )));
        }
        let pg_spec: Value = resp
            .json()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        let spec_vec = match pg_spec {
            Value::Array(arr) => arr,
            other => vec![other],
        };
        let roots = get_roots(&spec_vec);
        Ok(TranslatedGraph {
            pg_spec: spec_vec,
            roots,
            pgt_json: None,
        })
    }

    async fn translate_slurm(
        &self,
        graph: Value,
        config: &TranslateConfig,
    ) -> Result<TranslatedGraph, OrchestrationError> {
        let lg_name = default_lg_name();
        let num_partitions = config.num_par.max(1);
        let num_islands = if config.num_islands < 1 {
            1
        } else {
            config.num_islands
        };
        let lg_content = graph.to_string();
        let num_partitions_s = num_partitions.to_string();
        let num_islands_s = num_islands.to_string();
        let form = [
            ("lg_content", lg_content.as_str()),
            ("num_partitions", num_partitions_s.as_str()),
            ("num_islands", num_islands_s.as_str()),
            ("algorithm", config.algo.as_str()),
        ];
        let resp = self
            .client
            .post(format!("{}/unroll_and_partition", self.base_url))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&form)
            .send()
            .await
            .map_err(|e| {
                OrchestrationError::Backend(crate::format_service_request_error(
                    "TM",
                    &self.base_url,
                    "/unroll_and_partition",
                    e,
                ))
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(OrchestrationError::Backend(format!(
                "TM unroll_and_partition failed: HTTP {status} — {body}"
            )));
        }
        let raw: Value = resp
            .json()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        let pgt_json = partitioned_pgt_for_dlg_deploy(raw, lg_name);
        Ok(TranslatedGraph {
            pg_spec: Vec::new(),
            roots: Vec::new(),
            pgt_json: Some(pgt_json),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TranslatedGraph {
    pub pg_spec: Vec<Value>,
    pub roots: Vec<String>,
    pub pgt_json: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct HttpDimClient {
    pub base_url: String,
    pub client: reqwest::Client,
}

impl HttpDimClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_options(base_url, HttpClientOptions::dim_default())
    }

    pub fn with_options(base_url: impl Into<String>, options: HttpClientOptions) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: build_http_client(&options),
        }
    }
}

#[async_trait]
impl DimClient for HttpDimClient {
    async fn deploy(
        &self,
        session_id: &str,
        pg_spec: &[Value],
        roots: &[String],
    ) -> Result<Value, OrchestrationError> {
        let sid = urlencoding_path(session_id);
        self.client
            .post(format!("{}/api/sessions", self.base_url))
            .json(&serde_json::json!({"sessionId": session_id}))
            .timeout(Duration::from_secs(DIM_TIMEOUT_CREATE_SECS))
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        self.client
            .post(format!("{}/api/sessions/{sid}/graph/append", self.base_url))
            .json(pg_spec)
            .timeout(Duration::from_secs(DIM_TIMEOUT_APPEND_SECS))
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        let deploy_body = if roots.is_empty() {
            String::new()
        } else {
            format!("completed={}", roots.join(","))
        };
        self.client
            .post(format!("{}/api/sessions/{sid}/deploy", self.base_url))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(deploy_body)
            .timeout(Duration::from_secs(DIM_TIMEOUT_DEPLOY_SECS))
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        Ok(serde_json::json!({"session_id": session_id, "deployed": true}))
    }

    async fn poll(&self, session_id: &str) -> Result<BackendPoll, OrchestrationError> {
        let sid = urlencoding_path(session_id);
        let status: Value = self
            .client
            .get(format!("{}/api/sessions/{sid}/status", self.base_url))
            .timeout(Duration::from_secs(DIM_TIMEOUT_POLL_SECS))
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?
            .json()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        let mut st = crate::classify_dim_session_status(&status);
        let graph_status: Value = self
            .client
            .get(format!("{}/api/sessions/{sid}/graph/status", self.base_url))
            .timeout(Duration::from_secs(DIM_TIMEOUT_POLL_SECS))
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?
            .json()
            .await
            .unwrap_or(Value::Null);
        let error_uids = crate::dim_graph_status_error_uids(&graph_status);
        if st == ExecutionStatus::Completed && !error_uids.is_empty() {
            st = ExecutionStatus::Failed;
        }
        Ok(BackendPoll {
            status: st,
            poll_summary: serde_json::json!({
                "session_id": session_id,
                "status": status,
                "graph_status": graph_status,
                "error_drop_uids": error_uids,
            }),
        })
    }

    async fn cancel(&self, session_id: &str) -> Result<(), OrchestrationError> {
        let sid = urlencoding_path(session_id);
        self.client
            .post(format!("{}/api/sessions/{sid}/cancel", self.base_url))
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn destroy_session(&self, session_id: &str) -> Result<(), OrchestrationError> {
        let sid = urlencoding_path(session_id);
        self.client
            .delete(format!("{}/api/sessions/{sid}", self.base_url))
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?
            .error_for_status()
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SshSlurmClient {
    pub login_node: String,
    pub remote_user: Option<String>,
    pub session_dir: String,
    pub account: Option<String>,
    pub ssh_port: i32,
    pub dlg_root: String,
    pub deployment: Option<SlurmRemoteDeploymentConfig>,
}

#[async_trait]
impl SlurmClient for SshSlurmClient {
    async fn submit(
        &self,
        execution_id: &str,
        session_id: &str,
        pgt_json: Value,
    ) -> Result<String, OrchestrationError> {
        let deployment = self.deployment.clone().ok_or_else(|| {
            OrchestrationError::Backend("slurm deployment config required".into())
        })?;
        let username = resolve_remote_user(&deployment);
        let result = submit_slurm_session(SlurmSubmitParams {
            execution_id: execution_id.to_string(),
            session_id: session_id.to_string(),
            pgt_json,
            deployment,
            username,
        })
        .await?;
        Ok(result.composite_scheduler_job_id)
    }

    async fn poll(&self, scheduler_job_id: &str) -> Result<BackendPoll, OrchestrationError> {
        let parsed = slurm::parse_scheduler_job_id(scheduler_job_id);
        let slurm_id = if parsed.slurm_job_id.is_empty() {
            scheduler_job_id
                .rsplit(':')
                .next()
                .unwrap_or(scheduler_job_id)
                .to_string()
        } else {
            parsed.slurm_job_id
        };
        let username = self
            .remote_user
            .clone()
            .or_else(|| std::env::var("SLURM_REMOTE_USER").ok())
            .unwrap_or_else(|| "root".into());
        let deployment = self.deployment.clone().ok_or_else(|| {
            OrchestrationError::Backend("slurm deployment config required".into())
        })?;
        let target = SlurmTarget::from_deployment(&deployment, &username);
        let mut session = SlurmSshSession::connect(&target).await?;
        let results =
            query_slurm_states_batch(&mut session, std::slice::from_ref(&slurm_id)).await?;
        let _ = session.close().await;
        let result = results.get(&slurm_id).cloned().ok_or_else(|| {
            OrchestrationError::Backend(format!("no poll result for slurm job {slurm_id}"))
        })?;
        let normalized = result.normalized_state.clone();
        let status = match normalized.as_str() {
            "COMPLETED" => ExecutionStatus::Completed,
            "FAILED" | "TIMEOUT" => ExecutionStatus::Failed,
            "CANCELLED" => ExecutionStatus::Cancelled,
            "RUNNING" => ExecutionStatus::Running,
            "PENDING" => ExecutionStatus::AwaitingScheduler,
            _ => ExecutionStatus::AwaitingScheduler,
        };
        Ok(BackendPoll {
            status,
            poll_summary: serde_json::json!({
                "scheduler_job_id": scheduler_job_id,
                "normalized_state": normalized,
                "raw_state": result.raw_state,
                "slurm_job_id": slurm_id,
                "source": result.source,
                "exit_code": result.exit_code,
            }),
        })
    }

    async fn cancel(&self, scheduler_job_id: &str) -> Result<(), OrchestrationError> {
        let parsed = slurm::parse_scheduler_job_id(scheduler_job_id);
        let slurm_id = if parsed.slurm_job_id.is_empty() {
            scheduler_job_id
                .rsplit(':')
                .next()
                .unwrap_or(scheduler_job_id)
                .to_string()
        } else {
            parsed.slurm_job_id
        };
        let deployment = self.deployment.clone().ok_or_else(|| {
            OrchestrationError::Backend("slurm deployment config required".into())
        })?;
        let username = self
            .remote_user
            .clone()
            .or_else(|| std::env::var("SLURM_REMOTE_USER").ok())
            .unwrap_or_else(|| "root".into());
        let target = SlurmTarget::from_deployment(&deployment, &username);
        let mut session = SlurmSshSession::connect(&target).await?;
        session.run_command(&format!("scancel {slurm_id}")).await?;
        let _ = session.close().await;
        Ok(())
    }
}

pub fn translate_config_from_profile(
    translation: &DaliugeTranslationConfig,
    dim_host: Option<&str>,
    dim_port: Option<i32>,
    slurm_path: bool,
) -> TranslateConfig {
    TranslateConfig {
        algo: match translation.algo {
            beampipe_profiles::DaliugeAlgo::Metis => "metis".into(),
            beampipe_profiles::DaliugeAlgo::Mysarkar => "mysarkar".into(),
        },
        num_par: translation.num_par,
        num_islands: translation.num_islands,
        dim_host: dim_host.unwrap_or("localhost").to_string(),
        dim_port: dim_port.unwrap_or(8000),
        slurm_path,
    }
}

fn urlencoding_path(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

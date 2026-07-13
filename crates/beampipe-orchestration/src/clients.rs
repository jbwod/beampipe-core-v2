use crate::daliuge::{
    checked_empty, checked_json, DaliugeCapabilities, DaliugeClientError, DaliugeComponent,
    DaliugeManager, DaliugeManagerInfo, DaliugeSessionObservation, DaliugeSessionSummary,
    DaliugeTranslator, DaliugeTranslatorInfo,
};
use crate::dim::get_roots;
use crate::http_client::{build_http_client, HttpClientOptions};
use crate::slurm_deploy::{resolve_remote_user, submit_slurm_session, SlurmSubmitParams};
use crate::slurm_ssh::{query_slurm_states_batch, SlurmSshSession, SlurmTarget};
use crate::translator::{default_lg_name, partitioned_pgt_for_dlg_deploy};
use crate::{BackendPoll, DimClient, OrchestrationError, SlurmClient, TranslatorClient};
use async_trait::async_trait;
use beampipe_domain::{slurm, ExecutionStatus};
use beampipe_profiles::{DaliugeTranslationConfig, SlurmRemoteDeploymentConfig};
use serde::Deserialize;
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
    async fn unroll_and_partition(
        &self,
        graph: Value,
        config: &TranslateConfig,
    ) -> Result<Value, OrchestrationError> {
        let endpoint = format!("{}/unroll_and_partition", self.base_url);
        let lg_content = graph.to_string();
        let num_partitions = config.num_par.max(1).to_string();
        let num_islands = config.num_islands.max(1).to_string();
        let form = [
            ("lg_content", lg_content.as_str()),
            ("num_partitions", num_partitions.as_str()),
            ("num_islands", num_islands.as_str()),
            ("algorithm", config.algo.as_str()),
        ];
        let response = self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&form)
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::Translator,
                    "unroll_and_partition",
                    &endpoint,
                    error,
                )
            })?;
        Ok(checked_json(
            response,
            DaliugeComponent::Translator,
            "unroll_and_partition",
            &endpoint,
        )
        .await?)
    }

    async fn translate_rest(
        &self,
        graph: Value,
        config: &TranslateConfig,
    ) -> Result<TranslatedGraph, OrchestrationError> {
        if config.dim_host.trim().is_empty() || !(1..=65535).contains(&config.dim_port) {
            return Err(DaliugeClientError::compatibility(
                DaliugeComponent::Translator,
                "map",
                &self.base_url,
                "updated DALiuGE mapping requires a valid DIM host and port",
            )
            .into());
        }
        let pgt = self.unroll_and_partition(graph, config).await?;
        if !pgt.is_array() {
            return Err(DaliugeClientError::invalid_response(
                DaliugeComponent::Translator,
                "unroll_and_partition",
                &self.base_url,
                "expected a physical graph template array",
            )
            .into());
        }
        let endpoint = format!("{}/map", self.base_url);
        let pgt_content = pgt.to_string();
        let dim_port = config.dim_port.to_string();
        let num_islands = config.num_islands.max(1).to_string();
        let form = [
            ("pgt_content", pgt_content.as_str()),
            ("host", config.dim_host.as_str()),
            ("port", dim_port.as_str()),
            ("num_islands", num_islands.as_str()),
            ("co_host_dim", "true"),
        ];
        let response = self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&form)
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(DaliugeComponent::Translator, "map", &endpoint, error)
            })?;
        let pg_spec: Value =
            checked_json(response, DaliugeComponent::Translator, "map", &endpoint).await?;
        let spec_vec = match pg_spec {
            Value::Array(items) => items,
            _ => {
                return Err(DaliugeClientError::invalid_response(
                    DaliugeComponent::Translator,
                    "map",
                    &endpoint,
                    "expected a mapped physical graph array",
                )
                .into())
            }
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
        let raw = self.unroll_and_partition(graph, config).await?;
        let pgt_json = partitioned_pgt_for_dlg_deploy(raw, default_lg_name());
        Ok(TranslatedGraph {
            pg_spec: Vec::new(),
            roots: Vec::new(),
            pgt_json: Some(pgt_json),
        })
    }
}

#[derive(Debug, Deserialize)]
struct SubmissionMethodsResponse {
    #[serde(default)]
    methods: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ManagerRootResponse {
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default, rename = "sessionIds")]
    _session_ids: Vec<String>,
}

#[async_trait]
impl DaliugeTranslator for HttpTranslatorClient {
    async fn inspect(
        &self,
        manager_host: Option<&str>,
        manager_port: Option<i32>,
    ) -> Result<DaliugeTranslatorInfo, DaliugeClientError> {
        let endpoint = format!("{}/api/submission_method", self.base_url);
        let mut request = self.client.get(&endpoint);
        if let (Some(host), Some(port)) = (manager_host, manager_port) {
            request = request.query(&[
                ("dlg_mgr_host", host.to_string()),
                ("dlg_mgr_port", port.to_string()),
            ]);
        }
        let response = request.send().await.map_err(|error| {
            DaliugeClientError::request(DaliugeComponent::Translator, "inspect", &endpoint, error)
        })?;
        let methods: SubmissionMethodsResponse =
            checked_json(response, DaliugeComponent::Translator, "inspect", &endpoint).await?;
        Ok(DaliugeTranslatorInfo {
            endpoint: self.base_url.clone(),
            version: None,
            capabilities: DaliugeCapabilities {
                updated_translation_api: true,
                submission_methods: methods.methods,
                ..Default::default()
            },
            diagnostics: vec![beampipe_domain::Diagnostic::warning(
                "version",
                "daliuge.version_unreported",
                "the Translator Manager capability response does not report a version",
            )
            .with_hint(
                "record the deployed DALiuGE package/image version in the deployment profile",
            )],
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
        let create_endpoint = format!("{}/api/sessions", self.base_url);
        let response = self
            .client
            .post(&create_endpoint)
            .json(&serde_json::json!({"sessionId": session_id}))
            .timeout(Duration::from_secs(DIM_TIMEOUT_CREATE_SECS))
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "create_session",
                    &create_endpoint,
                    error,
                )
            })?;
        checked_empty(
            response,
            DaliugeComponent::DataIslandManager,
            "create_session",
            &create_endpoint,
        )
        .await?;

        let append_endpoint = format!("{}/api/sessions/{sid}/graph/append", self.base_url);
        let response = self
            .client
            .post(&append_endpoint)
            .json(pg_spec)
            .timeout(Duration::from_secs(DIM_TIMEOUT_APPEND_SECS))
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "append_graph",
                    &append_endpoint,
                    error,
                )
            })?;
        checked_empty(
            response,
            DaliugeComponent::DataIslandManager,
            "append_graph",
            &append_endpoint,
        )
        .await?;

        let deploy_body = if roots.is_empty() {
            String::new()
        } else {
            format!("completed={}", roots.join(","))
        };
        let deploy_endpoint = format!("{}/api/sessions/{sid}/deploy", self.base_url);
        let response = self
            .client
            .post(&deploy_endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(deploy_body)
            .timeout(Duration::from_secs(DIM_TIMEOUT_DEPLOY_SECS))
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "deploy_session",
                    &deploy_endpoint,
                    error,
                )
            })?;
        checked_empty(
            response,
            DaliugeComponent::DataIslandManager,
            "deploy_session",
            &deploy_endpoint,
        )
        .await?;
        Ok(serde_json::json!({"session_id": session_id, "deployed": true}))
    }

    async fn poll(&self, session_id: &str) -> Result<BackendPoll, OrchestrationError> {
        let sid = urlencoding_path(session_id);
        let observation = self.session_observation(session_id).await?;
        let mut status = observation.state.execution_status();

        let graph_endpoint = format!("{}/api/sessions/{sid}/graph/status", self.base_url);
        let response = self
            .client
            .get(&graph_endpoint)
            .timeout(Duration::from_secs(DIM_TIMEOUT_POLL_SECS))
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "graph_status",
                    &graph_endpoint,
                    error,
                )
            })?;
        let graph_status: Value = checked_json(
            response,
            DaliugeComponent::DataIslandManager,
            "graph_status",
            &graph_endpoint,
        )
        .await?;
        let error_uids = crate::dim_graph_status_error_uids(&graph_status);
        if status == ExecutionStatus::Completed && !error_uids.is_empty() {
            status = ExecutionStatus::Failed;
        }
        Ok(BackendPoll {
            status,
            poll_summary: serde_json::json!({
                "session_id": session_id,
                "status": observation.raw,
                "normalized_session_state": observation.state,
                "per_node": observation.per_node,
                "observed_at": observation.observed_at,
                "graph_status": graph_status,
                "error_drop_uids": error_uids,
            }),
        })
    }

    async fn cancel(&self, session_id: &str) -> Result<(), OrchestrationError> {
        let sid = urlencoding_path(session_id);
        let endpoint = format!("{}/api/sessions/{sid}/cancel", self.base_url);
        let response = self.client.post(&endpoint).send().await.map_err(|error| {
            DaliugeClientError::request(
                DaliugeComponent::DataIslandManager,
                "cancel_session",
                &endpoint,
                error,
            )
        })?;
        checked_empty(
            response,
            DaliugeComponent::DataIslandManager,
            "cancel_session",
            &endpoint,
        )
        .await?;
        Ok(())
    }

    async fn destroy_session(&self, session_id: &str) -> Result<(), OrchestrationError> {
        let sid = urlencoding_path(session_id);
        let endpoint = format!("{}/api/sessions/{sid}", self.base_url);
        let response = self
            .client
            .delete(&endpoint)
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "delete_session",
                    &endpoint,
                    error,
                )
            })?;
        checked_empty(
            response,
            DaliugeComponent::DataIslandManager,
            "delete_session",
            &endpoint,
        )
        .await?;
        Ok(())
    }
}

#[async_trait]
impl DaliugeManager for HttpDimClient {
    async fn inspect(&self) -> Result<DaliugeManagerInfo, DaliugeClientError> {
        let root_endpoint = format!("{}/api", self.base_url);
        let response = self
            .client
            .get(&root_endpoint)
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "inspect",
                    &root_endpoint,
                    error,
                )
            })?;
        let root: ManagerRootResponse = checked_json(
            response,
            DaliugeComponent::DataIslandManager,
            "inspect",
            &root_endpoint,
        )
        .await?;

        let nodes_endpoint = format!("{}/api/nodes", self.base_url);
        let response = self
            .client
            .get(&nodes_endpoint)
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "list_nodes",
                    &nodes_endpoint,
                    error,
                )
            })?;
        let nodes: Vec<String> = checked_json(
            response,
            DaliugeComponent::DataIslandManager,
            "list_nodes",
            &nodes_endpoint,
        )
        .await?;
        let sessions = self.sessions().await?;

        Ok(DaliugeManagerInfo {
            endpoint: self.base_url.clone(),
            version: None,
            hosts: root.hosts,
            nodes,
            sessions,
            capabilities: DaliugeCapabilities {
                session_api: true,
                manager_topology: true,
                session_logs: true,
                ..Default::default()
            },
            diagnostics: vec![beampipe_domain::Diagnostic::warning(
                "version",
                "daliuge.version_unreported",
                "the Data Island Manager API does not report a version",
            )
            .with_hint(
                "record the deployed DALiuGE package/image version in the deployment profile",
            )],
        })
    }

    async fn sessions(&self) -> Result<Vec<DaliugeSessionSummary>, DaliugeClientError> {
        let endpoint = format!("{}/api/sessions", self.base_url);
        let response = self.client.get(&endpoint).send().await.map_err(|error| {
            DaliugeClientError::request(
                DaliugeComponent::DataIslandManager,
                "list_sessions",
                &endpoint,
                error,
            )
        })?;
        checked_json(
            response,
            DaliugeComponent::DataIslandManager,
            "list_sessions",
            &endpoint,
        )
        .await
    }

    async fn session_observation(
        &self,
        session_id: &str,
    ) -> Result<DaliugeSessionObservation, DaliugeClientError> {
        let sid = urlencoding_path(session_id);
        let endpoint = format!("{}/api/sessions/{sid}/status", self.base_url);
        let response = self
            .client
            .get(&endpoint)
            .timeout(Duration::from_secs(DIM_TIMEOUT_POLL_SECS))
            .send()
            .await
            .map_err(|error| {
                DaliugeClientError::request(
                    DaliugeComponent::DataIslandManager,
                    "session_status",
                    &endpoint,
                    error,
                )
            })?;
        let raw: Value = checked_json(
            response,
            DaliugeComponent::DataIslandManager,
            "session_status",
            &endpoint,
        )
        .await?;
        Ok(DaliugeSessionObservation::from_raw(raw))
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

use async_trait::async_trait;
use beampipe_domain::{slurm, ExecutionStatus};
use chrono::{DateTime, Utc};
use serde_json::Value;
use thiserror::Error;

pub mod cancel;
pub mod clients;
pub mod daliuge;
pub mod dim;
pub mod graph;
pub mod http_client;
pub mod manifest;
pub mod scheduler;
pub mod security;
pub mod slurm_batch;
pub mod slurm_credentials;
pub mod slurm_deploy;
pub mod slurm_ssh;
pub mod staging;
pub mod tm_health;
pub mod translator;

pub use cancel::{cancel_scheduler_session, CancelParams, CancelResult};
pub use clients::{
    translate_config_from_profile, HttpDimClient, HttpTranslatorClient, SshSlurmClient,
    TranslateConfig, TranslatedGraph,
};
pub use daliuge::{
    DaliugeCapabilities, DaliugeClientError, DaliugeComponent, DaliugeErrorKind, DaliugeManager,
    DaliugeManagerInfo, DaliugeSessionObservation, DaliugeSessionState, DaliugeSessionSummary,
    DaliugeTranslator, DaliugeTranslatorInfo,
};
pub use graph::resolve_graph;
pub use http_client::{build_http_client, HttpClientOptions};
pub use manifest::{
    apply_project_graph_patches, build_manifest_from_config,
    build_manifest_from_config_with_staging,
};
pub use scheduler::{
    SchedulerAdapter, SchedulerAdapterError, SchedulerCapacity, SchedulerConnectivity,
    SchedulerErrorKind, SchedulerJobObservation, SchedulerKind, SchedulerLogLocations,
    SchedulerQueueInfo, SchedulerResourceRequest, SchedulerSubmission, SchedulerSubmissionRequest,
};
pub use security::{collect_security_issues, validate_security};
pub use slurm_batch::SlurmJobPollResult;
pub use slurm_credentials::{beampipe_env, is_production_env, SlurmSshCredentials};
pub use slurm_deploy::probe_slurm_login;
pub use slurm_ssh::{query_slurm_states_batch, SlurmSshPool, SlurmSshSession, SlurmTarget};
pub use staging::CasdaStagingClient;
pub use tm_health::{
    dim_unreachable_message, format_service_request_error, probe_dim_reachable, probe_tm_reachable,
    tm_unreachable_message, TmProbeResult,
};
pub use translator::{partitioned_pgt_for_dlg_deploy, pgt_filename_from_lg_name};

pub const ACTIVE_CONFIG_KEY: &str = "activeGraphConfigId";
pub const GRAPH_CONFIGS: &str = "graphConfigurations";
pub const GRAPH_NODES: &str = "nodeDataArray";
pub const GRAPH_FIELDS: &str = "fields";
pub const BEAMPIPE_INGEST_NODE_NAME: &str = "beampipe-ingest";
pub const MANIFEST_PATH_FIELD_NAME: &str = "manifest_path";

#[derive(Debug, Error)]
pub enum OrchestrationError {
    #[error("graph is not a JSON object")]
    GraphNotObject,
    #[error("manifest has no usable datasets")]
    NoUsableDatasets,
    #[error("graph patch target node not found: {0}")]
    GraphPatchNodeNotFound(String),
    #[error("graph patch field not found on node {node}: {field}")]
    GraphPatchFieldNotFound { node: String, field: String },
    #[error("backend error: {0}")]
    Backend(String),
    #[error(transparent)]
    Daliuge(#[from] DaliugeClientError),
}

#[derive(Debug, Clone, Default)]
pub struct StageOutcome {
    pub metadata: Vec<Value>,
    pub skipped_sbids: Vec<String>,
    pub staged_count: usize,
    pub staged_urls_by_scan_id: std::collections::HashMap<String, String>,
    pub checksum_urls_by_scan_id: std::collections::HashMap<String, String>,
    pub eval_urls_by_sbid: std::collections::HashMap<String, String>,
    pub eval_checksum_urls_by_sbid: std::collections::HashMap<String, String>,
}

#[async_trait]
pub trait StagingClient: Send + Sync {
    async fn stage(&self, metadata: &[Value]) -> Result<StageOutcome, OrchestrationError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PassThroughStagingClient;

#[async_trait]
impl StagingClient for PassThroughStagingClient {
    async fn stage(&self, metadata: &[Value]) -> Result<StageOutcome, OrchestrationError> {
        Ok(StageOutcome {
            metadata: metadata.to_vec(),
            skipped_sbids: Vec::new(),
            staged_count: metadata.len(),
            ..Default::default()
        })
    }
}

pub trait ManifestBuilder: Send + Sync {
    fn build_manifest(&self, metadata: &[Value]) -> Result<Value, OrchestrationError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WallabyManifestBuilder;

impl ManifestBuilder for WallabyManifestBuilder {
    fn build_manifest(&self, metadata: &[Value]) -> Result<Value, OrchestrationError> {
        build_wallaby_manifest(metadata)
    }
}

pub trait GraphResolver: Send + Sync {
    fn resolve_graph(&self) -> Result<Value, OrchestrationError>;
}

#[derive(Debug, Clone)]
pub struct StaticGraphResolver {
    pub graph: Value,
}

impl GraphResolver for StaticGraphResolver {
    fn resolve_graph(&self) -> Result<Value, OrchestrationError> {
        Ok(self.graph.clone())
    }
}

#[derive(Debug, Clone)]
pub struct BackendSubmit {
    pub scheduler_name: String,
    pub scheduler_job_id: Option<String>,
    pub session_id: Option<String>,
    pub remote_session_dir: Option<String>,
    pub physical_graph: Option<Value>,
    pub workflow_manifest: Value,
    pub next_status: ExecutionStatus,
}

#[derive(Debug, Clone)]
pub struct BackendPoll {
    pub status: ExecutionStatus,
    pub poll_summary: Value,
}

#[async_trait]
pub trait TranslatorClient: Send + Sync {
    async fn translate(
        &self,
        graph: Value,
        config: &clients::TranslateConfig,
    ) -> Result<clients::TranslatedGraph, OrchestrationError>;
}

#[async_trait]
pub trait DimClient: Send + Sync {
    async fn deploy(
        &self,
        session_id: &str,
        pg_spec: &[Value],
        roots: &[String],
    ) -> Result<Value, OrchestrationError>;
    async fn poll(&self, session_id: &str) -> Result<BackendPoll, OrchestrationError>;
    async fn cancel(&self, session_id: &str) -> Result<(), OrchestrationError>;
    async fn destroy_session(&self, session_id: &str) -> Result<(), OrchestrationError>;
}

#[async_trait]
pub trait SlurmClient: Send + Sync {
    async fn submit(
        &self,
        execution_id: &str,
        session_id: &str,
        pgt_json: Value,
    ) -> Result<String, OrchestrationError>;
    async fn poll(&self, scheduler_job_id: &str) -> Result<BackendPoll, OrchestrationError>;
    async fn cancel(&self, scheduler_job_id: &str) -> Result<(), OrchestrationError>;
}

#[async_trait]
pub trait ExecutionBackend: Send + Sync {
    async fn submit(
        &self,
        execution_id: &str,
        manifest: Value,
        graph: Value,
    ) -> Result<BackendSubmit, OrchestrationError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MockTranslatorClient;

#[async_trait]
impl TranslatorClient for MockTranslatorClient {
    async fn translate(
        &self,
        graph: Value,
        _config: &clients::TranslateConfig,
    ) -> Result<clients::TranslatedGraph, OrchestrationError> {
        Ok(clients::TranslatedGraph {
            pg_spec: vec![graph],
            roots: Vec::new(),
            pgt_json: None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MockDimClient;

#[async_trait]
impl DimClient for MockDimClient {
    async fn deploy(
        &self,
        session_id: &str,
        pg_spec: &[Value],
        roots: &[String],
    ) -> Result<Value, OrchestrationError> {
        Ok(serde_json::json!({
            "session_id": session_id,
            "pg_spec": pg_spec,
            "roots": roots,
        }))
    }

    async fn poll(&self, session_id: &str) -> Result<BackendPoll, OrchestrationError> {
        Ok(BackendPoll {
            status: ExecutionStatus::Completed,
            poll_summary: serde_json::json!({"session_id": session_id, "state": "FINISHED"}),
        })
    }

    async fn cancel(&self, _session_id: &str) -> Result<(), OrchestrationError> {
        Ok(())
    }

    async fn destroy_session(&self, _session_id: &str) -> Result<(), OrchestrationError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MockSlurmClient;

#[async_trait]
impl SlurmClient for MockSlurmClient {
    async fn submit(
        &self,
        _execution_id: &str,
        session_id: &str,
        _pgt_json: Value,
    ) -> Result<String, OrchestrationError> {
        Ok(format!("{session_id}:0001|/tmp/beampipe"))
    }

    async fn poll(&self, scheduler_job_id: &str) -> Result<BackendPoll, OrchestrationError> {
        Ok(BackendPoll {
            status: ExecutionStatus::Completed,
            poll_summary: serde_json::json!({
                "scheduler_job_id": scheduler_job_id,
                "normalized_state": slurm::normalize_state("COMPLETED"),
            }),
        })
    }

    async fn cancel(&self, _scheduler_job_id: &str) -> Result<(), OrchestrationError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RestExecutionBackend<T = MockTranslatorClient, D = MockDimClient> {
    pub translator: T,
    pub dim: D,
    pub profile_name: Option<String>,
    pub tm_url: Option<String>,
    pub dim_endpoint: Option<String>,
    pub translate_config: clients::TranslateConfig,
    pub session_created_at: DateTime<Utc>,
}

impl Default for RestExecutionBackend {
    fn default() -> Self {
        Self {
            translator: MockTranslatorClient,
            dim: MockDimClient,
            profile_name: None,
            tm_url: None,
            dim_endpoint: None,
            translate_config: clients::TranslateConfig::default(),
            session_created_at: Utc::now(),
        }
    }
}

#[async_trait]
impl<T, D> ExecutionBackend for RestExecutionBackend<T, D>
where
    T: TranslatorClient,
    D: DimClient,
{
    async fn submit(
        &self,
        execution_id: &str,
        manifest: Value,
        graph: Value,
    ) -> Result<BackendSubmit, OrchestrationError> {
        let session_id = beampipe_session_id(execution_id, self.session_created_at);
        let translated = self
            .translator
            .translate(graph, &self.translate_config)
            .await?;
        let _deploy = self
            .dim
            .deploy(&session_id, &translated.pg_spec, &translated.roots)
            .await?;
        let dim_base = self.dim_endpoint.clone().unwrap_or_default();
        let operator_urls = if dim_base.is_empty() {
            None
        } else {
            Some(
                dim::dim_operator_urls_from_base(&dim_base, &session_id)
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                            .collect::<std::collections::HashMap<_, _>>()
                    })
                    .unwrap_or_default(),
            )
        };
        let workflow_manifest = beampipe_domain::run_record::merge_dim_deploy_into_manifest(
            Some(manifest),
            &session_id,
            &dim_base,
            false,
            operator_urls,
        );
        Ok(BackendSubmit {
            scheduler_name: "daliuge".into(),
            scheduler_job_id: Some(session_id.clone()),
            session_id: Some(session_id),
            remote_session_dir: None,
            physical_graph: Some(Value::Array(translated.pg_spec)),
            workflow_manifest,
            next_status: ExecutionStatus::Running,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SlurmExecutionBackend<T = MockTranslatorClient, S = MockSlurmClient> {
    pub translator: T,
    pub slurm: S,
    pub profile_name: Option<String>,
    pub session_dir: String,
    pub login_node: Option<String>,
    pub remote_user: Option<String>,
    pub account: Option<String>,
    pub translate_config: clients::TranslateConfig,
    pub session_created_at: DateTime<Utc>,
}

impl Default for SlurmExecutionBackend {
    fn default() -> Self {
        Self {
            translator: MockTranslatorClient,
            slurm: MockSlurmClient,
            profile_name: None,
            session_dir: "/tmp/beampipe".into(),
            login_node: None,
            remote_user: None,
            account: None,
            translate_config: clients::TranslateConfig {
                slurm_path: true,
                ..Default::default()
            },
            session_created_at: Utc::now(),
        }
    }
}

#[async_trait]
impl<T, S> ExecutionBackend for SlurmExecutionBackend<T, S>
where
    T: TranslatorClient,
    S: SlurmClient,
{
    async fn submit(
        &self,
        execution_id: &str,
        manifest: Value,
        graph: Value,
    ) -> Result<BackendSubmit, OrchestrationError> {
        let session_id = beampipe_session_id(execution_id, self.session_created_at);
        let translated = self
            .translator
            .translate(graph, &self.translate_config)
            .await?;
        let pgt_json = translated.pgt_json.ok_or_else(|| {
            OrchestrationError::Backend("slurm translate missing pgt_json".into())
        })?;
        let physical_graph = pgt_json.clone();
        let scheduler_job_id = self
            .slurm
            .submit(execution_id, &session_id, pgt_json)
            .await?;
        let slurm_job_id = {
            let parsed = slurm::parse_scheduler_job_id(&scheduler_job_id);
            if parsed.slurm_job_id.is_empty() {
                scheduler_job_id.clone()
            } else {
                parsed.slurm_job_id
            }
        };
        let workflow_manifest = beampipe_domain::run_record::merge_slurm_submit_into_manifest(
            Some(manifest),
            &session_id,
            &slurm_job_id,
            &scheduler_job_id,
            self.login_node.as_deref(),
            self.remote_user.as_deref(),
        );
        let remote_session_dir = slurm::parse_scheduler_job_id(&scheduler_job_id).session_dir;
        Ok(BackendSubmit {
            scheduler_name: "slurm".into(),
            scheduler_job_id: Some(scheduler_job_id),
            session_id: Some(session_id),
            remote_session_dir,
            physical_graph: Some(physical_graph),
            workflow_manifest,
            next_status: ExecutionStatus::AwaitingScheduler,
        })
    }
}

pub fn beampipe_session_id(execution_id: &str, created_at: DateTime<Utc>) -> String {
    format!(
        "BeampipeExecution-{}-{}",
        execution_id,
        created_at.format("%Y-%m-%dT%H-%M-%S")
    )
}

pub fn build_wallaby_manifest(metadata: &[Value]) -> Result<Value, OrchestrationError> {
    let mut by_source: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, Vec<Value>>,
    > = std::collections::BTreeMap::new();
    for record in metadata {
        let source = record
            .get("source_identifier")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let sbid = record
            .get("sbid")
            .map(value_key)
            .filter(|v| v != "0")
            .unwrap_or_default();
        if sbid.is_empty() {
            continue;
        }
        by_source
            .entry(source)
            .or_default()
            .entry(sbid)
            .or_default()
            .push(record.clone());
    }
    let mut sources = Vec::new();
    let mut total_datasets = 0_usize;
    for (source_identifier, by_sbid) in by_source {
        let first = by_sbid
            .values()
            .flatten()
            .next()
            .cloned()
            .unwrap_or(Value::Null);
        let mut sbids = Vec::new();
        for (sbid, datasets) in by_sbid {
            total_datasets += datasets.len();
            sbids.push(serde_json::json!({
                "sbid": sbid,
                "datasets": datasets,
            }));
        }
        sources.push(serde_json::json!({
            "source_identifier": source_identifier,
            "ra_string": first.get("ra_string").cloned().unwrap_or(Value::Null),
            "dec_string": first.get("dec_string").cloned().unwrap_or(Value::Null),
            "vsys": first.get("vsys").cloned().unwrap_or(Value::Null),
            "sbids": sbids,
        }));
    }
    if total_datasets == 0 {
        return Err(OrchestrationError::NoUsableDatasets);
    }
    let mut manifest = serde_json::json!({"inputs": {}, "sources": sources});
    manifest["graph_overrides"] = serde_json::json!({
        "patches": [{
            "match": {"equals": "Scatter/GenericScatterApp/Beam"},
            "fields": [{"name": "num_of_copies", "value": total_datasets}]
        }]
    });
    Ok(manifest)
}

pub fn resolve_beampipe_ingest_uuids(graph: &Value) -> Option<(String, String)> {
    let nodes = graph.get(GRAPH_NODES)?.as_array()?;
    for node in nodes.iter().filter_map(Value::as_object) {
        if node.get("name").and_then(Value::as_str) != Some(BEAMPIPE_INGEST_NODE_NAME) {
            continue;
        }
        let node_id = node.get("id").and_then(Value::as_str)?.to_string();
        for field in node.get(GRAPH_FIELDS)?.as_array()? {
            let field = field.as_object()?;
            if field.get("name").and_then(Value::as_str) == Some(MANIFEST_PATH_FIELD_NAME) {
                let field_id = field.get("id").and_then(Value::as_str)?.to_string();
                return Some((node_id, field_id));
            }
        }
        return None;
    }
    None
}

/// Embed manifest JSON into `graphConfigurations` and set `activeGraphConfigId` (Python parity).
pub fn inject_manifest_config_into_graph(
    graph: &mut Value,
    manifest: &Value,
    config_id: Option<&str>,
) -> Result<(), OrchestrationError> {
    let Some((node_id, field_id)) = resolve_beampipe_ingest_uuids(graph) else {
        return Ok(());
    };

    let mut embed_content = serde_json::Map::new();
    if let Some(obj) = manifest.as_object() {
        for (key, value) in obj {
            if key == "graph_overrides" {
                continue;
            }
            embed_content.insert(key.clone(), value.clone());
        }
    }
    let value_str = serde_json::to_string(&Value::Object(embed_content))
        .map_err(|e| OrchestrationError::Backend(e.to_string()))?;

    let config_id = config_id
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let num_lg_nodes = graph
        .get(GRAPH_NODES)
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let now = Utc::now().timestamp();

    let config = serde_json::json!({
        "id": config_id,
        "modelData": {
            "name": "beampipe-core Auto-generated Manifest",
            "shortDescription": "Manifest embedded automatically by beampipe-core at submit time.",
            "detailedDescription": "This configuration was auto-generated by beampipe and embeds the data manifest via the beampipe-ingest node.",
            "type": "GraphConfig",
            "schemaVersion": "1.0",
            "readonly": true,
            "location": {
                "repositoryService": "Beampipe",
                "repositoryBranch": "",
                "repositoryName": "",
                "repositoryPath": "",
                "repositoryFileName": "",
                "commitHash": "",
                "downloadUrl": "",
            },
            "generatorVersion": "beampipe-v1",
            "generatorCommitHash": "",
            "generatorName": "beampipe",
            "repositoryUrl": "",
            "graphLocation": {},
            "signature": "",
            "lastModifiedName": "beampipe-core System",
            "lastModifiedEmail": "",
            "lastModifiedDatetime": now,
            "numLGNodes": num_lg_nodes,
        },
        "nodes": {
            node_id: {
                GRAPH_FIELDS: {
                    field_id: {"value": value_str, "comment": ""},
                },
            },
        },
    });

    let graph_obj = graph
        .as_object_mut()
        .ok_or(OrchestrationError::GraphNotObject)?;
    let configs = graph_obj
        .entry(GRAPH_CONFIGS)
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let configs_map = configs
        .as_object_mut()
        .ok_or(OrchestrationError::GraphNotObject)?;
    configs_map.insert(config_id.clone(), config);
    graph_obj.insert(ACTIVE_CONFIG_KEY.into(), Value::String(config_id));
    Ok(())
}

pub fn prepare_graph_for_manifest(
    mut graph: Value,
    manifest: &Value,
    _manifest_path: &str,
) -> Result<Value, OrchestrationError> {
    inject_manifest_config_into_graph(&mut graph, manifest, None)?;
    apply_manifest_graph_overrides(&mut graph, manifest)?;
    Ok(graph)
}

pub fn apply_manifest_graph_overrides(
    graph: &mut Value,
    manifest: &Value,
) -> Result<(), OrchestrationError> {
    let Some(spec) = manifest.get("graph_overrides").and_then(Value::as_object) else {
        return Ok(());
    };
    let Some(patches) = spec.get("patches").and_then(Value::as_array) else {
        return Ok(());
    };
    let nodes = graph
        .get_mut(GRAPH_NODES)
        .and_then(Value::as_array_mut)
        .ok_or(OrchestrationError::GraphNotObject)?;
    for patch in patches {
        let Some(match_spec) = patch.get("match").and_then(Value::as_object) else {
            continue;
        };
        let Some(fields) = patch.get("fields").and_then(Value::as_array) else {
            continue;
        };
        let expected_name = match_spec.get("equals").and_then(Value::as_str);
        let match_kind = match_spec.get("kind").and_then(Value::as_str);
        let mut matched_nodes = 0usize;
        let mut matched_fields = std::collections::HashSet::new();
        for node in nodes.iter_mut().filter_map(Value::as_object_mut) {
            let node_name = node.get("name").and_then(Value::as_str);
            let matches = match (match_kind, expected_name) {
                (Some("node_name"), Some(expected)) => node_name == Some(expected),
                (_, Some(expected)) => node_name == Some(expected),
                _ => true,
            };
            if !matches {
                continue;
            }
            matched_nodes += 1;
            let Some(node_fields) = node.get_mut(GRAPH_FIELDS).and_then(Value::as_array_mut) else {
                continue;
            };
            for fd in fields {
                let Some(name) = fd.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let value = fd.get("value").cloned().unwrap_or(Value::Null);
                for node_field in node_fields.iter_mut().filter_map(Value::as_object_mut) {
                    if node_field.get("name").and_then(Value::as_str) == Some(name) {
                        matched_fields.insert(name.to_string());
                        node_field.insert("value".into(), value.clone());
                        if node_field.get("type").and_then(Value::as_str) == Some("Integer") {
                            node_field
                                .insert("defaultValue".into(), Value::String(value.to_string()));
                        }
                    }
                }
            }
        }
        if matched_nodes == 0 {
            if let Some(node) = expected_name {
                return Err(OrchestrationError::GraphPatchNodeNotFound(node.into()));
            }
        }
        if let Some(node) = expected_name {
            for field in fields.iter().filter_map(|field| {
                field
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }) {
                if !matched_fields.contains(&field) {
                    return Err(OrchestrationError::GraphPatchFieldNotFound {
                        node: node.into(),
                        field,
                    });
                }
            }
        }
    }
    Ok(())
}

/// DROPStates.ERROR = 3 (distinct from session RUNNING = 3).
fn drop_status_is_error(status: &Value) -> bool {
    if let Some(n) = status.as_i64() {
        return n == 3;
    }
    if let Some(n) = status.get("status").and_then(Value::as_i64) {
        return n == 3;
    }
    let raw = status
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            status
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
        .to_ascii_uppercase();
    raw.contains("ERROR") || raw.contains("FAILED")
}

/// Preserve the compatibility helper while using the exact DALiuGE session model.
pub fn classify_dim_session_status(status: &Value) -> ExecutionStatus {
    DaliugeSessionState::from_raw(status).execution_status()
}

pub fn dim_graph_status_error_uids(graph: &Value) -> Vec<String> {
    let Some(obj) = graph.as_object() else {
        return Vec::new();
    };
    obj.iter()
        .filter_map(|(uid, status)| {
            if drop_status_is_error(status) {
                Some(uid.clone())
            } else {
                None
            }
        })
        .collect()
}

fn value_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn graph_override_sets_matching_field() {
        let mut graph = json!({"nodeDataArray": [{"name": "Scatter/GenericScatterApp/Beam", "fields": [{"name": "num_of_copies", "type": "Integer"}]}]});
        let manifest = json!({"graph_overrides": {"patches": [{"match": {"equals": "Scatter/GenericScatterApp/Beam"}, "fields": [{"name": "num_of_copies", "value": 3}]}]}});
        apply_manifest_graph_overrides(&mut graph, &manifest).unwrap();
        assert_eq!(graph["nodeDataArray"][0]["fields"][0]["value"], 3);
    }

    #[test]
    fn graph_override_rejects_missing_target_node() {
        let mut graph = json!({"nodeDataArray": [{"name": "other", "fields": []}]});
        let manifest = json!({"graph_overrides": {"patches": [{"match": {"kind": "node_name", "equals": "missing"}, "fields": [{"name": "copies", "value": 3}]}]}});
        let error = apply_manifest_graph_overrides(&mut graph, &manifest).unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::GraphPatchNodeNotFound(ref node) if node == "missing"
        ));
    }

    #[test]
    fn graph_override_rejects_missing_target_field() {
        let mut graph = json!({"nodeDataArray": [{"name": "target", "fields": []}]});
        let manifest = json!({"graph_overrides": {"patches": [{"match": {"kind": "node_name", "equals": "target"}, "fields": [{"name": "copies", "value": 3}]}]}});
        let error = apply_manifest_graph_overrides(&mut graph, &manifest).unwrap_err();
        assert!(matches!(
            error,
            OrchestrationError::GraphPatchFieldNotFound { ref node, ref field }
                if node == "target" && field == "copies"
        ));
    }

    #[test]
    fn wallaby_manifest_groups_by_source_and_sbid() {
        let manifest = build_wallaby_manifest(&[
            serde_json::json!({"source_identifier": "s1", "sbid": "1", "dataset_id": "d1"}),
            serde_json::json!({"source_identifier": "s1", "sbid": "2", "dataset_id": "d2"}),
        ])
        .unwrap();
        assert_eq!(manifest["sources"][0]["sbids"].as_array().unwrap().len(), 2);
        assert_eq!(
            manifest["graph_overrides"]["patches"][0]["fields"][0]["value"],
            2
        );
    }

    #[test]
    fn graph_injection_embeds_manifest_in_graph_configurations() {
        let mut graph = serde_json::json!({
            "nodeDataArray": [{
                "id": "n_ingest",
                "name": "beampipe-ingest",
                "fields": [{"id": "ingf1", "name": "manifest_path", "type": "String", "value": "{}"}],
            }],
            "linkDataArray": [],
        });
        let manifest = serde_json::json!({
            "sources": [{"source_identifier": "x"}],
            "graph_overrides": {"version": 1, "patches": []},
            "secret_marker": "should_be_embedded",
        });
        inject_manifest_config_into_graph(&mut graph, &manifest, Some("cfg-test")).unwrap();
        let cid = graph["activeGraphConfigId"].as_str().unwrap();
        assert_eq!(cid, "cfg-test");
        let embedded = graph["graphConfigurations"][cid]["nodes"]["n_ingest"]["fields"]["ingf1"]
            ["value"]
            .as_str()
            .unwrap();
        let parsed: Value = serde_json::from_str(embedded).unwrap();
        assert!(parsed.get("graph_overrides").is_none());
        assert_eq!(parsed["secret_marker"], "should_be_embedded");
    }

    #[test]
    fn dim_status_classification_maps_terminals() {
        assert_eq!(
            classify_dim_session_status(&serde_json::json!({"status": "FINISHED"})),
            ExecutionStatus::Completed
        );
        assert_eq!(
            classify_dim_session_status(&serde_json::json!({"status": "ERROR"})),
            ExecutionStatus::Failed
        );
    }
}

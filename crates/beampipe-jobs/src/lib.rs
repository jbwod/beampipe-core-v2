use async_trait::async_trait;
use beampipe_adapters::{all_reachable, probe_tap_health, unreachable_adapters, TapClient};
use beampipe_adapters::{casda_tap, vizier_tap, AdapterError, TapRow};
use beampipe_config::Settings;
use beampipe_db::{models::DeploymentProfileRow, repo};
use beampipe_domain::run_record::{
    dim_logs_url, dim_poll_round_from_manifest, merge_dim_poll_into_manifest,
    merge_dim_poll_tick_round, merge_scheduler_timeout_into_manifest,
    merge_slurm_poll_into_manifest, merge_slurm_poll_tick_round, slurm_poll_round_from_manifest,
    SlurmPollManifestOpts,
};
use beampipe_domain::{
    can_admit_by_in_flight,
    discovery::{should_skip_tap, DiscoverySourceResult, SignatureOptions},
    discovery_admission_budget, execute_admission_budget, is_non_retryable_job_error,
    ExecutionPhase, ExecutionStatus, LedgerPatch, SchedulerTickResult, SkipReason,
};
use beampipe_metrics as metrics;
use beampipe_orchestration::slurm_deploy::resolve_remote_user;
use beampipe_orchestration::{
    apply_project_graph_patches, beampipe_session_id, build_manifest_from_config_with_staging,
    dim_unreachable_message, prepare_graph_for_manifest, probe_dim_reachable, probe_slurm_login,
    probe_tm_reachable, resolve_graph, tm_unreachable_message, translate_config_from_profile,
    BackendPoll, CasdaStagingClient, DimClient, HttpClientOptions, HttpDimClient,
    HttpTranslatorClient, MockDimClient, PassThroughStagingClient, RestExecutionBackend,
    SlurmExecutionBackend, SlurmJobPollResult, SlurmSshPool, SlurmTarget, SshSlurmClient,
    StagingClient, TmProbeResult,
};
use beampipe_profiles::{
    DeploymentConfig, RestRemoteDeploymentConfig, SlurmRemoteDeploymentConfig,
};
use beampipe_project::{
    apply_field_transform, build_template_context, ExecutionAutomationConfig, HookKind,
    ProjectConfig, TransformRegistry, WasmHost,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn, Instrument};

static SLURM_SSH_POOL: LazyLock<SlurmSshPool> = LazyLock::new(SlurmSshPool::new_from_env);

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub poll_interval: Duration,
    pub lock_seconds: i64,
    pub discovery_batch_size: i64,
    pub discovery_stale_hours: i32,
    pub discovery_claim_ttl_minutes: i64,
    pub execution_global_in_flight_limit: Option<i64>,
    pub execution_queue_max_depth: Option<i64>,
    pub scheduler_interval: Duration,
    pub discovery_max_in_flight_batches: Option<i64>,
    pub discovery_max_batches_per_tick: i64,
    pub discovery_tap_health_check_enabled: bool,
    pub discovery_tap_health_timeout_seconds: u64,
    pub shaping_enqueue_pacing_ms: u64,
    pub use_real_backends: bool,
    /// Parallel job consumers in this process (each claims jobs independently).
    pub concurrency: u32,
    /// When true, enqueue recurring scheduler_tick / execution_scheduler_tick jobs.
    pub scheduler_enabled: bool,
    /// Max parallel TAP discoveries within one discover_batch job.
    pub discovery_source_concurrency: u32,
    pub metrics_bind_addr: String,
    pub metrics_server_enabled: bool,
}

impl WorkerConfig {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            poll_interval: Duration::from_millis(settings.worker_poll_interval_ms),
            lock_seconds: settings.worker_lock_seconds,
            discovery_batch_size: 50,
            discovery_stale_hours: 24,
            discovery_claim_ttl_minutes: 180,
            execution_global_in_flight_limit: Some(settings.shaping_execution_max_in_flight_runs),
            execution_queue_max_depth: Some(settings.shaping_queue_max_depth),
            scheduler_interval: Duration::from_secs(settings.scheduler_interval_seconds.max(1)),
            discovery_max_in_flight_batches: Some(settings.shaping_discovery_max_in_flight_batches),
            discovery_max_batches_per_tick: settings.shaping_discovery_max_batches_per_tick,
            discovery_tap_health_check_enabled: settings.discovery_tap_health_check_enabled,
            discovery_tap_health_timeout_seconds: settings.discovery_tap_health_timeout_seconds,
            shaping_enqueue_pacing_ms: settings.shaping_enqueue_pacing_ms,
            use_real_backends: std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            concurrency: settings.worker_concurrency.max(1),
            scheduler_enabled: settings.worker_scheduler_enabled,
            discovery_source_concurrency: settings.discovery_source_concurrency.max(1),
            metrics_bind_addr: settings.metrics_bind_addr.clone(),
            metrics_server_enabled: settings.metrics_server_enabled,
        }
    }

    pub fn with_polling(poll_interval: Duration, lock_seconds: i64) -> Self {
        Settings::from_env()
            .map(|s| {
                let mut cfg = Self::from_settings(&s);
                cfg.poll_interval = poll_interval;
                cfg.lock_seconds = lock_seconds;
                cfg
            })
            .unwrap_or(Self {
                poll_interval,
                lock_seconds,
                discovery_batch_size: 50,
                discovery_stale_hours: 24,
                discovery_claim_ttl_minutes: 180,
                execution_global_in_flight_limit: Some(2),
                execution_queue_max_depth: Some(200),
                scheduler_interval: Duration::from_secs(60),
                discovery_max_in_flight_batches: Some(4),
                discovery_max_batches_per_tick: 4,
                discovery_tap_health_check_enabled: true,
                discovery_tap_health_timeout_seconds: 10,
                shaping_enqueue_pacing_ms: 0,
                use_real_backends: std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false),
                concurrency: 1,
                scheduler_enabled: true,
                discovery_source_concurrency: 5,
                metrics_bind_addr: "127.0.0.1:9090".into(),
                metrics_server_enabled: true,
            })
    }
}

/// Background worker pool: optional scheduler bootstrap + N parallel job consumers.
pub struct WorkerPool {
    pub handles: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    pub async fn shutdown(self) {
        tracing::info!("event=worker_pool_shutdown");
        for handle in self.handles {
            handle.abort();
        }
    }
}

pub fn spawn_workers(pool: PgPool, config: WorkerConfig) -> WorkerPool {
    metrics::init_recorder();
    let mut handles = Vec::new();
    if config.metrics_server_enabled {
        if let Ok(addr) = config.metrics_bind_addr.parse() {
            handles.push(metrics::server::spawn_metrics_server(
                addr,
                Some(pool.clone()),
            ));
        }
    }
    if config.scheduler_enabled {
        let sched_pool = pool.clone();
        let sched_config = config.clone();
        handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(sched_config.scheduler_interval);
            info!(
                concurrency = sched_config.concurrency,
                scheduler_enabled = true,
                "event=scheduler_bootstrap_started"
            );
            loop {
                interval.tick().await;
                if let Err(err) = bootstrap_schedulers(&sched_pool, &sched_config).await {
                    error!(error = %err, "event=scheduler_bootstrap_error");
                }
            }
        }));
    }
    let consumer_count = config.concurrency.max(1);
    for worker_id in 0..consumer_count {
        let pool = pool.clone();
        let worker_config = config.clone();
        handles.push(tokio::spawn(async move {
            info!(
                worker_id,
                concurrency = consumer_count,
                "event=job_worker_started"
            );
            loop {
                if let Err(err) = tick(&pool, &worker_config).await {
                    error!(worker_id, error = %err, "event=job_worker_tick_error");
                }
                tokio::time::sleep(worker_config.poll_interval).await;
            }
        }));
    }
    WorkerPool { handles }
}

/// Start scheduler bootstrap (optional) + N parallel job consumers.
pub fn spawn_worker(pool: PgPool, config: WorkerConfig) -> WorkerPool {
    spawn_workers(pool, config)
}

async fn bootstrap_schedulers(pool: &PgPool, config: &WorkerConfig) -> Result<(), sqlx::Error> {
    let configs = repo::list_active_project_configs(pool).await?;
    for project_config in configs {
        let spec = &project_config.spec;
        let discovery = serde_json::from_value::<ProjectConfig>(spec.clone())
            .ok()
            .and_then(|c| c.automation.discovery)
            .unwrap_or_default();
        if discovery.enabled {
            let payload = json!({
                "project_module": project_config.project_id,
                "batch_size": discovery.batch_size,
                "stale_after_hours": discovery.stale_after_hours,
                "claim_ttl_minutes": discovery.claim_ttl_minutes,
                "queue_max_depth": discovery.queue_max_depth.or(config.execution_queue_max_depth),
                "tick_discovery_source_limit": discovery.tick_discovery_source_limit,
                "tick_discovery_batch_limit": discovery.tick_discovery_batch_limit,
                "concurrent_discovery_batch_limit": discovery.concurrent_discovery_batch_limit,
            });
            let _ = repo::enqueue_recurring_job(
                pool,
                "scheduler_tick",
                payload,
                &format!("scheduler_tick:{}", project_config.project_id),
            )
            .await;
        }
        let execution_enabled = spec
            .get("automation")
            .and_then(|v| v.get("execution"))
            .and_then(|v| v.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if execution_enabled {
            let _ = repo::enqueue_recurring_job(
                pool,
                "execution_scheduler_tick",
                json!({"project_module": project_config.project_id}),
                &format!("execution_scheduler_tick:{}", project_config.project_id),
            )
            .await;
        }
    }
    let _modules = repo::get_enabled_project_modules(pool).await?;
    let _ = repo::enqueue_recurring_job(
        pool,
        "alert_evaluator_tick",
        json!({}),
        "alert_evaluator_tick",
    )
    .await;
    let tick_interval = slurm_poll_tick_interval_secs(pool).await.unwrap_or(30);
    let _ = repo::enqueue_recurring_job(
        pool,
        "slurm_poll_tick",
        json!({ "interval_secs": tick_interval }),
        "slurm_poll_tick",
    )
    .await;
    let dim_tick_interval = dim_poll_tick_interval_secs(pool).await.unwrap_or(3);
    let _ = repo::enqueue_recurring_job(
        pool,
        "dim_poll_tick",
        json!({ "interval_secs": dim_tick_interval }),
        "dim_poll_tick",
    )
    .await;
    let _ = config;
    Ok(())
}

pub async fn tick(pool: &PgPool, config: &WorkerConfig) -> Result<(), sqlx::Error> {
    let Some(job) = repo::claim_next_job(pool, config.lock_seconds).await? else {
        return Ok(());
    };
    let job_started = std::time::Instant::now();
    metrics::record_job(&job.kind, "claimed");
    debug!(job_id = %job.uuid, kind = %job.kind, "event=job_claimed");
    let correlation = correlation_id_from_payload(&job.payload)
        .map(str::to_string)
        .or_else(|| job.execution_id.map(|id| id.to_string()))
        .unwrap_or_default();
    let span = tracing::info_span!(
        "job",
        job_id = %job.uuid,
        job_kind = %job.kind,
        correlation_id = %correlation,
        execution_id = job.execution_id.map(|id| id.to_string()).unwrap_or_default()
    );
    let runner = ConfigDiscoveryRunner::from_env_with_pool(Some(pool.clone()));
    let result = async { dispatch(pool, config, &runner, &job.kind, &job.payload).await }
        .instrument(span)
        .await;
    match result {
        Ok(()) => {
            metrics::record_job(&job.kind, "completed");
            metrics::record_job_duration(&job.kind, job_started.elapsed().as_secs_f64());
            if job.kind == "slurm_poll_tick" || job.kind == "dim_poll_tick" {
                let interval = job
                    .payload
                    .get("interval_secs")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(if job.kind == "dim_poll_tick" { 3 } else { 30 });
                repo::reschedule_recurring_job(pool, job.uuid, interval).await?;
            } else {
                repo::complete_job(pool, job.uuid).await?;
            }
        }
        Err(err) => {
            metrics::record_job(&job.kind, "failed");
            metrics::record_job_duration(&job.kind, job_started.elapsed().as_secs_f64());
            if is_non_retryable_job_error(&job.kind, &err) {
                repo::fail_job_permanently(pool, job.uuid, &err).await?;
            } else {
                repo::fail_or_retry_job(pool, job.uuid, &err).await?;
            }
        }
    }
    Ok(())
}

async fn dispatch<R: DiscoveryRunner + Clone + Send + Sync + 'static>(
    pool: &PgPool,
    config: &WorkerConfig,
    runner: &R,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<(), String> {
    match kind {
        "scheduler_tick" => {
            run_scheduler_tick(pool, config, payload)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        "discover_batch" => {
            run_discover_batch(pool, config, runner, payload)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        "execution_scheduler_tick" => {
            run_execution_scheduler_tick(pool, config, payload)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        "execute" => run_execute(pool, payload).await.map_err(|e| e.to_string()),
        "dim_poll" => run_dim_poll(pool, payload).await.map_err(|e| e.to_string()),
        "dim_poll_tick" => run_dim_poll_tick(pool, payload)
            .await
            .map_err(|e| e.to_string()),
        "slurm_poll_tick" => run_slurm_poll_tick(pool, payload)
            .await
            .map_err(|e| e.to_string()),
        "alert_evaluator_tick" => run_alert_evaluator_tick(pool)
            .await
            .map_err(|e| e.to_string()),
        other => Err(format!("unknown job kind {other}")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutionAutomationPolicy {
    enabled: bool,
    archive_name: String,
    max_sources_per_execution: i64,
    tick_execution_source_limit: i64,
    tick_execution_run_limit: i64,
    min_sources_to_trigger: i64,
    max_wait_minutes: i64,
    claim_ttl_minutes: i64,
    concurrent_execution_run_limit: Option<i64>,
    deployment_profile_name: Option<String>,
}

impl Default for ExecutionAutomationPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            archive_name: "casda".into(),
            max_sources_per_execution: 20,
            tick_execution_source_limit: 500,
            tick_execution_run_limit: 20,
            min_sources_to_trigger: 1,
            max_wait_minutes: 24 * 60,
            claim_ttl_minutes: 180,
            concurrent_execution_run_limit: None,
            deployment_profile_name: None,
        }
    }
}

impl ExecutionAutomationPolicy {
    fn from_spec(spec: &serde_json::Value) -> Self {
        if let Ok(config) = serde_json::from_value::<ProjectConfig>(spec.clone()) {
            return Self::from_config(config.automation.execution.unwrap_or_default());
        }
        Self::from_legacy_value(
            spec.get("automation")
                .and_then(|v| v.get("execution"))
                .unwrap_or(&serde_json::Value::Null),
        )
    }

    fn from_config(raw: ExecutionAutomationConfig) -> Self {
        Self {
            enabled: raw.enabled,
            archive_name: raw.archive_name,
            max_sources_per_execution: raw.max_sources_per_execution,
            tick_execution_source_limit: raw.tick_execution_source_limit,
            tick_execution_run_limit: raw.tick_execution_run_limit,
            min_sources_to_trigger: raw.min_sources_to_trigger,
            max_wait_minutes: raw.max_wait_minutes,
            claim_ttl_minutes: raw.claim_ttl_minutes,
            concurrent_execution_run_limit: raw.concurrent_execution_run_limit,
            deployment_profile_name: raw.deployment_profile_name,
        }
    }

    fn from_legacy_value(raw: &serde_json::Value) -> Self {
        let mut out = Self::default();
        if !raw.is_object() {
            return out;
        }
        out.enabled = raw
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(out.enabled);
        out.archive_name = raw
            .get("archive_name")
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(&out.archive_name)
            .to_string();
        out.max_sources_per_execution =
            positive_i64(raw, "max_sources_per_execution").unwrap_or(out.max_sources_per_execution);
        out.tick_execution_source_limit = positive_i64(raw, "tick_execution_source_limit")
            .unwrap_or(out.tick_execution_source_limit);
        out.tick_execution_run_limit =
            positive_i64(raw, "tick_execution_run_limit").unwrap_or(out.tick_execution_run_limit);
        out.min_sources_to_trigger =
            positive_i64(raw, "min_sources_to_trigger").unwrap_or(out.min_sources_to_trigger);
        out.max_wait_minutes =
            positive_i64(raw, "max_wait_minutes").unwrap_or(out.max_wait_minutes);
        out.claim_ttl_minutes =
            positive_i64(raw, "claim_ttl_minutes").unwrap_or(out.claim_ttl_minutes);
        out.concurrent_execution_run_limit = positive_i64(raw, "concurrent_execution_run_limit");
        out.deployment_profile_name = raw
            .get("deployment_profile_name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToString::to_string);
        out
    }
}

fn positive_i64(raw: &serde_json::Value, key: &str) -> Option<i64> {
    raw.get(key)
        .and_then(serde_json::Value::as_i64)
        .filter(|v| *v > 0)
}

#[async_trait]
pub trait DiscoveryRunner: Send + Sync {
    async fn discover_source(
        &self,
        project_config: Option<&ProjectConfig>,
        project_module: &str,
        source_identifier: &str,
    ) -> DiscoverySourceResult;
}

#[derive(Debug, Clone, Copy)]
pub struct DeterministicDiscoveryRunner;

#[async_trait]
impl DiscoveryRunner for DeterministicDiscoveryRunner {
    async fn discover_source(
        &self,
        _project_config: Option<&ProjectConfig>,
        _project_module: &str,
        source_identifier: &str,
    ) -> DiscoverySourceResult {
        DiscoverySourceResult::NoDatasets {
            source_identifier: source_identifier.to_string(),
            duration_ms: Some(0),
        }
    }
}

#[derive(Clone)]
pub struct ConfigDiscoveryRunner {
    clients: BTreeMap<String, Arc<dyn TapClient>>,
    pool: Option<PgPool>,
}

impl ConfigDiscoveryRunner {
    pub fn from_env() -> Self {
        Self::from_env_with_pool(None)
    }

    pub fn from_env_with_pool(pool: Option<PgPool>) -> Self {
        let mut clients: BTreeMap<String, Arc<dyn TapClient>> = BTreeMap::new();
        if let Ok(url) = std::env::var("BEAMPIPE_CASDA_TAP_URL") {
            clients.insert("casda".into(), Arc::new(casda_tap(url)));
        }
        if let Ok(url) = std::env::var("BEAMPIPE_VIZIER_TAP_URL") {
            clients.insert("vizier".into(), Arc::new(vizier_tap(url)));
        }
        Self { clients, pool }
    }

    fn client_for<'a>(
        &'a self,
        config: &'a ProjectConfig,
        adapter: &str,
    ) -> Result<Arc<dyn TapClient>, ConfigDiscoveryError> {
        if let Some(client) = self.clients.get(adapter) {
            return Ok(Arc::clone(client));
        }
        let timeout = Duration::from_secs(config.adapters.tap.timeout_seconds);
        let retries = config.adapters.tap.retries;
        let casda_env = std::env::var("BEAMPIPE_CASDA_TAP_URL").ok();
        let vizier_env = std::env::var("BEAMPIPE_VIZIER_TAP_URL").ok();
        let client: Arc<dyn TapClient> = match adapter {
            "casda" => {
                let url = config
                    .adapters
                    .casda_tap_url
                    .as_deref()
                    .or(casda_env.as_deref())
                    .ok_or_else(|| ConfigDiscoveryError::MissingAdapter("casda".into()))?;
                Arc::new(casda_tap(url).with_policy(timeout, retries))
            }
            "vizier" => {
                let url = config
                    .adapters
                    .vizier_tap_url
                    .as_deref()
                    .or(vizier_env.as_deref())
                    .ok_or_else(|| ConfigDiscoveryError::MissingAdapter("vizier".into()))?;
                Arc::new(vizier_tap(url).with_policy(timeout, retries))
            }
            other => return Err(ConfigDiscoveryError::MissingAdapter(other.to_string())),
        };
        Ok(client)
    }

    #[cfg(test)]
    fn with_clients(clients: BTreeMap<String, Arc<dyn TapClient>>) -> Self {
        Self {
            clients,
            pool: None,
        }
    }
}

#[async_trait]
impl DiscoveryRunner for ConfigDiscoveryRunner {
    async fn discover_source(
        &self,
        project_config: Option<&ProjectConfig>,
        project_module: &str,
        source_identifier: &str,
    ) -> DiscoverySourceResult {
        let started = std::time::Instant::now();
        let Some(config) = project_config else {
            return DiscoverySourceResult::Error {
                source_identifier: source_identifier.to_string(),
                error: format!("no active project config for project {project_module}"),
                duration_ms: Some(0),
            };
        };
        if let Some(pool) = &self.pool {
            if let Ok(Some(source_row)) =
                repo::get_source_by_identifier(pool, project_module, source_identifier).await
            {
                if let Ok(metadata_rows) = repo::list_source_metadata(pool, &source_row).await {
                    let records: Vec<(String, Value)> = metadata_rows
                        .iter()
                        .filter_map(|r| r.metadata_json.clone().map(|v| (r.sbid.clone(), v)))
                        .collect();
                    if !records.is_empty() {
                        let sig_opts = config
                            .discovery
                            .prepare_metadata
                            .as_ref()
                            .and_then(|p| p.signature.as_ref())
                            .map(|c| SignatureOptions {
                                exclude_fields: c.exclude_fields.clone(),
                                include_discovery_flags: c.include_discovery_flags,
                            })
                            .unwrap_or_default();
                        if should_skip_tap(
                            source_row.discovery_signature.as_deref(),
                            &records,
                            &sig_opts,
                        ) {
                            metrics::record_discovery_tap_skipped(project_module);
                            if let Some(pool) = &self.pool {
                                let payload = json!({
                                    "source_identifier": source_identifier,
                                    "reason": "signature_unchanged",
                                });
                                beampipe_db::provenance::record_provenance_event(
                                    pool,
                                    beampipe_domain::provenance::ProvenanceEventType::DiscoveryTapSkipped
                                        .as_str(),
                                    project_module,
                                    Some(source_identifier),
                                    None,
                                    Some("system:discovery"),
                                    None,
                                    &payload,
                                )
                                .await;
                            }
                            return DiscoverySourceResult::Unchanged {
                                source_identifier: source_identifier.to_string(),
                                duration_ms: Some(started.elapsed().as_millis() as i64),
                            };
                        }
                    }
                }
            }
        }
        match self.discover_from_config(config, source_identifier).await {
            Ok(Some((metadata, discovery_flags))) => {
                let metadata = if let Some(pool) = &self.pool {
                    apply_wasm_prepare_metadata(pool, config, &json!({}), &metadata)
                        .await
                        .unwrap_or(metadata)
                } else {
                    metadata
                };
                DiscoverySourceResult::HasMetadata {
                    source_identifier: source_identifier.to_string(),
                    metadata,
                    discovery_flags,
                    duration_ms: Some(started.elapsed().as_millis() as i64),
                }
            }
            Ok(None) => DiscoverySourceResult::NoDatasets {
                source_identifier: source_identifier.to_string(),
                duration_ms: Some(started.elapsed().as_millis() as i64),
            },
            Err(ConfigDiscoveryError::Adapter(AdapterError::Timeout)) => {
                DiscoverySourceResult::Timeout {
                    source_identifier: source_identifier.to_string(),
                    error: "TAP timeout".into(),
                    duration_ms: Some(started.elapsed().as_millis() as i64),
                }
            }
            Err(err) => DiscoverySourceResult::Error {
                source_identifier: source_identifier.to_string(),
                error: err.to_string(),
                duration_ms: Some(started.elapsed().as_millis() as i64),
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ConfigDiscoveryError {
    #[error("project config has no discovery queries")]
    NoQueries,
    #[error("adapter '{0}' is not configured")]
    MissingAdapter(String),
    #[error("adapter error: {0}")]
    Adapter(#[from] AdapterError),
}

impl ConfigDiscoveryRunner {
    async fn discover_from_config(
        &self,
        config: &ProjectConfig,
        source_identifier: &str,
    ) -> Result<Option<(Vec<Value>, Value)>, ConfigDiscoveryError> {
        let Some(primary) = config.discovery.queries.first() else {
            return Err(ConfigDiscoveryError::NoQueries);
        };
        let registry = TransformRegistry::from_config(config);
        let mut context = build_template_context(source_identifier, config);

        let rows = self
            .query_configured(
                config,
                &primary.adapter,
                &render_template(&primary.template, &context),
            )
            .await?;
        if rows.is_empty() {
            return Ok(None);
        }

        let mut enrichments = Map::new();
        for query in config.discovery.queries.iter().skip(1) {
            let name = query.name.clone();
            if config.source_identity.is_none() {
                if let Some(transform) = query.source_id_transform.as_deref() {
                    if let Some(value) = registry.apply_named(transform, &json!(source_identifier))
                    {
                        context.insert("source_name".into(), value);
                    }
                }
            }
            let rendered = render_template(&query.template, &context);
            match self
                .query_configured(config, &query.adapter, &rendered)
                .await
            {
                Ok(rows) => {
                    enrichments.insert(name, rows_value(rows));
                }
                Err(err) => {
                    warn!(
                        adapter = query.adapter,
                        query = name,
                        error = %err,
                        "event=discover_enrichment_query_failed"
                    );
                    enrichments.insert(name, Value::Array(vec![]));
                }
            }
        }

        let sbids: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                field_map_value(row, config, "sbid", &registry).and_then(|v| value_string(Some(&v)))
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        for query in &config.discovery.enrichments {
            let mut by_sbid = Map::new();
            for sbid in &sbids {
                let mut sbid_context = context.clone();
                sbid_context.insert("sbid".into(), json!(sbid));
                let rendered = render_template(&query.template, &sbid_context);
                match self
                    .query_configured(config, &query.adapter, &rendered)
                    .await
                {
                    Ok(rows) => {
                        if let Some(first) = rows.first() {
                            by_sbid.insert(sbid.clone(), Value::Object(first.clone()));
                        }
                    }
                    Err(err) => {
                        warn!(
                            adapter = query.adapter,
                            query = query.name,
                            sbid,
                            error = %err,
                            "event=discover_sbid_enrichment_failed"
                        );
                    }
                }
            }
            enrichments.insert(query.name.clone(), Value::Object(by_sbid));
        }

        let discovery_flags = discovery_flags_from_config(config, &enrichments, &registry);
        let metadata: Vec<Value> = rows
            .iter()
            .map(|row| {
                prepare_metadata_record(
                    source_identifier,
                    row,
                    config,
                    &enrichments,
                    &discovery_flags,
                    &registry,
                )
            })
            .collect();
        Ok(Some((metadata, discovery_flags)))
    }

    async fn query_configured(
        &self,
        config: &ProjectConfig,
        adapter: &str,
        adql: &str,
    ) -> Result<Vec<TapRow>, ConfigDiscoveryError> {
        let client = self.client_for(config, adapter)?;
        Ok(client.query_rows(adql).await?)
    }
}

async fn pacing_sleep(config: &WorkerConfig) {
    if config.shaping_enqueue_pacing_ms > 0 {
        tokio::time::sleep(Duration::from_millis(config.shaping_enqueue_pacing_ms)).await;
    }
}

async fn run_alert_evaluator_tick(pool: &PgPool) -> Result<(), sqlx::Error> {
    beampipe_alerts::evaluate_scheduled_rules(pool)
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))
}

async fn finish_discovery_scheduler_tick(
    pool: &PgPool,
    project_module: &str,
    result: &SchedulerTickResult,
    started: std::time::Instant,
) {
    metrics::record_scheduler_tick_duration("discovery", started.elapsed().as_secs_f64());
    metrics::record_scheduler_skip_reasons(project_module, &result.reason_counts);
    let _ = refresh_pool_gauges(pool).await;
}

async fn run_scheduler_tick(
    pool: &PgPool,
    config: &WorkerConfig,
    payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let started = std::time::Instant::now();
    let project_module = payload
        .get("project_module")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let mut result = SchedulerTickResult::new(project_module);

    let project_config = if project_module.is_empty() {
        None
    } else {
        repo::get_active_project_config(pool, project_module).await?
    };
    let parsed_config = project_config
        .as_ref()
        .and_then(|row| serde_json::from_value::<ProjectConfig>(row.spec.clone()).ok());

    if config.discovery_tap_health_check_enabled {
        if let Some(cfg) = parsed_config.as_ref() {
            let timeout = Duration::from_secs(config.discovery_tap_health_timeout_seconds);
            let casda_env = std::env::var("BEAMPIPE_CASDA_TAP_URL").ok();
            let vizier_env = std::env::var("BEAMPIPE_VIZIER_TAP_URL").ok();
            let casda_url = cfg
                .adapters
                .casda_tap_url
                .as_deref()
                .or(casda_env.as_deref());
            let vizier_url = cfg
                .adapters
                .vizier_tap_url
                .as_deref()
                .or(vizier_env.as_deref());
            let report = probe_tap_health(casda_url, vizier_url, timeout).await;
            if !all_reachable(&report, &cfg.adapters.required) {
                result.tap_unreachable = unreachable_adapters(&report, &cfg.adapters.required);
                result.bump(SkipReason::TapUnreachable);
                info!(
                    project_module,
                    tap_unreachable = ?result.tap_unreachable,
                    reason_counts = ?result.reason_counts,
                    "event=discovery_scheduler_tick_complete"
                );
                finish_discovery_scheduler_tick(pool, project_module, &result, started).await;
                return Ok(());
            }
        }
    }

    let max_depth = payload
        .get("queue_max_depth")
        .and_then(serde_json::Value::as_i64)
        .or(config.execution_queue_max_depth);
    if let Some(max_depth) = max_depth {
        let depth = repo::queue_depth(pool).await?;
        if depth >= max_depth {
            result.bump(SkipReason::QueueFull);
            info!(
                project_module,
                depth,
                max_depth,
                reason_counts = ?result.reason_counts,
                "event=discovery_scheduler_tick_complete"
            );
            finish_discovery_scheduler_tick(pool, project_module, &result, started).await;
            return Ok(());
        }
    }

    if let Some(cap) = config.discovery_max_in_flight_batches {
        let in_flight = repo::count_discovery_in_flight_batches(pool).await?;
        if !can_admit_by_in_flight(in_flight, cap) {
            result.bump(SkipReason::InFlightCap);
            info!(
                project_module,
                in_flight,
                cap,
                reason_counts = ?result.reason_counts,
                "event=discovery_scheduler_tick_complete"
            );
            finish_discovery_scheduler_tick(pool, project_module, &result, started).await;
            return Ok(());
        }
    }

    let project_in_flight_cap = payload
        .get("concurrent_discovery_batch_limit")
        .and_then(serde_json::Value::as_i64);
    if let (Some(module), Some(cap)) = (
        (!project_module.is_empty()).then_some(project_module),
        project_in_flight_cap,
    ) {
        let in_flight = repo::count_discovery_in_flight_for_module(pool, module).await?;
        if !can_admit_by_in_flight(in_flight, cap) {
            result.bump(SkipReason::ProjectInFlightCap);
            info!(
                project_module,
                in_flight,
                cap,
                reason_counts = ?result.reason_counts,
                "event=discovery_scheduler_tick_complete"
            );
            finish_discovery_scheduler_tick(pool, project_module, &result, started).await;
            return Ok(());
        }
    }

    let batch_size = payload
        .get("batch_size")
        .and_then(serde_json::Value::as_i64)
        .filter(|v| *v > 0)
        .unwrap_or(config.discovery_batch_size);
    let stale_after_hours = payload
        .get("stale_after_hours")
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(config.discovery_stale_hours);
    let claim_ttl_minutes = payload
        .get("claim_ttl_minutes")
        .and_then(serde_json::Value::as_i64)
        .filter(|v| *v > 0)
        .unwrap_or(config.discovery_claim_ttl_minutes);
    let tick_source_limit = payload
        .get("tick_discovery_source_limit")
        .and_then(serde_json::Value::as_i64)
        .filter(|v| *v > 0)
        .unwrap_or(batch_size);
    let max_batches_per_tick = payload
        .get("tick_discovery_batch_limit")
        .and_then(serde_json::Value::as_i64)
        .filter(|v| *v > 0)
        .unwrap_or(config.discovery_max_batches_per_tick);

    let mut remaining_sources = discovery_admission_budget(tick_source_limit.max(batch_size));
    let module_filter = (!project_module.is_empty()).then_some(project_module);

    while remaining_sources > 0 && (result.batches_this_tick as i64) < max_batches_per_tick {
        if let Some(max_depth) = max_depth {
            let depth = repo::queue_depth(pool).await?;
            if depth >= max_depth {
                result.bump(SkipReason::QueueFull);
                break;
            }
        }
        if let Some(cap) = config.discovery_max_in_flight_batches {
            let in_flight = repo::count_discovery_in_flight_batches(pool).await?;
            if !can_admit_by_in_flight(in_flight, cap) {
                result.bump(SkipReason::InFlightCap);
                break;
            }
        }

        let claim_limit = batch_size.min(remaining_sources);
        let (claim_token, rows) = repo::claim_source_rows_for_discovery(
            pool,
            module_filter,
            stale_after_hours,
            claim_limit,
            claim_ttl_minutes,
        )
        .await?;
        let Some(claim_token) = claim_token else {
            break;
        };
        if rows.is_empty() {
            break;
        }

        let mut by_module: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (module, source) in &rows {
            by_module
                .entry(module.clone())
                .or_default()
                .push(source.clone());
        }

        let mut enqueued_any = false;
        for (module, source_identifiers) in &by_module {
            if (result.batches_this_tick as i64) >= max_batches_per_tick {
                let all_sources: Vec<String> = rows.iter().map(|(_, s)| s.clone()).collect();
                let _ =
                    repo::release_discovery_claim(pool, module, &all_sources, &claim_token).await?;
                result.bump(SkipReason::MaxBatchesPerTick);
                info!(
                    project_module,
                    max_batches_per_tick, "event=discover_schedule_max_batches_per_tick"
                );
                break;
            }
            repo::enqueue_job(
                pool,
                "discover_batch",
                json!({
                    "project_module": module,
                    "source_identifiers": source_identifiers,
                    "claim_token": claim_token,
                    "scheduler": {
                        "policy_decision": "admitted",
                        "admitted_source_count": source_identifiers.len(),
                    },
                }),
                None,
                Some(&format!("discover:{module}:{claim_token}")),
            )
            .await?;
            result.total_sources += source_identifiers.len() as u64;
            result.total_jobs += 1;
            result.batches_this_tick += 1;
            enqueued_any = true;
            pacing_sleep(config).await;
        }

        if !enqueued_any {
            let all_sources: Vec<String> = rows.iter().map(|(_, s)| s.clone()).collect();
            if let Some((module, _)) = rows.first() {
                let _ =
                    repo::release_discovery_claim(pool, module, &all_sources, &claim_token).await?;
            }
            break;
        }

        remaining_sources -= rows.len() as i64;
    }

    info!(
        project_module,
        total_sources = result.total_sources,
        total_jobs = result.total_jobs,
        batches_this_tick = result.batches_this_tick,
        reason_counts = ?result.reason_counts,
        skipped_due_to_queue_full = result.skipped_due_to_queue_full,
        skipped_due_to_tap_unreachable = result.skipped_due_to_tap_unreachable,
        skipped_due_to_max_batches_per_tick = result.skipped_due_to_max_batches_per_tick,
        "event=discovery_scheduler_tick_complete"
    );
    finish_discovery_scheduler_tick(pool, project_module, &result, started).await;
    Ok(())
}

async fn discover_sources_parallel<R: DiscoveryRunner + Clone + Send + Sync + 'static>(
    runner: &R,
    project_config: Option<&ProjectConfig>,
    project_module: &str,
    source_identifiers: Vec<String>,
    concurrency: usize,
) -> Vec<DiscoverySourceResult> {
    let concurrency = concurrency.max(1);
    if concurrency == 1 {
        let mut results = Vec::with_capacity(source_identifiers.len());
        for source_identifier in source_identifiers {
            results.push(
                runner
                    .discover_source(project_config, project_module, &source_identifier)
                    .await,
            );
        }
        return results;
    }

    let sem = Arc::new(Semaphore::new(concurrency));
    let project_config = project_config.cloned();
    let module = project_module.to_string();
    let mut handles = Vec::with_capacity(source_identifiers.len());
    for source_identifier in source_identifiers {
        let permit = match sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break,
        };
        let runner = runner.clone();
        let pc = project_config.clone();
        let module = module.clone();
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            runner
                .discover_source(pc.as_ref(), &module, &source_identifier)
                .await
        }));
    }
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(result) => results.push(result),
            Err(err) => results.push(DiscoverySourceResult::Error {
                source_identifier: String::new(),
                error: format!("discover task join failed: {err}"),
                duration_ms: None,
            }),
        }
    }
    results
}

async fn run_discover_batch<R: DiscoveryRunner + Clone + Send + Sync + 'static>(
    pool: &PgPool,
    config: &WorkerConfig,
    runner: &R,
    payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let project_module = payload
        .get("project_module")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| sqlx::Error::Protocol("discover_batch missing project_module".into()))?;
    let claim_token = payload
        .get("claim_token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| sqlx::Error::Protocol("discover_batch missing claim_token".into()))?;
    let batch_started = std::time::Instant::now();
    let source_identifiers: Vec<String> = payload
        .get("source_identifiers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| sqlx::Error::Protocol("discover_batch missing source_identifiers".into()))?
        .iter()
        .filter_map(|v| v.as_str().map(ToString::to_string))
        .collect();
    let project_config = repo::get_active_project_config(pool, project_module)
        .await?
        .and_then(|row| serde_json::from_value::<ProjectConfig>(row.spec).ok());
    let results = discover_sources_parallel(
        runner,
        project_config.as_ref(),
        project_module,
        source_identifiers,
        config.discovery_source_concurrency as usize,
    )
    .await;
    let signature_config = project_config
        .as_ref()
        .and_then(|c| c.discovery.prepare_metadata.as_ref())
        .and_then(|p| p.signature.as_ref());
    let stats = repo::persist_discovery_results(
        pool,
        project_module,
        claim_token,
        &results,
        signature_config,
    )
    .await?;
    for result in &results {
        match result {
            DiscoverySourceResult::Error {
                source_identifier,
                error,
                ..
            } => warn!(
                project_module,
                source_identifier, error, "event=discover_source_error"
            ),
            DiscoverySourceResult::Timeout {
                source_identifier,
                error,
                ..
            } => warn!(
                project_module,
                source_identifier, error, "event=discover_source_timeout"
            ),
            DiscoverySourceResult::NoDatasets {
                source_identifier, ..
            } => {
                info!(
                    project_module,
                    source_identifier, "event=discover_source_no_datasets"
                )
            }
            _ => {}
        }
    }
    info!(
        project_module,
        total_sources = stats.total_sources,
        changed_count = stats.changed_count,
        unchanged_count = stats.unchanged_count,
        "event=discover_batch_persisted"
    );
    metrics::record_discovery_batch_stats(
        project_module,
        stats.changed_count,
        stats.unchanged_count,
        stats.no_datasets_count,
        stats.error_count,
        stats.timeout_count,
    );
    metrics::record_discovery_duration(project_module, batch_started.elapsed().as_secs_f64());
    for result in &results {
        if let Some(ms) = result.duration_ms() {
            metrics::record_discovery_duration(project_module, ms as f64 / 1000.0);
        }
    }
    refresh_pool_gauges(pool).await?;
    Ok(())
}

async fn refresh_pool_gauges(pool: &PgPool) -> Result<(), sqlx::Error> {
    let depth = repo::queue_depth(pool).await?;
    let running = repo::jobs_running_count(pool).await?;
    metrics::set_jobs_queue_depth(depth);
    metrics::set_jobs_running(running);
    let pending = repo::workflow_pending_counts_by_module(pool).await?;
    for (module, count) in pending {
        metrics::set_workflow_pending_sources(&module, count);
    }
    Ok(())
}

async fn flush_execution_scheduler_metrics(
    pool: &PgPool,
    project_module: &str,
    result: &SchedulerTickResult,
    admitted_runs: i64,
    started: std::time::Instant,
) {
    metrics::record_scheduler_tick_duration("execution", started.elapsed().as_secs_f64());
    metrics::record_scheduler_skip_reasons(project_module, &result.reason_counts);
    for _ in 0..admitted_runs {
        metrics::record_execution_admitted(project_module, "admitted");
    }
    let _ = refresh_pool_gauges(pool).await;
}

async fn finalize_execution_source_pending(
    pool: &PgPool,
    project_module: &str,
    sources: &[String],
    status: ExecutionStatus,
    execution_id: Option<uuid::Uuid>,
) -> Result<(), sqlx::Error> {
    let event_type = match status {
        ExecutionStatus::Failed => {
            beampipe_domain::provenance::ProvenanceEventType::ExecutionFailed
        }
        ExecutionStatus::Completed => {
            beampipe_domain::provenance::ProvenanceEventType::ExecutionCompleted
        }
        ExecutionStatus::NotSubmitted => {
            beampipe_domain::provenance::ProvenanceEventType::ExecutionNotSubmitted
        }
        ExecutionStatus::Cancelled => {
            beampipe_domain::provenance::ProvenanceEventType::ExecutionCancelled
        }
        _ => {
            return Ok(());
        }
    };
    match status {
        ExecutionStatus::Failed => {
            repo::mark_sources_pending_workflow_run(pool, project_module, sources).await?;
        }
        ExecutionStatus::Completed | ExecutionStatus::NotSubmitted => {
            repo::clear_workflow_pending_for_sources(pool, project_module, sources).await?;
            repo::set_last_executed_discovery_signature_for_sources(pool, project_module, sources)
                .await?;
        }
        ExecutionStatus::Cancelled => {
            repo::clear_workflow_pending_for_sources(pool, project_module, sources).await?;
        }
        _ => {}
    }
    let payload = json!({
        "source_identifiers": sources,
        "status": status.as_str(),
    });
    beampipe_db::provenance::record_provenance_event(
        pool,
        event_type.as_str(),
        project_module,
        sources.first().map(String::as_str),
        execution_id,
        Some("system:execution"),
        execution_id.map(|id| id.to_string()).as_deref(),
        &payload,
    )
    .await;
    Ok(())
}

fn render_template(template: &str, context: &Map<String, Value>) -> String {
    let mut rendered = template.to_string();
    for (key, value) in context {
        let replacement = escape_adql_string(&value_string(Some(value)).unwrap_or_default());
        rendered = rendered.replace(&format!("{{{key}}}"), &replacement);
    }
    rendered
}

fn prepare_metadata_record(
    source_identifier: &str,
    row: &TapRow,
    config: &ProjectConfig,
    enrichments: &Map<String, Value>,
    discovery_flags: &Value,
    registry: &TransformRegistry,
) -> Value {
    let mut out = Map::new();
    if let Some(field_map) = config
        .discovery
        .prepare_metadata
        .as_ref()
        .and_then(|p| p.field_map.as_object())
    {
        for (target, spec) in field_map {
            if let Some(value) = mapped_value(source_identifier, row, spec, enrichments, registry) {
                out.insert(target.clone(), value);
            }
        }
    }
    out.entry("source_identifier")
        .or_insert_with(|| json!(source_identifier));
    for (key, value) in row {
        out.entry(key.to_ascii_lowercase())
            .or_insert_with(|| value.clone());
    }
    if let Some(sbid) = out.get("sbid").and_then(|v| value_string(Some(v))) {
        for (name, enrichment) in enrichments {
            if let Some(value) = enrichment.get(&sbid) {
                out.insert(name.clone(), value.clone());
            }
        }
    }
    flatten_eval_enrichment(&mut out);
    if let Some(v) = discovery_flags.get("ra_string") {
        out.insert("ra_string".into(), v.clone());
    }
    if let Some(v) = discovery_flags.get("dec_string") {
        out.insert("dec_string".into(), v.clone());
    }
    if let Some(v) = discovery_flags.get("vsys") {
        out.insert("vsys".into(), v.clone());
    }
    Value::Object(out)
}

fn mapped_value(
    source_identifier: &str,
    row: &TapRow,
    spec: &Value,
    enrichments: &Map<String, Value>,
    registry: &TransformRegistry,
) -> Option<Value> {
    let from = spec.get("from").and_then(Value::as_str)?;
    let value = if from == "source_identifier" {
        json!(source_identifier)
    } else if let Some(enrichment_key) = from.strip_prefix("enrichments.") {
        let sbid = row_value(row, "sbid")
            .or_else(|| row_value(row, "obs_id"))
            .and_then(|v| value_string(Some(v)))?;
        enrichments
            .get(enrichment_key)
            .and_then(|map| map.get(&sbid))
            .cloned()
            .unwrap_or(Value::Null)
    } else if let Some(enrichment_key) = from.strip_prefix("enrichment.") {
        let sbid = row_value(row, "sbid")
            .or_else(|| row_value(row, "obs_id"))
            .and_then(|v| value_string(Some(v)))?;
        enrichments
            .get(enrichment_key)
            .and_then(|map| map.get(&sbid))
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        row_value(row, from)?.clone()
    };
    if spec.get("transform").and_then(Value::as_str).is_some() {
        return apply_field_transform(registry, spec, &value);
    }
    Some(value)
}

fn field_map_value(
    row: &TapRow,
    config: &ProjectConfig,
    target: &str,
    registry: &TransformRegistry,
) -> Option<Value> {
    let spec = config
        .discovery
        .prepare_metadata
        .as_ref()?
        .field_map
        .get(target)?;
    mapped_value("", row, spec, &Map::new(), registry)
}

fn discovery_flags_from_config(
    config: &ProjectConfig,
    enrichments: &Map<String, Value>,
    registry: &TransformRegistry,
) -> Value {
    let mut out = Map::new();
    if let Some(flags) = config
        .discovery
        .prepare_metadata
        .as_ref()
        .and_then(|p| p.discovery_flags.as_object())
    {
        let enrichment_value = json!({"enrichments": enrichments});
        for (target, spec) in flags {
            let source = spec.get("from").and_then(Value::as_str).unwrap_or_default();
            let raw = value_at_path(&enrichment_value, source);
            let raw_value = raw.cloned().unwrap_or(Value::Null);
            let value = if spec.get("transform").and_then(Value::as_str).is_some() {
                apply_field_transform(registry, spec, &raw_value).unwrap_or(raw_value)
            } else {
                raw_value
            };
            out.insert(target.clone(), value);
        }
    }
    if let Some(Value::Array(rows)) = enrichments.get("ra_dec_vsys") {
        if let Some(Value::Object(row)) = rows.first() {
            insert_ra_dec_vsys_flags(&mut out, row);
        }
    }
    Value::Object(out)
}

fn flatten_eval_enrichment(out: &mut Map<String, Value>) {
    let eval = out
        .get("sbid_to_eval_file")
        .and_then(Value::as_object)
        .cloned();
    let Some(eval) = eval else {
        return;
    };
    if let Some(url) = eval.get("access_url") {
        out.insert("evaluation_file_access_url".into(), url.clone());
    }
    if let Some(filename) = eval
        .get("filename")
        .or_else(|| eval.get("file_name"))
        .cloned()
    {
        out.entry("evaluation_file".to_string()).or_insert(filename);
    }
}

fn insert_flag_from_row(
    out: &mut Map<String, Value>,
    row: &Map<String, Value>,
    key: &str,
    candidates: &[&str],
) {
    if out.contains_key(key) {
        return;
    }
    for candidate in candidates {
        if let Some(value) = row_value(row, candidate) {
            out.insert(key.into(), value.clone());
            return;
        }
    }
}

fn insert_ra_dec_vsys_flags(out: &mut Map<String, Value>, row: &Map<String, Value>) {
    if let Some(ra_deg) = numeric_row_value(row, &["RAJ2000", "ra_j2000"]) {
        out.insert("ra_string".into(), json!(degrees_to_ra_string(ra_deg)));
    } else {
        insert_flag_from_row(out, row, "ra_string", &["RAJ2000", "ra_j2000"]);
    }
    if let Some(dec_deg) = numeric_row_value(row, &["DEJ2000", "dec_j2000"]) {
        out.insert("dec_string".into(), json!(degrees_to_dec_string(dec_deg)));
    } else {
        insert_flag_from_row(out, row, "dec_string", &["DEJ2000", "dec_j2000"]);
    }
    insert_flag_from_row(out, row, "vsys", &["RVmom", "RV50max", "RV50min"]);
}

fn numeric_row_value(row: &TapRow, candidates: &[&str]) -> Option<f64> {
    for candidate in candidates {
        if let Some(value) = row_value(row, candidate) {
            if let Some(n) = value.as_f64() {
                return Some(n);
            }
            if let Some(s) = value.as_str() {
                if let Ok(n) = s.trim().parse::<f64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Match Python `degrees_to_hms` + `f"{h}h{m}m{s}s"` (seconds rounded to 2 dp).
fn degrees_to_ra_string(degrees: f64) -> String {
    let hours = degrees / 15.0;
    let h = hours.trunc() as i32;
    let rem_h = hours - f64::from(h);
    let m = (rem_h * 60.0).trunc() as i32;
    let s = ((rem_h - f64::from(m) / 60.0) * 3600.0 * 100.0).round() / 100.0;
    format!("{h}h{m}m{s}s")
}

/// Match Python `degrees_to_dms` + `f"{d}.{m}.{s}"` (seconds rounded to 2 dp).
fn degrees_to_dec_string(degrees: f64) -> String {
    let d = degrees.trunc() as i32;
    let rem = (degrees - f64::from(d)).abs();
    let m = (rem * 60.0).trunc() as i32;
    let s = ((rem - f64::from(m) / 60.0) * 3600.0 * 100.0).round() / 100.0;
    format!("{d}.{m}.{s}")
}

fn rows_value(rows: Vec<TapRow>) -> Value {
    Value::Array(rows.into_iter().map(Value::Object).collect())
}

fn row_value<'a>(row: &'a TapRow, key: &str) -> Option<&'a Value> {
    row.get(key)
        .or_else(|| row.get(&key.to_ascii_lowercase()))
        .or_else(|| row.get(&key.to_ascii_uppercase()))
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn value_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }
}

fn escape_adql_string(value: &str) -> String {
    value.replace('\'', "''")
}

fn staging_context_from_metadata(metadata: &[Value]) -> Value {
    let mut data_url_by_scan_id = Map::new();
    let mut checksum_url_by_scan_id = Map::new();
    let mut eval_url_by_sbid = Map::new();
    for record in metadata {
        if let Some(scan) = record.get("scan_id").and_then(Value::as_str) {
            if let Some(url) = record.get("data_url").and_then(Value::as_str) {
                data_url_by_scan_id.insert(scan.into(), json!(url));
            }
            if let Some(url) = record.get("checksum_url").and_then(Value::as_str) {
                checksum_url_by_scan_id.insert(scan.into(), json!(url));
            }
        }
        if let Some(sbid) = record.get("sbid").and_then(Value::as_str) {
            if let Some(url) = record.get("eval_url").and_then(Value::as_str) {
                eval_url_by_sbid.insert(sbid.into(), json!(url));
            }
        }
    }
    json!({
        "data_url_by_scan_id": data_url_by_scan_id,
        "checksum_url_by_scan_id": checksum_url_by_scan_id,
        "eval_url_by_sbid": eval_url_by_sbid,
    })
}

async fn load_wasm_bytes(
    pool: &PgPool,
    config: &ProjectConfig,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let Some(sha) = config
        .extension
        .as_ref()
        .and_then(|e| e.wasm_sha256.as_deref())
    else {
        return Ok(None);
    };
    let Some(row) = repo::get_active_project_config(pool, &config.metadata.id).await? else {
        return Ok(None);
    };
    repo::get_project_config_wasm(pool, row.uuid, sha).await
}

async fn apply_wasm_prepare_metadata(
    pool: &PgPool,
    config: &ProjectConfig,
    envelope: &Value,
    metadata: &[Value],
) -> Result<Vec<Value>, sqlx::Error> {
    let Some(bytes) = load_wasm_bytes(pool, config).await? else {
        return Ok(metadata.to_vec());
    };
    let hooks = config
        .extension
        .as_ref()
        .map(|e| e.hooks.as_slice())
        .unwrap_or_default();
    let host = WasmHost::default();
    let declarative = Value::Array(metadata.to_vec());
    let out = host
        .maybe_apply_hook(
            Some(&bytes),
            hooks,
            HookKind::PrepareMetadata,
            &declarative,
            envelope,
        )
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    match out {
        Value::Array(items) => Ok(items),
        other => Ok(vec![other]),
    }
}

async fn apply_wasm_manifest(
    pool: &PgPool,
    config: &ProjectConfig,
    metadata: &[Value],
    manifest: Value,
) -> Result<Value, sqlx::Error> {
    let Some(bytes) = load_wasm_bytes(pool, config).await? else {
        return Ok(manifest);
    };
    let hooks = config
        .extension
        .as_ref()
        .map(|e| e.hooks.as_slice())
        .unwrap_or_default();
    let host = WasmHost::default();
    let envelope = json!({"metadata": metadata, "project_id": config.metadata.id});
    host.maybe_apply_hook(
        Some(&bytes),
        hooks,
        HookKind::Manifest,
        &manifest,
        &envelope,
    )
    .map_err(|e| sqlx::Error::Protocol(e.to_string()))
}

async fn apply_wasm_graph_patches(
    pool: &PgPool,
    config: &ProjectConfig,
    manifest: &Value,
) -> Result<Value, sqlx::Error> {
    let Some(bytes) = load_wasm_bytes(pool, config).await? else {
        return Ok(manifest.clone());
    };
    let hooks = config
        .extension
        .as_ref()
        .map(|e| e.hooks.as_slice())
        .unwrap_or_default();
    let host = WasmHost::default();
    let envelope = json!({"project_id": config.metadata.id});
    host.maybe_apply_hook(
        Some(&bytes),
        hooks,
        HookKind::GraphPatches,
        manifest,
        &envelope,
    )
    .map_err(|e| sqlx::Error::Protocol(e.to_string()))
}

async fn run_execution_scheduler_tick(
    pool: &PgPool,
    config: &WorkerConfig,
    payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let requested_module = payload
        .get("project_module")
        .and_then(serde_json::Value::as_str);
    let configs = if let Some(module) = requested_module {
        repo::get_active_project_config(pool, module)
            .await?
            .into_iter()
            .collect()
    } else {
        repo::list_active_project_configs(pool).await?
    };
    for project_config in configs {
        let policy = ExecutionAutomationPolicy::from_spec(&project_config.spec);
        if !policy.enabled {
            debug!(
                project_module = project_config.project_id,
                "event=execution_scheduler_disabled"
            );
            continue;
        }
        schedule_project_executions(pool, config, &project_config.project_id, &policy).await?;
    }
    Ok(())
}

async fn schedule_project_executions(
    pool: &PgPool,
    config: &WorkerConfig,
    project_module: &str,
    policy: &ExecutionAutomationPolicy,
) -> Result<(), sqlx::Error> {
    let started = std::time::Instant::now();
    let mut result = SchedulerTickResult::new(project_module);

    if let Some(cap) = policy.concurrent_execution_run_limit {
        let current = repo::count_auto_in_flight_for_module(pool, project_module).await?;
        if !can_admit_by_in_flight(current, cap) {
            result.bump(SkipReason::ProjectInFlightCap);
            info!(
                project_module,
                current,
                cap,
                reason_counts = ?result.reason_counts,
                "event=execution_scheduler_tick_complete"
            );
            flush_execution_scheduler_metrics(pool, project_module, &result, 0, started).await;
            return Ok(());
        }
    }
    if let Some(cap) = config.execution_global_in_flight_limit {
        let current = repo::count_execute_in_flight_runs(pool).await?;
        if !can_admit_by_in_flight(current, cap) {
            result.bump(SkipReason::InFlightCap);
            info!(
                project_module,
                current,
                cap,
                reason_counts = ?result.reason_counts,
                "event=execution_scheduler_tick_complete"
            );
            flush_execution_scheduler_metrics(pool, project_module, &result, 0, started).await;
            return Ok(());
        }
    }
    if let Some(max_depth) = config.execution_queue_max_depth {
        let depth = repo::queue_depth(pool).await?;
        if depth >= max_depth {
            result.bump(SkipReason::QueueFull);
            info!(
                project_module,
                depth,
                max_depth,
                reason_counts = ?result.reason_counts,
                "event=execution_scheduler_tick_complete"
            );
            flush_execution_scheduler_metrics(pool, project_module, &result, 0, started).await;
            return Ok(());
        }
    }

    let (pending_count, oldest_pending_at) =
        repo::get_workflow_pending_stats(pool, project_module).await?;
    if pending_count <= 0 {
        result.bump(SkipReason::NoPendingSources);
        debug!(
            project_module,
            reason_counts = ?result.reason_counts,
            "event=execution_scheduler_tick_complete"
        );
        flush_execution_scheduler_metrics(pool, project_module, &result, 0, started).await;
        return Ok(());
    }
    let max_wait_triggered = oldest_pending_at
        .map(|oldest| {
            oldest <= chrono::Utc::now() - chrono::Duration::minutes(policy.max_wait_minutes)
        })
        .unwrap_or(false);
    if !max_wait_triggered && pending_count < policy.min_sources_to_trigger {
        result.bump(SkipReason::ThresholdNotMet);
        debug!(
            project_module,
            pending_count,
            min_sources_to_trigger = policy.min_sources_to_trigger,
            reason_counts = ?result.reason_counts,
            "event=execution_scheduler_tick_complete"
        );
        flush_execution_scheduler_metrics(pool, project_module, &result, 0, started).await;
        return Ok(());
    }

    let (claim_token, pending_sources) = repo::claim_pending_sources_for_workflow_run(
        pool,
        project_module,
        policy.tick_execution_source_limit,
        policy.claim_ttl_minutes,
    )
    .await?;
    let Some(claim_token) = claim_token else {
        return Ok(());
    };

    let deployment_profile_id = match &policy.deployment_profile_name {
        Some(name) => repo::get_deployment_profile_by_name(pool, name)
            .await?
            .map(|p| p.uuid),
        None => repo::get_default_deployment_profile(pool, project_module)
            .await?
            .map(|p| p.uuid),
    };

    let mut run_limit = policy.tick_execution_run_limit;
    if let Some(cap) = policy.concurrent_execution_run_limit {
        let current = repo::count_auto_in_flight_for_module(pool, project_module).await?;
        let remaining = (cap - current).max(0);
        run_limit = run_limit.min(remaining);
        if run_limit <= 0 {
            result.bump(SkipReason::ProjectInFlightCap);
            info!(
                project_module,
                current,
                cap,
                reason_counts = ?result.reason_counts,
                "event=execution_scheduler_tick_complete"
            );
            flush_execution_scheduler_metrics(pool, project_module, &result, 0, started).await;
            return Ok(());
        }
    }

    run_limit = execute_admission_budget(run_limit);
    if run_limit <= 0 {
        result.bump(SkipReason::RateLimited);
        info!(
            project_module,
            reason_counts = ?result.reason_counts,
            "event=execution_scheduler_tick_complete"
        );
        flush_execution_scheduler_metrics(pool, project_module, &result, 0, started).await;
        return Ok(());
    }

    let mut created_runs = 0_i64;
    let mut admitted_sources = Vec::new();
    for chunk in pending_sources.chunks(policy.max_sources_per_execution as usize) {
        if created_runs >= run_limit {
            break;
        }
        let chunk_sources: Vec<String> = chunk.to_vec();
        let (valid, skipped) =
            repo::partition_sources_ready_for_execution(pool, project_module, &chunk_sources)
                .await?;
        for (source, reason) in skipped {
            result.bump(SkipReason::SourcesSkippedNotReady);
            debug!(
                project_module,
                source_identifier = source,
                reason,
                "event=execution_scheduler_source_skipped_not_ready"
            );
        }
        if valid.is_empty() {
            continue;
        }
        let sources = serde_json::Value::Array(
            valid
                .iter()
                .map(|sid| json!({"source_identifier": sid}))
                .collect(),
        );
        let project_config_id = repo::get_active_project_config(pool, project_module)
            .await?
            .map(|c| c.uuid);
        let execution = repo::create_execution(
            pool,
            project_module,
            sources,
            &policy.archive_name,
            deployment_profile_id,
            project_config_id,
            None,
        )
        .await?;
        repo::apply_execution_patch_with_correlation(
            pool,
            execution.uuid,
            LedgerPatch {
                scheduler_name: Some("workflow_auto".into()),
                workflow_manifest: Some(json!({
                    "beampipe_run_record": {
                        "scheduler": {
                            "policy_decision": "admitted",
                            "claim_token": claim_token,
                            "admitted_source_count": valid.len(),
                            "queue_depth": repo::queue_depth(pool).await.unwrap_or_default(),
                        }
                    }
                })),
                ..beampipe_domain::LedgerPatch::default()
            },
            None,
        )
        .await?;
        repo::enqueue_job(
            pool,
            "execute",
            json!({"execution_id": execution.uuid}),
            Some(execution.uuid),
            Some(&format!("execute:{}", execution.uuid)),
        )
        .await?;
        pacing_sleep(config).await;
        admitted_sources.extend(valid);
        created_runs += 1;
        result.total_jobs += 1;
    }

    repo::clear_workflow_pending_for_sources(pool, project_module, &admitted_sources).await?;
    repo::release_workflow_claim(pool, project_module, &pending_sources, &claim_token).await?;
    result.total_sources = admitted_sources.len() as u64;
    info!(
        project_module,
        created_runs,
        admitted_sources = admitted_sources.len(),
        reason_counts = ?result.reason_counts,
        "event=execution_scheduler_tick_complete"
    );
    flush_execution_scheduler_metrics(pool, project_module, &result, created_runs, started).await;
    Ok(())
}

async fn terminal_execute_failure(
    pool: &PgPool,
    execution_id: uuid::Uuid,
    project_module: &str,
    source_identifiers: &[String],
    error: String,
) -> Result<(), sqlx::Error> {
    error!(
        execution_id = %execution_id,
        project_module,
        error = %error,
        event = "execute_failed"
    );
    repo::apply_execution_patch_with_correlation(
        pool,
        execution_id,
        LedgerPatch {
            status: Some(ExecutionStatus::Failed),
            execution_phase: Some(None),
            error: Some(error.clone()),
            ..LedgerPatch::default()
        },
        None,
    )
    .await?;
    repo::mark_sources_pending_workflow_run(pool, project_module, source_identifiers).await?;
    metrics::record_execute_terminal(project_module, "failed");
    let payload = json!({
        "source_identifiers": source_identifiers,
        "error": error,
    });
    beampipe_db::provenance::record_provenance_event(
        pool,
        beampipe_domain::provenance::ProvenanceEventType::ExecutionFailed.as_str(),
        project_module,
        source_identifiers.first().map(String::as_str),
        Some(execution_id),
        Some("system:execute"),
        Some(&execution_id.to_string()),
        &payload,
    )
    .await;
    let alert = beampipe_alerts::AlertPayload {
        alert: "execution.failed".into(),
        severity: "critical".into(),
        project_module: project_module.to_string(),
        summary: format!("Execution {execution_id} failed: {error}"),
        execution_id: Some(execution_id),
        source_identifiers: source_identifiers.to_vec(),
        discovery_signature: None,
        links: json!({
            "events": format!("/api/v2/executions/{execution_id}/events"),
        }),
        fired_at: chrono::Utc::now().to_rfc3339(),
    };
    let _ = beampipe_alerts::fire_immediate_for_trigger(
        pool,
        "execution_terminal",
        project_module,
        alert,
    )
    .await;
    Ok(())
}

fn profile_tm_url(profile: Option<&DeploymentProfileRow>) -> Option<String> {
    profile.and_then(|p| {
        serde_json::from_value::<beampipe_profiles::DaliugeTranslationConfig>(p.translation.clone())
            .ok()
            .and_then(|t| t.tm_url.filter(|u| !u.trim().is_empty()))
    })
}

fn execution_requires_casda(
    execution: &beampipe_db::models::ExecutionRow,
    project_config: Option<&ProjectConfig>,
) -> bool {
    execution.archive_name == "casda"
        || project_config
            .and_then(|c| c.automation.execution.as_ref())
            .is_some_and(|e| e.archive_name == "casda")
}

async fn preflight_execute(
    do_stage: bool,
    do_submit: bool,
    requires_casda: bool,
    backend_kind: &str,
    profile: Option<&DeploymentProfileRow>,
    casda: Option<&CasdaStagingClient>,
) -> Result<(), String> {
    if do_submit {
        let tm_url = profile_tm_url(profile).unwrap_or_else(|| "http://localhost:9000".into());
        info!(event = "execute_preflight_start", tm_url = %tm_url, backend_kind);
        match probe_tm_reachable(&tm_url, Duration::from_secs(5)).await {
            TmProbeResult::Ok => info!(event = "execute_preflight_tm_ok", tm_url = %tm_url),
            TmProbeResult::NotConfigured => {
                warn!(
                    event = "execute_preflight_tm_skipped",
                    "deployment profile has no translation.tm_url"
                );
            }
            TmProbeResult::Unreachable(detail) => {
                warn!(event = "execute_preflight_tm_failed", tm_url = %tm_url, detail = %detail);
                return Err(tm_unreachable_message(&tm_url, &detail));
            }
        }
        if backend_kind == "slurm_remote" {
            if let Some(profile) = profile {
                if let Ok(DeploymentConfig::SlurmRemote(slurm)) =
                    serde_json::from_value::<DeploymentConfig>(profile.deployment.clone())
                {
                    let user = slurm
                        .remote_user
                        .clone()
                        .or_else(|| std::env::var("SLURM_REMOTE_USER").ok())
                        .unwrap_or_else(|| "root".into());
                    info!(
                        event = "execute_preflight_slurm_start",
                        login_node = %slurm.login_node,
                        remote_user = %user
                    );
                    probe_slurm_login(&slurm, &user).await.map_err(|e| {
                        warn!(event = "execute_preflight_slurm_failed", error = %e);
                        e
                    })?;
                    info!(event = "execute_preflight_slurm_ok", login_node = %slurm.login_node);
                }
            }
        } else if backend_kind == "rest_remote" {
            if let Some(profile) = profile {
                if let Ok(DeploymentConfig::RestRemote(rest)) =
                    serde_json::from_value::<DeploymentConfig>(profile.deployment.clone())
                {
                    if let Some(dim_base) = rest_endpoint(&rest) {
                        info!(event = "execute_preflight_dim_start", dim_url = %dim_base);
                        match probe_dim_reachable(&dim_base, Duration::from_secs(5)).await {
                            TmProbeResult::Ok => {
                                info!(event = "execute_preflight_dim_ok", dim_url = %dim_base);
                            }
                            TmProbeResult::NotConfigured => {
                                warn!(
                                    event = "execute_preflight_dim_skipped",
                                    "rest_remote profile missing deploy_host"
                                );
                            }
                            TmProbeResult::Unreachable(detail) => {
                                warn!(event = "execute_preflight_dim_failed", dim_url = %dim_base, detail = %detail);
                                return Err(dim_unreachable_message(&dim_base, &detail));
                            }
                        }
                    }
                }
            }
        }
    }
    if do_stage && requires_casda {
        match casda {
            Some(client) => {
                info!(event = "execute_preflight_casda_start");
                client.verify_credentials().await.map_err(|e| {
                    warn!(event = "execute_preflight_casda_failed", error = %e);
                    format!("CASDA authentication failed: {e}")
                })?;
                info!(event = "execute_preflight_casda_ok");
            }
            None => {
                return Err(
                    "CASDA_USERNAME/CASDA_PASSWORD required for staging but not configured".into(),
                );
            }
        }
    }
    Ok(())
}

fn correlation_id_from_payload(payload: &serde_json::Value) -> Option<&str> {
    payload
        .get("correlation_id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
}

async fn run_execute(pool: &PgPool, payload: &serde_json::Value) -> Result<(), sqlx::Error> {
    let started = std::time::Instant::now();
    let correlation_id = correlation_id_from_payload(payload);
    let execution_id = payload
        .get("execution_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        .ok_or_else(|| sqlx::Error::Protocol("execute missing execution_id".into()))?;
    let Some(execution) = repo::get_execution(pool, execution_id).await? else {
        return Ok(());
    };
    let project_module = execution.project_module.clone();
    let source_identifiers = source_identifiers_from_json(&execution.sources);
    let result = run_execute_body(pool, execution_id, &execution, payload, correlation_id).await;
    metrics::record_execute_duration("total", started.elapsed().as_secs_f64());
    if let Err(msg) = result {
        terminal_execute_failure(
            pool,
            execution_id,
            &project_module,
            &source_identifiers,
            msg,
        )
        .await?;
    }
    Ok(())
}

async fn run_execute_body(
    pool: &PgPool,
    execution_id: uuid::Uuid,
    execution: &beampipe_db::models::ExecutionRow,
    payload: &serde_json::Value,
    correlation_id: Option<&str>,
) -> Result<(), String> {
    let do_stage = payload
        .get("do_stage")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let do_submit = payload
        .get("do_submit")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let use_real = payload
        .get("use_real_backends")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        });
    let phase_is_submit = execution.phase_enum() == Some(ExecutionPhase::Submit);
    let replay_manifest = phase_is_submit && execution.workflow_manifest.is_some();
    let project_config_row = repo::get_project_config_for_execution(pool, execution)
        .await
        .map_err(|e| e.to_string())?;
    let project_config: Option<ProjectConfig> = project_config_row
        .as_ref()
        .and_then(|row| serde_json::from_value(row.spec.clone()).ok());
    let profile = match execution.deployment_profile_id {
        Some(id) => repo::get_deployment_profile(pool, id)
            .await
            .map_err(|e| e.to_string())?,
        None => repo::get_default_deployment_profile(pool, &execution.project_module)
            .await
            .map_err(|e| e.to_string())?,
    };
    let backend_kind = profile
        .as_ref()
        .and_then(|row| deployment_kind(&row.deployment))
        .unwrap_or("rest_remote");
    let requires_casda = execution_requires_casda(execution, project_config.as_ref());
    let casda_client = CasdaStagingClient::from_env();
    preflight_execute(
        do_stage,
        do_submit,
        requires_casda,
        backend_kind,
        profile.as_ref(),
        casda_client.as_ref(),
    )
    .await?;
    if !replay_manifest {
        repo::apply_execution_patch_with_correlation(
            pool,
            execution_id,
            LedgerPatch {
                status: Some(ExecutionStatus::Running),
                execution_phase: Some(Some(ExecutionPhase::StageAndManifest)),
                ..LedgerPatch::default()
            },
            correlation_id,
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    let source_identifiers = source_identifiers_from_json(&execution.sources);
    let manifest = if replay_manifest {
        execution.workflow_manifest.clone().unwrap_or(json!({}))
    } else {
        let mut skipped_sbids: Vec<String> = Vec::new();
        let metadata_rows = repo::list_archive_metadata_for_sources(
            pool,
            &execution.project_module,
            &source_identifiers,
        )
        .await
        .map_err(|e| e.to_string())?;
        let mut metadata: Vec<_> = metadata_rows
            .into_iter()
            .filter_map(|row| row.metadata_json)
            .flat_map(|value| {
                value
                    .get("datasets")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        if do_stage {
            let staging: Arc<dyn StagingClient> = if let Some(client) = casda_client {
                Arc::new(client)
            } else if requires_casda {
                return Err("CASDA staging credentials required but not configured".into());
            } else {
                Arc::new(PassThroughStagingClient)
            };
            info!(
                event = "execute_stage_start",
                execution_id = %execution_id,
                dataset_count = metadata.len()
            );
            match staging.stage(&metadata).await {
                Ok(outcome) => {
                    info!(
                        event = "execute_stage_complete",
                        execution_id = %execution_id,
                        staged_count = outcome.staged_count,
                        skipped_sbids = ?outcome.skipped_sbids
                    );
                    metadata = outcome.metadata;
                    skipped_sbids = outcome.skipped_sbids;
                }
                Err(err) => return Err(err.to_string()),
            }
        }
        let staging_context = staging_context_from_metadata(&metadata);
        let mut built = if let Some(ref cfg) = project_config {
            build_manifest_from_config_with_staging(
                cfg,
                &metadata,
                &skipped_sbids,
                &staging_context,
            )
            .map_err(|e| e.to_string())?
        } else {
            beampipe_orchestration::build_wallaby_manifest(&metadata).map_err(|e| e.to_string())?
        };
        if let Some(ref cfg) = project_config {
            built = apply_wasm_manifest(pool, cfg, &metadata, built)
                .await
                .map_err(|e| e.to_string())?;
            apply_project_graph_patches(&mut built, cfg);
            built = apply_wasm_graph_patches(pool, cfg, &built)
                .await
                .map_err(|e| e.to_string())?;
        }
        built
    };
    let manifest_path = project_config
        .as_ref()
        .and_then(|c| c.manifest.as_ref())
        .map(|m| m.path.as_str())
        .unwrap_or("manifest.json");
    let graph = if let Some(ref cfg) = project_config {
        resolve_graph(cfg).await.map_err(|e| e.to_string())?
    } else {
        json!({
            "nodeDataArray": [
                {"name": "beampipe-ingest", "fields": []},
                {"name": "Scatter/GenericScatterApp/Beam", "fields": [{"name": "num_of_copies", "type": "Integer"}]}
            ]
        })
    };
    let graph =
        prepare_graph_for_manifest(graph, &manifest, manifest_path).map_err(|e| e.to_string())?;
    if !do_submit {
        repo::apply_execution_patch_with_correlation(
            pool,
            execution_id,
            LedgerPatch {
                status: Some(ExecutionStatus::NotSubmitted),
                execution_phase: Some(None),
                workflow_manifest: Some(manifest),
                ..LedgerPatch::default()
            },
            correlation_id,
        )
        .await
        .map_err(|e| e.to_string())?;
        repo::clear_workflow_pending_for_sources(
            pool,
            &execution.project_module,
            &source_identifiers,
        )
        .await
        .map_err(|e| e.to_string())?;
        repo::set_last_executed_discovery_signature_for_sources(
            pool,
            &execution.project_module,
            &source_identifiers,
        )
        .await
        .map_err(|e| e.to_string())?;
        info!(event = "execute_complete", execution_id = %execution_id, status = "not_submitted");
        metrics::record_execute_terminal(&execution.project_module, "not_submitted");
        return Ok(());
    }
    repo::apply_execution_patch_with_correlation(
        pool,
        execution_id,
        LedgerPatch {
            execution_phase: Some(Some(ExecutionPhase::Submit)),
            workflow_manifest: Some(manifest.clone()),
            ..LedgerPatch::default()
        },
        correlation_id,
    )
    .await
    .map_err(|e| e.to_string())?;
    let session_id = beampipe_session_id(&execution_id.to_string(), execution.created_at);
    if execution.scheduler_name.as_deref() == Some("daliuge")
        && execution.scheduler_job_id.as_deref() == Some(session_id.as_str())
    {
        return Ok(());
    }
    if execution.scheduler_name.as_deref() == Some("slurm")
        && execution
            .scheduler_job_id
            .as_deref()
            .is_some_and(|id| id.starts_with(&session_id))
    {
        return Ok(());
    }
    let created_at = execution.created_at;
    let tm_url = profile_tm_url(profile.as_ref()).unwrap_or_default();
    info!(
        event = "execute_submit_start",
        execution_id = %execution_id,
        backend_kind,
        tm_url = %tm_url
    );
    match backend_kind {
        "slurm_remote" => {
            if use_real {
                let backend = slurm_backend_from_profile(profile.as_ref(), true, created_at);
                let submitted = beampipe_orchestration::ExecutionBackend::submit(
                    &backend,
                    &execution_id.to_string(),
                    manifest,
                    graph,
                )
                .await
                .map_err(|e| e.to_string())?;
                apply_submit_result(pool, execution_id, execution, submitted, use_real)
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                let backend = SlurmExecutionBackend {
                    session_created_at: created_at,
                    ..Default::default()
                };
                let submitted = beampipe_orchestration::ExecutionBackend::submit(
                    &backend,
                    &execution_id.to_string(),
                    manifest,
                    graph,
                )
                .await
                .map_err(|e| e.to_string())?;
                apply_submit_result(pool, execution_id, execution, submitted, use_real)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        _ => {
            if use_real {
                let backend = rest_backend_from_profile(profile.as_ref(), true, created_at);
                let submitted = beampipe_orchestration::ExecutionBackend::submit(
                    &backend,
                    &execution_id.to_string(),
                    manifest,
                    graph,
                )
                .await
                .map_err(|e| e.to_string())?;
                apply_submit_result(pool, execution_id, execution, submitted, use_real)
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                let backend = RestExecutionBackend {
                    session_created_at: created_at,
                    ..Default::default()
                };
                let submitted = beampipe_orchestration::ExecutionBackend::submit(
                    &backend,
                    &execution_id.to_string(),
                    manifest,
                    graph,
                )
                .await
                .map_err(|e| e.to_string())?;
                apply_submit_result(pool, execution_id, execution, submitted, use_real)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    info!(event = "execute_submit_complete", execution_id = %execution_id);
    Ok(())
}

async fn apply_submit_result(
    pool: &PgPool,
    execution_id: uuid::Uuid,
    execution: &beampipe_db::models::ExecutionRow,
    submitted: beampipe_orchestration::BackendSubmit,
    use_real: bool,
) -> Result<(), sqlx::Error> {
    let scheduler_name = submitted.scheduler_name.clone();
    repo::apply_execution_patch_with_correlation(
        pool,
        execution_id,
        LedgerPatch {
            status: Some(submitted.next_status),
            scheduler_name: Some(submitted.scheduler_name),
            scheduler_job_id: submitted.scheduler_job_id.clone(),
            workflow_manifest: Some(submitted.workflow_manifest),
            execution_phase: Some(Some(ExecutionPhase::Submit)),
            ..LedgerPatch::default()
        },
        None,
    )
    .await?;
    if scheduler_name != "slurm" && !use_real {
        repo::enqueue_job(
            pool,
            "dim_poll",
            json!({
                "execution_id": execution_id,
                "poll_round": 0,
                "use_real_backends": use_real,
            }),
            Some(execution_id),
            Some(&format!("dim_poll:{execution_id}:0")),
        )
        .await?;
    }
    let _ = execution;
    Ok(())
}

async fn run_dim_poll(pool: &PgPool, payload: &serde_json::Value) -> Result<(), sqlx::Error> {
    let execution_id = execution_id_from_payload(payload, "dim_poll")?;
    let poll_round = payload
        .get("poll_round")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let Some(execution) = repo::get_execution(pool, execution_id).await? else {
        return Ok(());
    };
    let policy = poll_policy_for_module(pool, &execution.project_module).await?;
    let session_id = execution
        .scheduler_job_id
        .clone()
        .unwrap_or_else(|| execution_id.to_string());
    let use_real = payload
        .get("use_real_backends")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| {
            std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        });
    let poll = if use_real {
        let dim = build_dim_client(&execution, pool).await?;
        dim.poll(&session_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
    } else {
        MockDimClient
            .poll(&session_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
    };
    apply_dim_poll_update(
        pool,
        execution_id,
        &execution,
        poll,
        poll_round,
        &policy,
        None,
        None,
        false,
        use_real,
    )
    .await
}

struct DimPollExec {
    execution: beampipe_db::models::ExecutionRow,
    session_id: String,
    verify_ssl: bool,
}

async fn run_dim_poll_tick(pool: &PgPool, _payload: &serde_json::Value) -> Result<(), sqlx::Error> {
    metrics::set_dim_poll_batch_size(0);
    let use_real = std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let executions = repo::list_rest_executions_pending_poll(pool).await?;
    metrics::set_dim_poll_batch_size(executions.len());
    if executions.is_empty() {
        return Ok(());
    }

    let mut by_endpoint: HashMap<String, Vec<DimPollExec>> = HashMap::new();
    for execution in executions {
        let session_id = match execution.scheduler_job_id.as_deref() {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };
        let profile = match execution.deployment_profile_id {
            Some(id) => repo::get_deployment_profile(pool, id).await?,
            None => repo::get_default_deployment_profile(pool, &execution.project_module).await?,
        };
        let Some(rest) = profile.as_ref().and_then(|p| {
            serde_json::from_value::<DeploymentConfig>(p.deployment.clone())
                .ok()
                .and_then(|d| match d {
                    DeploymentConfig::RestRemote(rest) => Some(rest),
                    _ => None,
                })
        }) else {
            continue;
        };
        let Some(endpoint) = rest_endpoint(&rest) else {
            continue;
        };
        by_endpoint.entry(endpoint).or_default().push(DimPollExec {
            execution,
            session_id,
            verify_ssl: rest.verify_ssl,
        });
    }

    for (endpoint, group) in by_endpoint {
        let verify_ssl = group.first().map(|g| g.verify_ssl).unwrap_or(true);
        let dim = HttpDimClient::with_options(
            &endpoint,
            HttpClientOptions::dim_default().with_verify_ssl(verify_ssl),
        );
        for item in group {
            let execution_id = item.execution.uuid;
            let poll_round =
                dim_poll_round_from_manifest(item.execution.workflow_manifest.as_ref());
            let policy = poll_policy_for_module(pool, &item.execution.project_module).await?;
            let poll = if use_real {
                dim.poll(&item.session_id)
                    .await
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
            } else {
                MockDimClient
                    .poll(&item.session_id)
                    .await
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
            };
            apply_dim_poll_update(
                pool,
                execution_id,
                &item.execution,
                poll,
                poll_round,
                &policy,
                Some(endpoint.as_str()),
                Some(&dim),
                true,
                use_real,
            )
            .await?;
        }
    }
    Ok(())
}

async fn apply_dim_poll_update(
    pool: &PgPool,
    execution_id: uuid::Uuid,
    execution: &beampipe_db::models::ExecutionRow,
    poll: BackendPoll,
    poll_round: i64,
    policy: &PollPolicy,
    dim_base: Option<&str>,
    dim_client: Option<&HttpDimClient>,
    from_tick: bool,
    use_real: bool,
) -> Result<(), sqlx::Error> {
    let max_rounds = policy.rest_max_rounds.unwrap_or(240);
    let interval_secs = policy.rest_interval_secs.unwrap_or(3.0) as i64;
    let correlation = execution_id.to_string();
    let correlation_id = Some(correlation.as_str());
    let session_id = execution
        .scheduler_job_id
        .clone()
        .unwrap_or_else(|| execution_id.to_string());
    let mut manifest = if poll.status.is_terminal() {
        merge_dim_poll_into_manifest(
            execution.workflow_manifest.clone(),
            &session_id,
            poll.poll_summary
                .get("status")
                .and_then(|v| v.get("status").or(Some(v)))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            true,
            Some(match poll.status {
                ExecutionStatus::Completed => "completed",
                ExecutionStatus::Failed => "failed",
                ExecutionStatus::Cancelled => "cancelled",
                _ => "unknown",
            }),
            None,
            poll.poll_summary
                .get("error_drop_uids")
                .and_then(|v| v.as_array())
                .map(|a| a.len() as i64),
        )
    } else {
        merge_poll_summary(
            execution.workflow_manifest.clone(),
            "dim_poll",
            poll.poll_summary,
        )
    };
    if poll.status.is_terminal() {
        if poll.status == ExecutionStatus::Failed {
            if let Some(base) = dim_base {
                let logs_url = dim_logs_url(base, &session_id);
                if let Some(obj) = manifest.as_object_mut() {
                    obj.insert("dim_logs_url".into(), json!(logs_url));
                }
            }
        }
        if dim_destroy_on_terminal() {
            if let Some(client) = dim_client {
                let _ = client.destroy_session(&session_id).await;
            }
        }
        repo::apply_execution_patch_with_correlation(
            pool,
            execution_id,
            LedgerPatch {
                status: Some(poll.status),
                workflow_manifest: Some(manifest),
                ..LedgerPatch::default()
            },
            correlation_id,
        )
        .await?;
        let sources = source_identifiers_from_json(&execution.sources);
        finalize_execution_source_pending(
            pool,
            &execution.project_module,
            &sources,
            poll.status,
            Some(execution_id),
        )
        .await?;
        metrics::record_execute_terminal(&execution.project_module, poll.status.as_str());
        return Ok(());
    }
    if poll_round + 1 >= max_rounds {
        let timed_out =
            merge_scheduler_timeout_into_manifest(Some(manifest), "DIM poll exceeded max rounds");
        repo::apply_execution_patch_with_correlation(
            pool,
            execution_id,
            LedgerPatch {
                status: Some(ExecutionStatus::Failed),
                workflow_manifest: Some(timed_out),
                error: Some("DIM poll timeout".into()),
                ..LedgerPatch::default()
            },
            correlation_id,
        )
        .await?;
        let sources = source_identifiers_from_json(&execution.sources);
        repo::mark_sources_pending_workflow_run(pool, &execution.project_module, &sources).await?;
        metrics::record_execute_terminal(&execution.project_module, "failed");
        return Ok(());
    }
    if from_tick {
        manifest = merge_dim_poll_tick_round(Some(manifest), poll_round + 1);
    }
    repo::apply_execution_patch_with_correlation(
        pool,
        execution_id,
        LedgerPatch {
            status: Some(poll.status),
            workflow_manifest: Some(manifest),
            ..LedgerPatch::default()
        },
        None,
    )
    .await?;
    if !from_tick {
        repo::enqueue_job_deferred(
            pool,
            "dim_poll",
            json!({
                "execution_id": execution_id,
                "poll_round": poll_round + 1,
                "use_real_backends": use_real,
            }),
            interval_secs,
            Some(execution_id),
            Some(&format!("dim_poll:{execution_id}:{}", poll_round + 1)),
        )
        .await?;
    }
    Ok(())
}

fn dim_destroy_on_terminal() -> bool {
    std::env::var("BEAMPIPE_DIM_DESTROY_SESSION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

async fn dim_poll_tick_interval_secs(pool: &PgPool) -> Result<i64, sqlx::Error> {
    if let Ok(v) = std::env::var("BEAMPIPE_DIM_POLL_INTERVAL_SECONDS") {
        if let Ok(secs) = v.parse::<i64>() {
            return Ok(secs.max(1));
        }
    }
    let mut min_interval = 3_i64;
    let configs = repo::list_active_project_configs(pool).await?;
    for row in configs {
        if let Ok(cfg) = serde_json::from_value::<ProjectConfig>(row.spec) {
            if let Some(exec) = cfg.automation.execution {
                if let Some(secs) = exec.execution_rest_remote_poll_interval_seconds {
                    let s = secs.round() as i64;
                    if s >= 1 {
                        min_interval = min_interval.min(s);
                    }
                }
            }
        }
    }
    Ok(min_interval.max(1))
}

async fn slurm_poll_tick_interval_secs(pool: &PgPool) -> Result<i64, sqlx::Error> {
    if let Ok(v) = std::env::var("BEAMPIPE_SLURM_POLL_INTERVAL_SECONDS") {
        if let Ok(secs) = v.parse::<i64>() {
            return Ok(secs.max(5));
        }
    }
    let mut min_interval = 30_i64;
    let configs = repo::list_active_project_configs(pool).await?;
    for row in configs {
        if let Ok(cfg) = serde_json::from_value::<ProjectConfig>(row.spec) {
            if let Some(exec) = cfg.automation.execution {
                if let Some(secs) = exec.execution_slurm_remote_poll_interval_seconds {
                    min_interval = min_interval.min(secs as i64);
                }
            }
        }
    }
    Ok(min_interval.max(5))
}

fn slurm_job_id_from_scheduler(scheduler_job_id: &str) -> String {
    let parsed = beampipe_domain::slurm::parse_scheduler_job_id(scheduler_job_id);
    if parsed.slurm_job_id.is_empty() {
        scheduler_job_id
            .rsplit(':')
            .next()
            .unwrap_or(scheduler_job_id)
            .to_string()
    } else {
        parsed.slurm_job_id
    }
}

fn slurm_poll_is_unknown(result: &SlurmJobPollResult) -> bool {
    result.normalized_state == "UNKNOWN" && result.source == "none"
}

fn execution_status_for_slurm_state(state: &str) -> ExecutionStatus {
    match state {
        "COMPLETED" => ExecutionStatus::Completed,
        "FAILED" | "TIMEOUT" => ExecutionStatus::Failed,
        "CANCELLED" => ExecutionStatus::Cancelled,
        "RUNNING" => ExecutionStatus::Running,
        "PENDING" => ExecutionStatus::AwaitingScheduler,
        _ => ExecutionStatus::AwaitingScheduler,
    }
}

fn terminal_ledger_and_reason(state: &str) -> (&'static str, Option<&'static str>) {
    match state {
        "COMPLETED" => ("completed", None),
        "CANCELLED" => ("cancelled", Some("scheduler_cancelled")),
        "TIMEOUT" => ("failed", Some("timeout")),
        "FAILED" => ("failed", Some("failed")),
        _ => ("failed", Some("unknown")),
    }
}

fn stderr_diagnostics(session_dir: Option<&str>) -> Option<Value> {
    session_dir.filter(|d| !d.trim().is_empty()).map(|d| {
        json!({
            "stderr_glob": format!("{}/logs/err-*.log", d.trim_end_matches('/')),
        })
    })
}

fn manifest_for_slurm_poll(
    execution: &beampipe_db::models::ExecutionRow,
    result: &SlurmJobPollResult,
    scheduler_job_id: &str,
    record_terminal: bool,
    terminal_ledger_status: Option<&str>,
    reason: Option<&str>,
) -> Value {
    let parsed = beampipe_domain::slurm::parse_scheduler_job_id(scheduler_job_id);
    let raw_line = result
        .raw_line
        .as_deref()
        .or(Some(result.raw_state.as_str()));
    merge_slurm_poll_into_manifest(
        execution.workflow_manifest.clone(),
        scheduler_job_id,
        &parsed.slurm_job_id,
        &result.normalized_state,
        result.source,
        raw_line,
        record_terminal,
        terminal_ledger_status,
        SlurmPollManifestOpts {
            exit_code: result.exit_code,
            remote_session_dir: parsed.session_dir.as_deref(),
            reason,
            diagnostics: stderr_diagnostics(parsed.session_dir.as_deref()),
        },
    )
}

async fn apply_slurm_poll_update(
    pool: &PgPool,
    execution_id: uuid::Uuid,
    execution: &beampipe_db::models::ExecutionRow,
    result: &SlurmJobPollResult,
    poll_round: i64,
    policy: &PollPolicy,
) -> Result<(), sqlx::Error> {
    let max_rounds = policy.slurm_max_rounds.unwrap_or(480);
    let correlation = execution_id.to_string();
    let correlation_id = Some(correlation.as_str());
    let scheduler_job_id = execution.scheduler_job_id.clone().unwrap_or_default();
    let parsed = beampipe_domain::slurm::parse_scheduler_job_id(&scheduler_job_id);

    if slurm_poll_is_unknown(result) {
        warn!(
            execution_id = %execution_id,
            slurm_job_id = %parsed.slurm_job_id,
            "event=slurm_poll_state_unknown"
        );
        let manifest =
            manifest_for_slurm_poll(execution, result, &scheduler_job_id, false, None, None);
        repo::apply_execution_patch_with_correlation(
            pool,
            execution_id,
            LedgerPatch {
                workflow_manifest: Some(manifest),
                ..LedgerPatch::default()
            },
            correlation_id,
        )
        .await?;
        return Ok(());
    }

    let state = result.normalized_state.as_str();
    if state == "PENDING" || state == "RUNNING" {
        debug!(
            execution_id = %execution_id,
            slurm_job_id = %parsed.slurm_job_id,
            state,
            source = result.source,
            "event=slurm_poll_active"
        );
        let mut next_status = None;
        if state == "RUNNING" && execution.status_enum() == Some(ExecutionStatus::AwaitingScheduler)
        {
            next_status = Some(ExecutionStatus::Running);
            info!(
                execution_id = %execution_id,
                slurm_job_id = %parsed.slurm_job_id,
                "event=slurm_job_running"
            );
        }
        let mut manifest =
            manifest_for_slurm_poll(execution, result, &scheduler_job_id, false, None, None);
        let next_round = poll_round + 1;
        if next_round >= max_rounds {
            let timed_out = merge_scheduler_timeout_into_manifest(
                Some(manifest),
                "Slurm poll exceeded max rounds",
            );
            repo::apply_execution_patch_with_correlation(
                pool,
                execution_id,
                LedgerPatch {
                    status: Some(ExecutionStatus::Failed),
                    workflow_manifest: Some(timed_out),
                    error: Some("Slurm poll timeout".into()),
                    ..LedgerPatch::default()
                },
                None,
            )
            .await?;
            let sources = source_identifiers_from_json(&execution.sources);
            repo::mark_sources_pending_workflow_run(pool, &execution.project_module, &sources)
                .await?;
            metrics::record_execute_terminal(&execution.project_module, "failed");
            return Ok(());
        }
        manifest = merge_slurm_poll_tick_round(Some(manifest), next_round);
        repo::apply_execution_patch_with_correlation(
            pool,
            execution_id,
            LedgerPatch {
                status: next_status,
                workflow_manifest: Some(manifest),
                ..LedgerPatch::default()
            },
            correlation_id,
        )
        .await?;
        return Ok(());
    }

    let status = execution_status_for_slurm_state(state);
    let (ledger_status, reason) = terminal_ledger_and_reason(state);
    let manifest = manifest_for_slurm_poll(
        execution,
        result,
        &scheduler_job_id,
        true,
        Some(ledger_status),
        reason,
    );
    let mut error = None;
    if status == ExecutionStatus::Failed {
        let reason_str = reason.unwrap_or(state);
        let mut msg = format!(
            "SLURM job {} finished in state={state} reason={reason_str}",
            parsed.slurm_job_id
        );
        if let Some(code) = result.exit_code {
            msg.push_str(&format!(" exit_code={code}"));
        }
        if let Some(dir) = parsed.session_dir.as_deref().filter(|d| !d.is_empty()) {
            msg.push_str(&format!(
                " stderr_glob={}/logs/err-*.log",
                dir.trim_end_matches('/')
            ));
        }
        error = Some(msg);
    }
    repo::apply_execution_patch_with_correlation(
        pool,
        execution_id,
        LedgerPatch {
            status: Some(status),
            workflow_manifest: Some(manifest),
            error,
            ..LedgerPatch::default()
        },
        None,
    )
    .await?;
    let sources = source_identifiers_from_json(&execution.sources);
    finalize_execution_source_pending(
        pool,
        &execution.project_module,
        &sources,
        status,
        Some(execution_id),
    )
    .await?;
    metrics::record_execute_terminal(&execution.project_module, status.as_str());
    Ok(())
}

struct SlurmPollExec {
    execution: beampipe_db::models::ExecutionRow,
    slurm_job_id: String,
}

async fn run_slurm_poll_tick(
    pool: &PgPool,
    _payload: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    metrics::set_slurm_poll_batch_size(0);
    let use_real = std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let executions = repo::list_slurm_executions_pending_poll(pool).await?;
    metrics::set_slurm_poll_batch_size(executions.len());
    if executions.is_empty() {
        return Ok(());
    }

    let mut by_target: HashMap<SlurmTarget, Vec<SlurmPollExec>> = HashMap::new();
    for execution in executions {
        let scheduler_job_id = match execution.scheduler_job_id.as_deref() {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };
        let slurm_job_id = slurm_job_id_from_scheduler(scheduler_job_id);
        let profile = match execution.deployment_profile_id {
            Some(id) => repo::get_deployment_profile(pool, id).await?,
            None => repo::get_default_deployment_profile(pool, &execution.project_module).await?,
        };
        let Some(profile) = profile else {
            continue;
        };
        let Ok(DeploymentConfig::SlurmRemote(deployment)) =
            serde_json::from_value::<DeploymentConfig>(profile.deployment.clone())
        else {
            continue;
        };
        let username = resolve_remote_user(&deployment);
        let target = SlurmTarget::from_deployment(&deployment, &username);
        by_target.entry(target).or_default().push(SlurmPollExec {
            execution,
            slurm_job_id,
        });
    }

    for (target, group) in by_target {
        metrics::set_slurm_ssh_sessions_active(
            &target.login_node,
            SLURM_SSH_POOL.active_session_count(),
        );
        let job_ids: Vec<String> = group.iter().map(|e| e.slurm_job_id.clone()).collect();
        let poll_map: HashMap<String, SlurmJobPollResult> = if use_real {
            let lock_key = target.advisory_lock_key();
            if !repo::try_pg_advisory_lock(pool, lock_key).await? {
                debug!(
                    login_node = %target.login_node,
                    "event=slurm_poll_tick_lock_busy"
                );
                metrics::record_slurm_poll_error("advisory_lock_busy");
                continue;
            }
            let batch_result = SLURM_SSH_POOL
                .query_slurm_states(&target, &job_ids)
                .await
                .map_err(|e| {
                    metrics::record_slurm_poll_error("ssh_batch_failed");
                    sqlx::Error::Protocol(e.to_string())
                });
            let _ = repo::pg_advisory_unlock(pool, lock_key).await;
            batch_result?
        } else {
            job_ids
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        SlurmJobPollResult {
                            raw_state: "COMPLETED".into(),
                            normalized_state: "COMPLETED".into(),
                            source: "mock",
                            exit_code: Some(0),
                            raw_line: Some("COMPLETED".into()),
                        },
                    )
                })
                .collect()
        };

        for item in group {
            let execution_id = item.execution.uuid;
            let poll_round =
                slurm_poll_round_from_manifest(item.execution.workflow_manifest.as_ref());
            let policy = poll_policy_for_module(pool, &item.execution.project_module).await?;
            let result = poll_map
                .get(&item.slurm_job_id)
                .cloned()
                .unwrap_or(SlurmJobPollResult {
                    raw_state: String::new(),
                    normalized_state: "UNKNOWN".into(),
                    source: "none",
                    exit_code: None,
                    raw_line: None,
                });
            apply_slurm_poll_update(
                pool,
                execution_id,
                &item.execution,
                &result,
                poll_round,
                &policy,
            )
            .await?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct PollPolicy {
    rest_max_rounds: Option<i64>,
    slurm_max_rounds: Option<i64>,
    rest_interval_secs: Option<f64>,
    slurm_interval_secs: Option<f64>,
}

async fn poll_policy_for_module(
    pool: &PgPool,
    project_module: &str,
) -> Result<PollPolicy, sqlx::Error> {
    let mut policy = PollPolicy::default();
    if let Some(row) = repo::get_active_project_config(pool, project_module).await? {
        if let Ok(cfg) = serde_json::from_value::<ProjectConfig>(row.spec) {
            if let Some(exec) = cfg.automation.execution {
                policy.rest_max_rounds = exec.execution_rest_remote_poll_max_rounds;
                policy.slurm_max_rounds = exec.execution_slurm_remote_poll_max_rounds;
                policy.rest_interval_secs = exec.execution_rest_remote_poll_interval_seconds;
                policy.slurm_interval_secs = exec.execution_slurm_remote_poll_interval_seconds;
            }
        }
    }
    Ok(policy)
}

async fn build_dim_client(
    execution: &beampipe_db::models::ExecutionRow,
    pool: &PgPool,
) -> Result<HttpDimClient, sqlx::Error> {
    let profile = match execution.deployment_profile_id {
        Some(id) => repo::get_deployment_profile(pool, id).await?,
        None => repo::get_default_deployment_profile(pool, &execution.project_module).await?,
    };
    let (endpoint, verify_ssl) = profile
        .as_ref()
        .and_then(|p| {
            serde_json::from_value::<DeploymentConfig>(p.deployment.clone())
                .ok()
                .and_then(|d| match d {
                    DeploymentConfig::RestRemote(rest) => {
                        rest_endpoint(&rest).map(|ep| (ep, rest.verify_ssl))
                    }
                    _ => None,
                })
        })
        .unwrap_or_else(|| ("http://localhost:8000".into(), true));
    Ok(HttpDimClient::with_options(
        endpoint,
        HttpClientOptions::dim_default().with_verify_ssl(verify_ssl),
    ))
}

fn tm_http_options(profile: Option<&DeploymentProfileRow>) -> HttpClientOptions {
    let mut opts = HttpClientOptions::translator_default();
    let Some(profile) = profile else {
        return opts;
    };
    if let Ok(DeploymentConfig::RestRemote(rest)) =
        serde_json::from_value::<DeploymentConfig>(profile.deployment.clone())
    {
        opts.verify_ssl = rest.verify_ssl;
    } else if let Ok(DeploymentConfig::SlurmRemote(slurm)) =
        serde_json::from_value::<DeploymentConfig>(profile.deployment.clone())
    {
        if let Some(v) = slurm.verify_ssl {
            opts.verify_ssl = v;
        }
    }
    opts
}

fn execution_id_from_payload(
    payload: &serde_json::Value,
    job_kind: &str,
) -> Result<uuid::Uuid, sqlx::Error> {
    payload
        .get("execution_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        .ok_or_else(|| sqlx::Error::Protocol(format!("{job_kind} missing execution_id")))
}

fn source_identifiers_from_json(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("source_identifier")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

fn deployment_kind(value: &serde_json::Value) -> Option<&'static str> {
    match serde_json::from_value::<DeploymentConfig>(value.clone()).ok()? {
        DeploymentConfig::RestRemote(_) => Some("rest_remote"),
        DeploymentConfig::SlurmRemote(_) => Some("slurm_remote"),
    }
}

fn rest_backend_from_profile(
    profile: Option<&DeploymentProfileRow>,
    use_real: bool,
    created_at: chrono::DateTime<chrono::Utc>,
) -> RestExecutionBackend<HttpTranslatorClient, HttpDimClient> {
    let translation = profile.and_then(|p| {
        serde_json::from_value::<beampipe_profiles::DaliugeTranslationConfig>(p.translation.clone())
            .ok()
    });
    let tm_url = translation.as_ref().and_then(|t| t.tm_url.clone());
    let (dim_host, dim_port) = profile
        .and_then(|p| {
            serde_json::from_value::<DeploymentConfig>(p.deployment.clone())
                .ok()
                .and_then(|d| match d {
                    DeploymentConfig::RestRemote(rest) => {
                        Some((rest.dim_host_for_tm.clone(), rest.dim_port_for_tm))
                    }
                    _ => None,
                })
        })
        .unwrap_or((None, None));
    let dim_endpoint = profile.and_then(|p| {
        serde_json::from_value::<DeploymentConfig>(p.deployment.clone())
            .ok()
            .and_then(|d| match d {
                DeploymentConfig::RestRemote(rest) => rest_endpoint(&rest),
                _ => None,
            })
    });
    let translate_config = translation
        .as_ref()
        .map(|t| translate_config_from_profile(t, dim_host.as_deref(), dim_port, false))
        .unwrap_or_default();
    let tm_opts = tm_http_options(profile);
    let dim_verify = profile
        .and_then(|p| {
            serde_json::from_value::<DeploymentConfig>(p.deployment.clone())
                .ok()
                .and_then(|d| match d {
                    DeploymentConfig::RestRemote(rest) => Some(rest.verify_ssl),
                    _ => None,
                })
        })
        .unwrap_or(true);
    RestExecutionBackend {
        translator: if use_real {
            HttpTranslatorClient::with_options(
                tm_url
                    .clone()
                    .unwrap_or_else(|| "http://localhost:9000".into()),
                tm_opts.clone(),
            )
        } else {
            HttpTranslatorClient::with_options("http://localhost:9000", tm_opts.clone())
        },
        dim: if use_real {
            HttpDimClient::with_options(
                dim_endpoint
                    .clone()
                    .unwrap_or_else(|| "http://localhost:8000".into()),
                HttpClientOptions::dim_default().with_verify_ssl(dim_verify),
            )
        } else {
            HttpDimClient::with_options(
                "http://localhost:8000",
                HttpClientOptions::dim_default().with_verify_ssl(dim_verify),
            )
        },
        profile_name: profile.map(|p| p.name.clone()),
        tm_url,
        dim_endpoint,
        translate_config,
        session_created_at: created_at,
    }
}

fn slurm_backend_from_profile(
    profile: Option<&DeploymentProfileRow>,
    _use_real: bool,
    created_at: chrono::DateTime<chrono::Utc>,
) -> SlurmExecutionBackend<HttpTranslatorClient, SshSlurmClient> {
    let mut session_dir = "/tmp/beampipe".to_string();
    let mut login = "localhost".to_string();
    let mut remote_user = None;
    let mut account = None;
    let mut slurm_dep: Option<SlurmRemoteDeploymentConfig> = None;
    let translation = profile.and_then(|p| {
        serde_json::from_value::<beampipe_profiles::DaliugeTranslationConfig>(p.translation.clone())
            .ok()
    });
    let tm_url = translation.as_ref().and_then(|t| t.tm_url.clone());
    if let Some(profile) = profile {
        if let Ok(DeploymentConfig::SlurmRemote(slurm)) =
            serde_json::from_value::<DeploymentConfig>(profile.deployment.clone())
        {
            session_dir = format!(
                "{}/beampipe/{}",
                slurm.log_dir.trim_end_matches('/'),
                chrono::Utc::now().format("%Y%m%d")
            );
            login = slurm.login_node.clone();
            remote_user = slurm.remote_user.clone();
            account = Some(slurm.account.clone());
            slurm_dep = Some(slurm);
        }
    }
    let translate_config = translation
        .as_ref()
        .map(|t| translate_config_from_profile(t, None, None, true))
        .unwrap_or_else(|| {
            translate_config_from_profile(
                &beampipe_profiles::DaliugeTranslationConfig {
                    tm_url: tm_url.clone(),
                    ..Default::default()
                },
                None,
                None,
                true,
            )
        });
    SlurmExecutionBackend {
        translator: HttpTranslatorClient::with_options(
            tm_url.unwrap_or_else(|| "http://localhost:9000".into()),
            tm_http_options(profile),
        ),
        slurm: SshSlurmClient {
            login_node: login.clone(),
            remote_user: remote_user.clone(),
            session_dir: session_dir.clone(),
            account: account.clone(),
            ssh_port: slurm_dep.as_ref().map(|s| s.ssh_port).unwrap_or(22),
            dlg_root: slurm_dep
                .as_ref()
                .map(|s| s.dlg_root.clone())
                .unwrap_or_else(|| "/tmp".into()),
            deployment: slurm_dep,
        },
        profile_name: profile.map(|p| p.name.clone()),
        session_dir,
        login_node: Some(login),
        remote_user,
        account,
        translate_config,
        session_created_at: created_at,
    }
}

fn rest_endpoint(rest: &RestRemoteDeploymentConfig) -> Option<String> {
    let host = rest.deploy_host.as_deref()?.trim();
    if host.is_empty() {
        return None;
    }
    let port = rest.deploy_port.unwrap_or(8001);
    Some(if port == 80 {
        format!("http://{host}")
    } else {
        format!("http://{host}:{port}")
    })
}

fn merge_poll_summary(
    existing: Option<serde_json::Value>,
    key: &str,
    summary: serde_json::Value,
) -> serde_json::Value {
    let mut manifest = existing.unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = manifest.as_object_mut() else {
        return serde_json::json!({"beampipe_run_record": {key: summary}});
    };
    let rr = obj
        .entry("beampipe_run_record")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(rr) = rr.as_object_mut() {
        rr.insert(key.into(), summary);
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;
    use beampipe_adapters::MockTapClient;
    use serde_json::json;

    #[test]
    fn slurm_unknown_does_not_map_to_terminal() {
        let result = SlurmJobPollResult {
            raw_state: String::new(),
            normalized_state: "UNKNOWN".into(),
            source: "none",
            exit_code: None,
            raw_line: None,
        };
        assert!(slurm_poll_is_unknown(&result));
        assert!(!execution_status_for_slurm_state(&result.normalized_state).is_terminal());
    }

    #[test]
    fn slurm_timeout_maps_to_failed_status() {
        assert_eq!(
            execution_status_for_slurm_state("TIMEOUT"),
            ExecutionStatus::Failed
        );
        assert_eq!(
            terminal_ledger_and_reason("TIMEOUT"),
            ("failed", Some("timeout"))
        );
    }

    #[test]
    fn execution_policy_defaults_disabled() {
        let policy = ExecutionAutomationPolicy::from_spec(&json!({}));
        assert!(!policy.enabled);
        assert_eq!(policy.archive_name, "casda");
        assert_eq!(policy.max_sources_per_execution, 20);
    }

    #[test]
    fn execution_policy_reads_wallaby_shape() {
        let policy = ExecutionAutomationPolicy::from_spec(&json!({
            "automation": {
                "execution": {
                    "enabled": true,
                    "archive_name": "casda",
                    "max_sources_per_execution": 1,
                    "tick_execution_source_limit": 200,
                    "tick_execution_run_limit": 5,
                    "min_sources_to_trigger": 1,
                    "max_wait_minutes": 1440,
                    "claim_ttl_minutes": 180,
                    "concurrent_execution_run_limit": 5,
                    "deployment_profile_name": "slurm-remote"
                }
            }
        }));
        assert!(policy.enabled);
        assert_eq!(policy.max_sources_per_execution, 1);
        assert_eq!(policy.tick_execution_run_limit, 5);
        assert_eq!(policy.concurrent_execution_run_limit, Some(5));
        assert_eq!(
            policy.deployment_profile_name.as_deref(),
            Some("slurm-remote")
        );
    }

    #[tokio::test]
    async fn config_discovery_uses_project_query_templates() {
        let mut clients: BTreeMap<String, Arc<dyn TapClient>> = BTreeMap::new();
        let mut casda = MockTapClient::default();
        casda.insert_rows(
            "ivoa.obscore",
            vec![json!({"filename": "HIPASSJ1.ms", "obs_id": "ASKAP-123", "obs_publisher_did": "scan-9"})],
        );
        casda.insert_rows(
            "observation_evaluation_file",
            vec![json!({"sbid": "123", "access_url": "https://x"})],
        );
        let vizier = MockTapClient::with_rows(
            "VIII/73/hicat",
            vec![json!({"RAJ2000": "1:2:3", "DEJ2000": "-1:2:3", "RVmom": 42})],
        );
        clients.insert("casda".into(), Arc::new(casda));
        clients.insert("vizier".into(), Arc::new(vizier));
        let runner = ConfigDiscoveryRunner::with_clients(clients);
        let config =
            ProjectConfig::from_slice(include_bytes!("../../../config/wallaby_hires.v1.yaml"))
                .unwrap();
        let result = runner
            .discover_source(Some(&config), "wallaby_hires", "HIPASSJ1")
            .await;
        match result {
            DiscoverySourceResult::HasMetadata {
                metadata,
                discovery_flags,
                ..
            } => {
                assert_eq!(metadata[0]["sbid"], "123");
                assert_eq!(metadata[0]["dataset_id"], "HIPASSJ1.ms");
                assert_eq!(discovery_flags["ra_dec_vsys_complete"], true);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn deployment_kind_reads_typed_profile_json() {
        assert_eq!(
            deployment_kind(
                &json!({"kind": "slurm_remote", "login_node": "setonix", "account": "a", "home_dir": "/h", "log_dir": "/l", "dlg_root": "/d"})
            ),
            Some("slurm_remote")
        );
    }

    #[test]
    fn worker_config_reads_concurrency_from_settings() {
        std::env::set_var("DATABASE_URL", "postgres://localhost/test");
        std::env::set_var("BEAMPIPE_WORKER_CONCURRENCY", "8");
        std::env::set_var("BEAMPIPE_WORKER_SCHEDULER_ENABLED", "false");
        std::env::set_var("BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY", "12");
        let settings = beampipe_config::Settings::from_env().unwrap();
        let cfg = WorkerConfig::from_settings(&settings);
        assert_eq!(cfg.concurrency, 8);
        assert!(!cfg.scheduler_enabled);
        assert_eq!(cfg.discovery_source_concurrency, 12);
        std::env::remove_var("BEAMPIPE_WORKER_CONCURRENCY");
        std::env::remove_var("BEAMPIPE_WORKER_SCHEDULER_ENABLED");
        std::env::remove_var("BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY");
    }

    #[tokio::test]
    async fn discover_sources_parallel_returns_all_results() {
        let runner = DeterministicDiscoveryRunner;
        let sources: Vec<String> = (0..8).map(|i| format!("src-{i}")).collect();
        let results = discover_sources_parallel(&runner, None, "mod", sources, 4).await;
        assert_eq!(results.len(), 8);
    }
}

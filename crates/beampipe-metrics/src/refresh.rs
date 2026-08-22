//! Periodic refresh of DB-backed Prometheus gauges and dependency probes.

use beampipe_adapters::probe_tap_health;
use beampipe_db::models::DeploymentProfileRow;
use beampipe_db::test_modules::is_integration_test_project_module;
use beampipe_orchestration::cancel::rest_endpoint;
use beampipe_orchestration::slurm_credentials::SlurmSshCredentials;
use beampipe_orchestration::slurm_deploy::{probe_slurm_login, resolve_remote_user};
use beampipe_orchestration::tm_health::{probe_dim_reachable, probe_tm_reachable, TmProbeResult};
use beampipe_profiles::{DaliugeTranslationConfig, DeploymentConfig, SlurmRemoteDeploymentConfig};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

static LAST_SOURCE_PROCESSING_KEYS: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static LAST_PROFILE_DEPENDENCY_KEYS: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static LAST_JOB_KIND_KEYS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static LAST_EXECUTION_STATUS_KEYS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn is_internal_test_module(module: &str) -> bool {
    is_integration_test_project_module(module)
}

fn phase_priority(phase: &str) -> u8 {
    match phase {
        "executing" => 3,
        "admitting" => 2,
        "discovering" => 1,
        _ => 0,
    }
}

#[derive(Debug, Clone)]
enum ProfileProbe {
    Http {
        dependency: &'static str,
        url: String,
    },
    Slurm(Box<SlurmRemoteDeploymentConfig>),
}

impl ProfileProbe {
    fn dependency(&self) -> &'static str {
        match self {
            Self::Http { dependency, .. } => dependency,
            Self::Slurm(_) => "slurm",
        }
    }
}

/// Probe Postgres and optional TAP endpoints; update `beampipe_dependency_up`.
pub async fn refresh_dependencies(pool: &PgPool) {
    let postgres_ok = sqlx::query("SELECT 1").execute(pool).await.is_ok();
    crate::set_dependency_up("postgres", postgres_ok);

    let timeout_secs = std::env::var("BEAMPIPE_DISCOVERY_TAP_HEALTH_TIMEOUT_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let casda_url = std::env::var("BEAMPIPE_CASDA_TAP_URL").ok();
    let vizier_url = std::env::var("BEAMPIPE_VIZIER_TAP_URL").ok();
    let tap_report = probe_tap_health(
        casda_url.as_deref().filter(|u| !u.is_empty()),
        vizier_url.as_deref().filter(|u| !u.is_empty()),
        Duration::from_secs(timeout_secs),
    )
    .await;
    crate::set_dependency_up(
        "casda",
        tap_report.casda.reachable || !tap_report.casda.configured,
    );
    crate::set_dependency_up(
        "vizier",
        tap_report.vizier.reachable || !tap_report.vizier.configured,
    );

    let redis_up = match std::env::var("BEAMPIPE_REDIS_URL") {
        Ok(url) if !url.is_empty() => {
            if let Ok(client) = redis::Client::open(url.as_str()) {
                if let Ok(mut conn) = client.get_multiplexed_async_connection().await {
                    redis::cmd("PING")
                        .query_async::<()>(&mut conn)
                        .await
                        .is_ok()
                } else {
                    false
                }
            } else {
                false
            }
        }
        _ => true,
    };
    crate::set_dependency_up("redis", redis_up);

    let fallback_tm_url = std::env::var("BEAMPIPE_TM_HEALTH_URL")
        .or_else(|_| std::env::var("BEAMPIPE_TM_URL"))
        .ok()
        .filter(|url| !url.trim().is_empty());
    let fallback_dim_url = std::env::var("BEAMPIPE_DIM_HEALTH_URL")
        .or_else(|_| std::env::var("BEAMPIPE_DIM_URL"))
        .ok()
        .filter(|url| !url.trim().is_empty());
    refresh_profile_dependencies(
        pool,
        fallback_tm_url.as_deref(),
        fallback_dim_url.as_deref(),
        Duration::from_secs(timeout_secs),
    )
    .await;
}

async fn active_deployment_profiles(
    pool: &PgPool,
) -> Result<Vec<DeploymentProfileRow>, sqlx::Error> {
    sqlx::query_as::<_, DeploymentProfileRow>(
        r#"
        SELECT profile.*
        FROM daliuge_deployment_profile profile
        WHERE profile.is_default = true
           OR EXISTS (
                SELECT 1
                FROM batch_execution_record execution
                WHERE execution.deployment_profile_id = profile.uuid
                  AND execution.status IN (
                      'pending', 'running', 'awaiting_scheduler', 'retrying'
                  )
           )
        ORDER BY profile.name ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

fn profile_probes(
    profile: &DeploymentProfileRow,
    fallback_tm_url: Option<&str>,
    fallback_dim_url: Option<&str>,
) -> Result<Vec<ProfileProbe>, serde_json::Error> {
    let translation: DaliugeTranslationConfig =
        serde_json::from_value(profile.translation.clone())?;
    let deployment: DeploymentConfig = serde_json::from_value(profile.deployment.clone())?;
    let mut probes = Vec::new();
    if let Some(url) = translation
        .tm_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .or(fallback_tm_url)
    {
        probes.push(ProfileProbe::Http {
            dependency: "tm",
            url: url.to_string(),
        });
    }
    match deployment {
        DeploymentConfig::RestRemote(rest) => {
            if let Some(url) = rest_endpoint(&rest).or_else(|| fallback_dim_url.map(str::to_string))
            {
                probes.push(ProfileProbe::Http {
                    dependency: "dim",
                    url,
                });
            }
        }
        DeploymentConfig::SlurmRemote(slurm) => probes.push(ProfileProbe::Slurm(Box::new(slurm))),
    }
    Ok(probes)
}

async fn run_profile_probe(probe: &ProfileProbe, timeout: Duration) -> bool {
    match probe {
        ProfileProbe::Http {
            dependency: "tm",
            url,
        } => matches!(
            probe_tm_reachable(url, timeout).await,
            TmProbeResult::Ok | TmProbeResult::NotConfigured
        ),
        ProfileProbe::Http { url, .. } => matches!(
            probe_dim_reachable(url, timeout).await,
            TmProbeResult::Ok | TmProbeResult::NotConfigured
        ),
        ProfileProbe::Slurm(slurm) => {
            let username = resolve_remote_user(slurm);
            matches!(
                tokio::time::timeout(timeout, probe_slurm_login(slurm, &username)).await,
                Ok(Ok(()))
            )
        }
    }
}

async fn refresh_profile_dependencies(
    pool: &PgPool,
    fallback_tm_url: Option<&str>,
    fallback_dim_url: Option<&str>,
    timeout: Duration,
) {
    let profiles = match active_deployment_profiles(pool).await {
        Ok(profiles) => profiles,
        Err(error) => {
            tracing::warn!(event = "metrics_profile_query_failed", %error);
            return;
        }
    };
    let mut current = HashSet::new();
    let mut tasks = tokio::task::JoinSet::new();
    let mut slurm_credentials_configured = true;
    for profile in profiles {
        let probes = match profile_probes(&profile, fallback_tm_url, fallback_dim_url) {
            Ok(probes) => probes,
            Err(error) => {
                current.insert((profile.name.clone(), "configuration".to_string()));
                crate::set_deployment_profile_dependency_up(&profile.name, "configuration", false);
                tracing::warn!(
                    event = "metrics_profile_parse_failed",
                    profile = %profile.name,
                    %error
                );
                continue;
            }
        };
        for probe in probes {
            if let ProfileProbe::Slurm(slurm) = &probe {
                slurm_credentials_configured &=
                    SlurmSshCredentials::resolve_for(slurm.ssh_credential.as_deref()).is_ok();
            }
            let profile_name = profile.name.clone();
            let dependency = probe.dependency().to_string();
            current.insert((profile_name.clone(), dependency.clone()));
            tasks.spawn(async move {
                let up = run_profile_probe(&probe, timeout).await;
                (profile_name, dependency, up)
            });
        }
    }
    crate::set_slurm_ssh_configured(slurm_credentials_configured);
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((profile, dependency, up)) => {
                crate::set_deployment_profile_dependency_up(&profile, &dependency, up);
            }
            Err(error) => tracing::warn!(event = "metrics_profile_probe_join_failed", %error),
        }
    }
    zero_missing_profile_dependency_gauges(&current);
}

fn zero_missing_profile_dependency_gauges(current: &HashSet<(String, String)>) {
    let mut last = LAST_PROFILE_DEPENDENCY_KEYS
        .lock()
        .expect("profile dependency metric keys lock");
    for (profile, dependency) in last.difference(current) {
        // Deleted or inactive profiles must not leave a permanently firing
        // time series behind in this long-lived process.
        crate::set_deployment_profile_dependency_up(profile, dependency, true);
    }
    *last = current.clone();
}

/// Refresh queue, pending backlog, and execution gauges from Postgres.
pub async fn refresh_gauges_from_pool(pool: &PgPool) {
    refresh_dependencies(pool).await;

    if let Ok(depth) = beampipe_db::repo::runnable_queue_depth(pool).await {
        crate::set_jobs_queue_depth(depth);
    }
    if let Ok(running) = beampipe_db::repo::jobs_running_count(pool).await {
        crate::set_jobs_running(running);
    }
    let mut current_job_kinds = HashSet::new();
    if let Ok(by_kind) = beampipe_db::repo::queue_depth_by_kind(pool).await {
        for (kind, count) in by_kind {
            current_job_kinds.insert(kind.clone());
            crate::set_jobs_queued_by_kind(&kind, count);
        }
    }
    if let Ok(by_kind) = beampipe_db::repo::oldest_queued_job_age_by_kind(pool).await {
        for (kind, age) in by_kind {
            current_job_kinds.insert(kind.clone());
            crate::set_oldest_queued_job_age_seconds(&kind, age);
        }
    }
    zero_missing_job_kind_gauges(&current_job_kinds);

    let mut current_statuses = HashSet::new();
    if let Ok(by_status) = beampipe_db::repo::execution_counts_by_status(pool).await {
        for (status, count) in by_status {
            current_statuses.insert(status.clone());
            crate::set_executions_active_by_status(&status, count);
        }
    }
    if let Ok(by_status) = beampipe_db::repo::oldest_active_execution_age_by_status(pool).await {
        for (status, age) in by_status {
            current_statuses.insert(status.clone());
            crate::set_oldest_active_execution_age_seconds(&status, age);
        }
    }
    zero_missing_execution_status_gauges(&current_statuses);

    if let Ok(by_scheduler) = beampipe_db::repo::execution_counts_by_scheduler_name(pool).await {
        for (scheduler_name, count) in by_scheduler {
            crate::set_executions_inflight_by_scheduler(&scheduler_name, count);
        }
    }
    if let Ok(by_axis) = beampipe_db::repo::execution_counts_by_external_axis(pool).await {
        for (axis, state, count) in by_axis {
            crate::set_execution_axis_count(&axis, &state, count);
        }
    }
    if let Ok(count) = beampipe_db::repo::reconciliation_risk_count(pool).await {
        crate::set_reconciliation_risk_count(count);
    }
    if let Ok(count) = beampipe_db::repo::execution_retry_total(pool).await {
        crate::set_execution_retry_total(count);
    }
    if let Ok(workers) = beampipe_db::repo::list_worker_instances(pool, false).await {
        for worker in workers {
            let worker_id = worker.uuid.to_string();
            let heartbeat_age = chrono::Utc::now()
                .signed_duration_since(worker.last_heartbeat_at)
                .num_seconds();
            crate::set_worker_heartbeat_age(&worker_id, &worker.pool, heartbeat_age);
            if let Ok(leases) =
                beampipe_db::repo::active_worker_lease_count(pool, worker.uuid).await
            {
                crate::set_worker_active_leases(&worker_id, &worker.pool, leases);
            }
        }
    }

    let mut pending_by_module: HashMap<String, i64> = HashMap::new();
    if let Ok(pending) = beampipe_db::repo::workflow_pending_counts_by_module(pool).await {
        for (module, count) in pending {
            if is_internal_test_module(&module) {
                continue;
            }
            pending_by_module.insert(module.clone(), count);
            crate::set_workflow_pending_sources(&module, count);
        }
    }

    let mut age_by_module: HashMap<String, i64> = HashMap::new();
    if let Ok(ages) = beampipe_db::repo::max_pending_age_by_module(pool).await {
        for (module, age) in ages {
            if is_internal_test_module(&module) {
                continue;
            }
            age_by_module.insert(module.clone(), age);
            crate::set_pending_age_seconds(&module, age);
        }
    }

    if let Ok(modules) = beampipe_db::repo::get_enabled_project_modules(pool).await {
        for module in modules {
            if is_internal_test_module(&module) {
                continue;
            }
            if !pending_by_module.contains_key(&module) {
                crate::set_workflow_pending_sources(&module, 0);
            }
            if !age_by_module.contains_key(&module) {
                crate::set_pending_age_seconds(&module, 0);
            }
        }
    }

    if let Ok(test_modules) = beampipe_db::repo::list_internal_test_project_modules(pool).await {
        for module in test_modules {
            crate::set_workflow_pending_sources(&module, 0);
            crate::set_pending_age_seconds(&module, 0);
        }
    }

    refresh_source_processing_gauges(pool).await;
}

fn zero_missing_job_kind_gauges(current: &HashSet<String>) {
    let mut last = LAST_JOB_KIND_KEYS
        .lock()
        .expect("job kind metric keys lock");
    for kind in last.difference(current) {
        crate::set_jobs_queued_by_kind(kind, 0);
        crate::set_oldest_queued_job_age_seconds(kind, 0);
    }
    *last = current.clone();
}

fn zero_missing_execution_status_gauges(current: &HashSet<String>) {
    let mut last = LAST_EXECUTION_STATUS_KEYS
        .lock()
        .expect("execution status metric keys lock");
    for status in last.difference(current) {
        crate::set_executions_active_by_status(status, 0);
        crate::set_oldest_active_execution_age_seconds(status, 0);
    }
    *last = current.clone();
}

async fn refresh_source_processing_gauges(pool: &PgPool) {
    let Ok(rows) = beampipe_db::repo::list_sources_currently_processing(pool).await else {
        return;
    };

    let counts = aggregate_source_processing(rows);
    let current = counts.keys().cloned().collect::<HashSet<_>>();
    for ((module, phase), count) in counts {
        crate::set_sources_processing(&module, &phase, count);
    }

    let mut last = LAST_SOURCE_PROCESSING_KEYS
        .lock()
        .expect("source processing metric keys lock");
    for (module, phase) in last.difference(&current) {
        crate::set_sources_processing(module, phase, 0);
    }
    *last = current;
}

fn aggregate_source_processing(
    rows: Vec<(String, String, String)>,
) -> HashMap<(String, String), i64> {
    let mut best: HashMap<(String, String), String> = HashMap::new();
    for (module, source, phase) in rows {
        if is_internal_test_module(&module) {
            continue;
        }
        let key = (module.clone(), source.clone());
        best.entry(key)
            .and_modify(|existing| {
                if phase_priority(&phase) > phase_priority(existing) {
                    *existing = phase.clone();
                }
            })
            .or_insert(phase);
    }
    let mut counts = HashMap::new();
    for ((module, _source), phase) in best {
        *counts.entry((module, phase)).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn internal_test_modules_filtered() {
        assert!(is_internal_test_module(
            "fail_requeue_019e80f2-cd6f-7883-93e1-62b9ccd35cdf"
        ));
        assert!(is_internal_test_module(
            "sig_test_019e80f3-1089-78b3-94d7-e22800f7a751"
        ));
        assert!(is_internal_test_module("exec_sig_abc"));
        assert!(!is_internal_test_module("wallaby_hires"));
    }

    #[test]
    fn source_processing_is_aggregated_without_source_labels() {
        let counts = aggregate_source_processing(vec![
            ("wallaby".into(), "a".into(), "discovering".into()),
            ("wallaby".into(), "a".into(), "executing".into()),
            ("wallaby".into(), "b".into(), "executing".into()),
            ("wallaby".into(), "c".into(), "admitting".into()),
            ("exec_sig_test".into(), "hidden".into(), "executing".into()),
        ]);
        assert_eq!(
            counts.get(&("wallaby".into(), "executing".into())),
            Some(&2)
        );
        assert_eq!(
            counts.get(&("wallaby".into(), "admitting".into())),
            Some(&1)
        );
        assert_eq!(counts.len(), 2);
    }

    fn profile(
        translation: serde_json::Value,
        deployment: serde_json::Value,
    ) -> DeploymentProfileRow {
        DeploymentProfileRow {
            uuid: Uuid::now_v7(),
            name: "qualified".into(),
            description: None,
            project_module: Some("wallaby".into()),
            is_default: true,
            max_concurrent_executions: None,
            translation,
            deployment,
            revision: 1,
            spec_sha256: None,
            created_at: Utc::now(),
            updated_at: None,
        }
    }

    #[test]
    fn rest_profile_probes_its_own_tm_and_dim() {
        let row = profile(
            json!({"tm_url": "http://profile-tm:9000"}),
            json!({
                "kind": "rest_remote",
                "deploy_host": "profile-dim",
                "deploy_port": 8001
            }),
        );
        let probes = profile_probes(
            &row,
            Some("http://fallback-tm:9000"),
            Some("http://fallback-dim:8001"),
        )
        .unwrap();
        assert!(matches!(
            &probes[0],
            ProfileProbe::Http { dependency: "tm", url } if url == "http://profile-tm:9000"
        ));
        assert!(matches!(
            &probes[1],
            ProfileProbe::Http { dependency: "dim", url } if url.contains("profile-dim:8001")
        ));
    }

    #[test]
    fn slurm_profile_creates_a_real_ssh_probe() {
        let row = profile(
            json!({"tm_url": "http://profile-tm:9000"}),
            json!({
                "kind": "slurm_remote",
                "login_node": "login.example.org",
                "ssh_credential": "setonix",
                "account": "project",
                "home_dir": "/scratch/project",
                "log_dir": "/scratch/project/log",
                "dlg_root": "/scratch/project/dlg"
            }),
        );
        let probes = profile_probes(&row, None, None).unwrap();
        assert_eq!(probes.len(), 2);
        assert!(
            matches!(&probes[1], ProfileProbe::Slurm(slurm) if slurm.login_node == "login.example.org")
        );
    }
}

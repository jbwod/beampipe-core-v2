use beampipe_adapters::probe_tap_health;
use beampipe_config::Settings;
use beampipe_db::{models::DeploymentProfileRow, repo};
use beampipe_orchestration::{
    collect_security_issues, DaliugeManager, DaliugeTranslator, HttpDimClient,
    HttpTranslatorClient, SchedulerAdapter, SlurmSshCredentials, SlurmSshSession, SlurmTarget,
    SshSlurmClient,
};
use beampipe_profiles::{DeploymentConfig, DeploymentProfile};
use chrono::Utc;
use serde::Serialize;
use sqlx::PgPool;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub code: String,
    pub component: String,
    pub ok: bool,
    pub required: bool,
    pub severity: DoctorSeverity,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub generated_at: chrono::DateTime<Utc>,
    pub profile: Option<String>,
    pub checks: Vec<DoctorCheck>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes_applied: Vec<String>,
}

impl DoctorReport {
    pub fn database_unreachable(error: &str, fixes_applied: Vec<String>) -> Self {
        Self {
            ok: false,
            generated_at: Utc::now(),
            profile: None,
            checks: vec![failure(
                "postgres.unreachable",
                "postgres",
                true,
                bounded(error),
                "verify DATABASE_URL, PostgreSQL service state, and network access",
            )],
            fixes_applied,
        }
    }
}

fn success(code: &str, component: &str, detail: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        code: code.into(),
        component: component.into(),
        ok: true,
        required: true,
        severity: DoctorSeverity::Info,
        detail: detail.into(),
        hint: None,
    }
}

fn warning(
    code: &str,
    component: &str,
    detail: impl Into<String>,
    hint: impl Into<String>,
) -> DoctorCheck {
    DoctorCheck {
        code: code.into(),
        component: component.into(),
        ok: false,
        required: false,
        severity: DoctorSeverity::Warning,
        detail: detail.into(),
        hint: Some(hint.into()),
    }
}

fn failure(
    code: &str,
    component: &str,
    required: bool,
    detail: impl Into<String>,
    hint: impl Into<String>,
) -> DoctorCheck {
    DoctorCheck {
        code: code.into(),
        component: component.into(),
        ok: false,
        required,
        severity: if required {
            DoctorSeverity::Error
        } else {
            DoctorSeverity::Warning
        },
        detail: detail.into(),
        hint: Some(hint.into()),
    }
}

pub async fn run_doctor(
    pool: &PgPool,
    settings: &Settings,
    profile_name: Option<&str>,
    fixes_applied: Vec<String>,
) -> DoctorReport {
    let mut checks = Vec::new();
    checks.push(success(
        "config.loaded",
        "configuration",
        "layered application configuration parsed successfully",
    ));

    let db_ok = sqlx::query("SELECT 1").execute(pool).await.is_ok();
    checks.push(if db_ok {
        success("postgres.connected", "postgres", "connected")
    } else {
        failure(
            "postgres.unreachable",
            "postgres",
            true,
            "database query failed",
            "verify DATABASE_URL and PostgreSQL service state",
        )
    });

    let applied_migrations: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM _sqlx_migrations WHERE success = true")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    checks.push(if applied_migrations > 0 {
        success(
            "postgres.migrations",
            "postgres",
            format!("{applied_migrations} successful migrations recorded"),
        )
    } else {
        failure(
            "postgres.migrations_missing",
            "postgres",
            true,
            "no successful SQLx migration was found",
            "run `beampipe migrate`",
        )
    });

    let jwt_ok =
        settings.jwt_secret.len() >= 32 && !settings.jwt_secret.eq_ignore_ascii_case("change-me");
    checks.push(if jwt_ok {
        success("auth.jwt_secret", "authentication", "secret configured")
    } else {
        failure(
            "auth.weak_jwt_secret",
            "authentication",
            true,
            "JWT secret is missing, short, or a known placeholder",
            "set BEAMPIPE_JWT_SECRET to at least 32 random characters",
        )
    });

    let security_issues = collect_security_issues(settings);
    checks.push(if security_issues.is_empty() {
        success(
            "security.policy",
            "security",
            "security policy checks passed",
        )
    } else {
        failure(
            "security.policy_failed",
            "security",
            settings.beampipe_env.eq_ignore_ascii_case("production"),
            security_issues.join("; "),
            "correct the reported secret, TLS, and SSH verification settings",
        )
    });

    check_redis(settings, &mut checks).await;
    check_tap(settings, &mut checks).await;
    check_queue_and_workers(pool, settings, &mut checks).await;
    check_projects(pool, settings, &mut checks).await;

    let profile = resolve_profile(pool, profile_name).await;
    match profile {
        Ok(profile) => {
            if let Some(profile) = profile.as_ref() {
                check_profile(settings, profile, &mut checks).await;
            } else if let Some(profile_name) = profile_name {
                checks.push(failure(
                    "profile.not_found",
                    "deployment_profile",
                    true,
                    format!("deployment profile '{profile_name}' was not found"),
                    "list profiles with `beampipe profile list`",
                ));
            } else {
                check_configured_daliuge(settings, &mut checks).await;
            }
        }
        Err(error) => checks.push(failure(
            "profile.lookup_failed",
            "deployment_profile",
            true,
            bounded(&error.to_string()),
            "verify the database and deployment profile records",
        )),
    }

    let ok = checks.iter().all(|check| check.ok || !check.required);
    DoctorReport {
        ok,
        generated_at: Utc::now(),
        profile: profile_name.map(str::to_string),
        checks,
        fixes_applied,
    }
}

async fn check_redis(settings: &Settings, checks: &mut Vec<DoctorCheck>) {
    let Some(url) = settings.redis_url.as_deref().filter(|url| !url.is_empty()) else {
        checks.push(warning(
            "redis.not_configured",
            "redis",
            "Redis rate limiter is not configured",
            "this is acceptable for a single local API; configure Redis for replicated APIs",
        ));
        return;
    };
    let ok = match redis::Client::open(url) {
        Ok(client) => match client.get_multiplexed_async_connection().await {
            Ok(mut connection) => redis::cmd("PING")
                .query_async::<String>(&mut connection)
                .await
                .is_ok_and(|response| response == "PONG"),
            Err(_) => false,
        },
        Err(_) => false,
    };
    checks.push(if ok {
        success("redis.connected", "redis", "PING returned PONG")
    } else {
        failure(
            "redis.unreachable",
            "redis",
            settings.require_rate_limiter,
            "configured Redis endpoint is unreachable",
            "verify REDIS_URL and network access",
        )
    });
}

async fn check_tap(settings: &Settings, checks: &mut Vec<DoctorCheck>) {
    let timeout = Duration::from_secs(settings.discovery_tap_health_timeout_seconds);
    let tap = probe_tap_health(
        settings.casda_tap_url.as_deref(),
        settings.vizier_tap_url.as_deref(),
        timeout,
    )
    .await;
    for (name, endpoint) in [("casda", tap.casda), ("vizier", tap.vizier)] {
        if !endpoint.configured {
            checks.push(warning(
                &format!("tap.{name}_not_configured"),
                "archive_adapter",
                format!("{name} TAP endpoint is not configured"),
                "configure the endpoint when this archive adapter is required",
            ));
        } else if endpoint.reachable {
            checks.push(success(
                &format!("tap.{name}_reachable"),
                "archive_adapter",
                format!("{name} TAP endpoint is reachable"),
            ));
        } else {
            checks.push(failure(
                &format!("tap.{name}_unreachable"),
                "archive_adapter",
                settings.use_real_backends,
                format!("{name} TAP health probe failed"),
                "verify the TAP URL, credentials, VPN, and network path",
            ));
        }
    }
}

async fn check_queue_and_workers(
    pool: &PgPool,
    settings: &Settings,
    checks: &mut Vec<DoctorCheck>,
) {
    let queue_depth = repo::queue_depth(pool).await.unwrap_or(-1);
    let jobs_running = repo::jobs_running_count(pool).await.unwrap_or(-1);
    checks.push(if queue_depth >= 0 {
        success(
            "queue.readable",
            "job_queue",
            format!("depth={queue_depth} running={jobs_running}"),
        )
    } else {
        failure(
            "queue.unreadable",
            "job_queue",
            true,
            "job queue counts could not be read",
            "inspect PostgreSQL logs and run migrations",
        )
    });

    let workers = repo::list_worker_instances(pool, false)
        .await
        .unwrap_or_default();
    let stale_after = (settings.worker_heartbeat_interval_seconds as i64 * 3).max(30);
    let stale = workers
        .iter()
        .filter(|worker| {
            Utc::now()
                .signed_duration_since(worker.last_heartbeat_at)
                .num_seconds()
                > stale_after
        })
        .count();
    let future = workers
        .iter()
        .filter(|worker| {
            worker
                .last_heartbeat_at
                .signed_duration_since(Utc::now())
                .num_seconds()
                > 5
        })
        .count();
    checks.push(if workers.is_empty() {
        warning(
            "workers.none",
            "beampipe_worker",
            "no active Beampipe control-plane workers are registered",
            "start `beampipe worker` before submitting asynchronous work",
        )
    } else if stale > 0 {
        failure(
            "workers.stale_heartbeat",
            "beampipe_worker",
            true,
            format!("{stale} of {} workers have stale heartbeats", workers.len()),
            "inspect `beampipe worker list`; restart or drain stale instances",
        )
    } else {
        success(
            "workers.healthy",
            "beampipe_worker",
            format!("{} workers have current heartbeats", workers.len()),
        )
    });
    if future > 0 {
        checks.push(failure(
            "workers.clock_skew",
            "beampipe_worker",
            true,
            format!("{future} workers reported heartbeats more than five seconds in the future"),
            "synchronise PostgreSQL, API, and worker hosts with NTP",
        ));
    }

    let expired = repo::list_worker_leases(pool, None, true)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|job| {
            job.lease_expires_at
                .is_some_and(|expires| expires <= Utc::now())
        })
        .count();
    checks.push(if expired == 0 {
        success(
            "leases.current",
            "job_queue",
            "no expired running leases remain",
        )
    } else {
        failure(
            "leases.expired_running",
            "job_queue",
            true,
            format!("{expired} running jobs have expired leases"),
            "ensure an eligible worker is active so it can recover expired claims",
        )
    });
}

async fn check_projects(pool: &PgPool, settings: &Settings, checks: &mut Vec<DoctorCheck>) {
    let rows = repo::list_active_project_configs(pool)
        .await
        .unwrap_or_default();
    if rows.is_empty() {
        checks.push(warning(
            "projects.none",
            "project_config",
            "no active project configuration is installed",
            "run `beampipe project add -f config/wallaby_hires.v2.yaml`",
        ));
        return;
    }
    for row in rows {
        let config = match serde_json::from_value::<beampipe_project::ProjectConfig>(row.spec) {
            Ok(config) => config,
            Err(error) => {
                checks.push(failure(
                    "project.deserialize_failed",
                    "project_config",
                    true,
                    format!("{}: {error}", row.project_id),
                    "upload a valid beampipe.dev/v2 project configuration",
                ));
                continue;
            }
        };
        let report = config.validate_report();
        checks.push(if report.valid {
            success(
                "project.valid",
                "project_config",
                format!("{} v{} is valid", row.project_id, row.version),
            )
        } else {
            failure(
                "project.invalid",
                "project_config",
                true,
                format!(
                    "{}: {} validation errors",
                    row.project_id,
                    report.errors.len()
                ),
                "run `beampipe project validate -f <file>` and upload a corrected version",
            )
        });
        if settings.use_real_backends && report.valid {
            checks.push(match beampipe_orchestration::resolve_graph(&config).await {
                Ok(_) => success(
                    "project.graph_accessible",
                    "daliuge_graph",
                    format!("{} source graph loaded", row.project_id),
                ),
                Err(error) => failure(
                    "project.graph_unavailable",
                    "daliuge_graph",
                    true,
                    format!("{}: {error}", row.project_id),
                    "verify graph.url/path and network or filesystem permissions",
                ),
            });
        }
    }
}

async fn resolve_profile(
    pool: &PgPool,
    profile_name: Option<&str>,
) -> Result<Option<DeploymentProfileRow>, sqlx::Error> {
    if let Some(name) = profile_name {
        return repo::get_deployment_profile_by_name(pool, name).await;
    }
    let profiles = repo::list_deployment_profiles(pool, None, 500, 0).await?;
    Ok(profiles
        .iter()
        .find(|profile| profile.is_default && profile.project_module.is_none())
        .cloned())
}

fn typed_profile(row: &DeploymentProfileRow) -> Result<DeploymentProfile, serde_json::Error> {
    Ok(DeploymentProfile {
        name: row.name.clone(),
        description: row.description.clone(),
        project_module: row.project_module.clone(),
        is_default: row.is_default,
        max_concurrent_executions: row.max_concurrent_executions,
        translation: serde_json::from_value(row.translation.clone())?,
        deployment: serde_json::from_value(row.deployment.clone())?,
    })
}

async fn check_profile(
    settings: &Settings,
    row: &DeploymentProfileRow,
    checks: &mut Vec<DoctorCheck>,
) {
    let profile = match typed_profile(row) {
        Ok(profile) => profile,
        Err(error) => {
            checks.push(failure(
                "profile.deserialize_failed",
                "deployment_profile",
                true,
                bounded(&error.to_string()),
                "replace the profile with a valid typed deployment profile",
            ));
            return;
        }
    };
    checks.push(match profile.validate() {
        Ok(()) => success(
            "profile.valid",
            "deployment_profile",
            format!("{} revision {} is valid", row.name, row.revision),
        ),
        Err(error) => failure(
            "profile.invalid",
            "deployment_profile",
            true,
            error.to_string(),
            "correct the profile and run `beampipe profile validate`",
        ),
    });

    check_translator(
        profile
            .translation
            .tm_url
            .as_deref()
            .or(settings.tm_url.as_deref()),
        &mut *checks,
        settings.use_real_backends,
    )
    .await;
    match profile.deployment {
        DeploymentConfig::RestRemote(rest) => {
            let manager_url = beampipe_orchestration::cancel::rest_endpoint(&rest)
                .or_else(|| settings.dim_url.clone());
            check_manager(manager_url.as_deref(), checks, settings.use_real_backends).await;
        }
        DeploymentConfig::SlurmRemote(slurm) => {
            let credential_ok = SlurmSshCredentials::try_resolve_ok();
            checks.push(if credential_ok {
                success(
                    "slurm.credentials",
                    "slurm_ssh",
                    "SSH credentials and host verification policy resolve",
                )
            } else {
                failure(
                    "slurm.credentials_missing",
                    "slurm_ssh",
                    true,
                    "SSH credentials could not be resolved",
                    "configure a private key and verified known_hosts file",
                )
            });
            if !credential_ok {
                return;
            }
            let client = SshSlurmClient {
                login_node: slurm.login_node.clone(),
                remote_user: slurm.remote_user.clone(),
                session_dir: slurm.log_dir.clone(),
                account: Some(slurm.account.clone()),
                ssh_port: slurm.ssh_port,
                dlg_root: slurm.dlg_root.clone(),
                deployment: Some(slurm.clone()),
            };
            match client.test_connectivity().await {
                Ok(info) => checks.push(success(
                    "slurm.connected",
                    "slurm",
                    format!(
                        "{}; version={}",
                        info.target,
                        info.scheduler_version.as_deref().unwrap_or("unreported")
                    ),
                )),
                Err(error) => checks.push(failure(
                    "slurm.connectivity_failed",
                    "slurm",
                    true,
                    bounded(&error.to_string()),
                    "verify VPN, SSH identity, known_hosts, and SLURM command access",
                )),
            }
            check_remote_directory(&slurm, checks).await;
        }
    }
}

async fn check_configured_daliuge(settings: &Settings, checks: &mut Vec<DoctorCheck>) {
    check_translator(
        settings.tm_url.as_deref(),
        checks,
        settings.use_real_backends,
    )
    .await;
    check_manager(
        settings.dim_url.as_deref(),
        checks,
        settings.use_real_backends,
    )
    .await;
}

async fn check_translator(url: Option<&str>, checks: &mut Vec<DoctorCheck>, required: bool) {
    let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
        checks.push(failure(
            "daliuge.translator_not_configured",
            "daliuge_translator",
            required,
            "Translator Manager URL is not configured",
            "set integrations.tm_url or translation.tm_url in the selected profile",
        ));
        return;
    };
    match HttpTranslatorClient::new(url).inspect(None, None).await {
        Ok(info) if info.capabilities.updated_translation_api => checks.push(success(
            "daliuge.translator_compatible",
            "daliuge_translator",
            format!("{} exposes the updated translation API", info.endpoint),
        )),
        Ok(_) => checks.push(failure(
            "daliuge.translator_incompatible",
            "daliuge_translator",
            required,
            "Translator Manager did not report the updated translation API",
            "deploy a DALiuGE version exposing /unroll_and_partition, /map, and /api/submission_method",
        )),
        Err(error) => checks.push(failure(
            "daliuge.translator_unreachable",
            "daliuge_translator",
            required,
            bounded(&error.to_string()),
            "verify the Translator Manager URL and network path",
        )),
    }
}

async fn check_manager(url: Option<&str>, checks: &mut Vec<DoctorCheck>, required: bool) {
    let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
        checks.push(failure(
            "daliuge.manager_not_configured",
            "daliuge_dim",
            required,
            "Data Island Manager URL is not configured",
            "set integrations.dim_url or a REST deployment endpoint",
        ));
        return;
    };
    match HttpDimClient::new(url).inspect().await {
        Ok(info) => checks.push(success(
            "daliuge.manager_reachable",
            "daliuge_dim",
            format!(
                "{} hosts={} nodes={} sessions={}",
                info.endpoint,
                info.hosts.len(),
                info.nodes.len(),
                info.sessions.len()
            ),
        )),
        Err(error) => checks.push(failure(
            "daliuge.manager_unreachable",
            "daliuge_dim",
            required,
            bounded(&error.to_string()),
            "verify the Data Island Manager URL, TLS policy, and network path",
        )),
    }
}

async fn check_remote_directory(
    slurm: &beampipe_profiles::SlurmRemoteDeploymentConfig,
    checks: &mut Vec<DoctorCheck>,
) {
    let username = beampipe_orchestration::slurm_deploy::resolve_remote_user(slurm);
    let target = SlurmTarget::from_deployment(slurm, &username);
    let mut session = match SlurmSshSession::connect(&target).await {
        Ok(session) => session,
        Err(error) => {
            checks.push(failure(
                "slurm.remote_directory_unchecked",
                "slurm_ssh",
                true,
                bounded(&error.to_string()),
                "restore SSH connectivity before checking remote paths",
            ));
            return;
        }
    };
    let path = shell_quote(&slurm.dlg_root);
    let result = session
        .run_command(&format!(
            "test -d {path} && test -w {path} && echo writable"
        ))
        .await;
    let _ = session.close().await;
    checks.push(match result {
        Ok(output) if output.trim() == "writable" => success(
            "slurm.remote_directory_writable",
            "slurm_ssh",
            format!("{} is writable", slurm.dlg_root),
        ),
        Ok(_) => failure(
            "slurm.remote_directory_unwritable",
            "slurm_ssh",
            true,
            format!("{} did not pass writable-directory checks", slurm.dlg_root),
            "create the directory and grant the configured remote user write access",
        ),
        Err(error) => failure(
            "slurm.remote_directory_unwritable",
            "slurm_ssh",
            true,
            bounded(&error.to_string()),
            "create the configured dlg_root and grant the remote user write access",
        ),
    });
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn bounded(value: &str) -> String {
    value.chars().take(2048).collect()
}

pub fn print_human(report: &DoctorReport) {
    println!(
        "Beampipe doctor{}",
        report
            .profile
            .as_deref()
            .map(|profile| format!(" [profile: {profile}]"))
            .unwrap_or_default()
    );
    for item in &report.checks {
        let marker = if item.ok {
            "OK"
        } else if item.required {
            "FAIL"
        } else {
            "WARN"
        };
        println!("[{marker:4}] {:22} {}", item.component, item.detail);
        if let Some(hint) = &item.hint {
            println!("       fix: {hint}");
        }
    }
    if !report.fixes_applied.is_empty() {
        println!("Applied safe fixes: {}", report.fixes_applied.join(", "));
    }
    println!(
        "Result: {}",
        if report.ok {
            "ready"
        } else {
            "attention required"
        }
    );
}

pub async fn run_status(pool: &PgPool) -> serde_json::Value {
    let queue_depth = repo::queue_depth(pool).await.unwrap_or(0);
    let jobs_running = repo::jobs_running_count(pool).await.unwrap_or(0);
    let pending = repo::workflow_pending_counts_by_module(pool)
        .await
        .unwrap_or_default();
    let workers = repo::list_worker_pools(pool).await.unwrap_or_default();
    serde_json::json!({
        "queue_depth": queue_depth,
        "jobs_running": jobs_running,
        "workflow_pending_by_module": pending,
        "worker_pools": workers,
    })
}

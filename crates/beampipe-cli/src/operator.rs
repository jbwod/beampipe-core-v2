use anyhow::{bail, Context, Result};
use beampipe_config::{Settings, SettingsResolution};
use beampipe_db::{models::DeploymentProfileRow, repo};
use beampipe_domain::{DaliugeState, ExecutionStatus, LedgerPatch, SchedulerState};
use beampipe_orchestration::{
    cancel::CancelParams, cancel_scheduler_session, DaliugeManager, DaliugeTranslator,
    HttpDimClient, HttpTranslatorClient, SchedulerAdapter, SshSlurmClient, TranslatorClient,
};
use beampipe_profiles::{DeploymentConfig, DeploymentProfile};
use beampipe_project::ProjectConfig;
use serde_json::json;
use sqlx::PgPool;
use std::path::Path;
use uuid::Uuid;

use crate::{
    DaliugeCommand, ExecutionCommand, GraphCommand, ProfileCommand, SchedulerCommand, WorkerCommand,
};

fn load() -> Result<SettingsResolution> {
    Ok(Settings::load()?)
}

async fn connect() -> Result<(Settings, PgPool)> {
    let settings = load()?.settings;
    let pool = beampipe_db::connect(&settings.database_url).await?;
    Ok((settings, pool))
}

pub fn explain_config() -> Result<()> {
    let resolution = load()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "config_path": resolution.config_path,
            "settings": resolution.explain(),
        }))?
    );
    Ok(())
}

pub fn explain_project(path: &Path) -> Result<()> {
    let config = read_project(path)?;
    let report = config.validate_report();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "project_id": config.metadata.id,
            "api_version": config.api_version,
            "valid": report.valid,
            "spec_sha256": report.spec_sha256,
            "required_adapters": config.adapters.required,
            "transform_count": config.definitions.as_ref().map(|d| d.transforms.len()).unwrap_or(0),
            "graph_patch_count": config.graph_patches.len(),
            "has_manifest_template": config.manifest.is_some(),
            "automation": config.automation,
            "diagnostics": report.errors,
            "warnings": report.warnings,
        }))?
    );
    if !report.valid {
        bail!("project configuration is invalid");
    }
    Ok(())
}

pub fn render_project(path: &Path) -> Result<()> {
    let config = read_project(path)?;
    let report = config.validate_report();
    if !report.valid {
        bail!("project configuration is invalid: {:?}", report.errors);
    }
    print!("{}", serde_yaml::to_string(&config)?);
    Ok(())
}

fn read_project(path: &Path) -> Result<ProjectConfig> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(ProjectConfig::from_slice(&bytes)?)
}

pub async fn run_worker_command(command: WorkerCommand) -> Result<()> {
    let (_, pool) = connect().await?;
    match command {
        WorkerCommand::List { include_stopped } => {
            let workers = repo::list_worker_instances(&pool, include_stopped).await?;
            let mut values = Vec::with_capacity(workers.len());
            for worker in workers {
                let leases = repo::active_worker_lease_count(&pool, worker.uuid).await?;
                values.push(json!({
                    "worker": worker,
                    "active_leases": leases,
                }));
            }
            print_json(&values)?;
        }
        WorkerCommand::Inspect { id } => {
            let worker = repo::get_worker_instance(&pool, id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("worker not found: {id}"))?;
            let leases = repo::list_worker_leases(&pool, Some(id), true).await?;
            print_json(&json!({"worker": worker, "leases": leases}))?;
        }
        WorkerCommand::Drain { id } => mutate_worker(&pool, id, true).await?,
        WorkerCommand::Resume { id } => mutate_worker(&pool, id, false).await?,
        WorkerCommand::Pools => print_json(&repo::list_worker_pools(&pool).await?)?,
        WorkerCommand::Leases {
            worker,
            include_expired,
        } => print_json(&repo::list_worker_leases(&pool, worker, include_expired).await?)?,
    }
    Ok(())
}

async fn mutate_worker(pool: &PgPool, id: Uuid, draining: bool) -> Result<()> {
    let worker = repo::set_worker_draining(pool, id, draining)
        .await?
        .ok_or_else(|| anyhow::anyhow!("worker not found or stopped: {id}"))?;
    let event = if draining {
        "worker.drained"
    } else {
        "worker.resumed"
    };
    let correlation = id.to_string();
    repo::insert_provenance_event(
        pool,
        event,
        "system",
        None,
        None,
        Some("operator:cli"),
        Some(&correlation),
        &json!({"worker_id": id, "draining": draining}),
    )
    .await?;
    print_json(&worker)
}

pub async fn run_profile_command(command: ProfileCommand) -> Result<()> {
    let (_, pool) = connect().await?;
    match command {
        ProfileCommand::Add { file } => {
            let profile = read_profile(&file)?;
            profile.validate()?;
            let row = repo::create_deployment_profile(
                &pool,
                &profile.name,
                profile.description.as_deref(),
                profile.project_module.as_deref(),
                profile.is_default,
                profile.max_concurrent_executions,
                serde_json::to_value(&profile.translation)?,
                serde_json::to_value(&profile.deployment)?,
            )
            .await?;
            print_json(&row)?;
        }
        ProfileCommand::List => {
            print_json(&repo::list_deployment_profiles(&pool, None, 500, 0).await?)?;
        }
        ProfileCommand::Validate { profile } => {
            let row = profile_by_name(&pool, &profile).await?;
            let parsed = profile_from_row(&row)?;
            parsed.validate()?;
            print_json(&json!({
                "valid": true,
                "profile": parsed.name,
                "revision": row.revision,
                "spec_sha256": row.spec_sha256,
            }))?;
        }
        ProfileCommand::Test { profile } => {
            let row = profile_by_name(&pool, &profile).await?;
            test_profile(&row).await?;
        }
        ProfileCommand::Render { profile } => {
            let row = profile_by_name(&pool, &profile).await?;
            let parsed = profile_from_row(&row)?;
            parsed.validate()?;
            let rendered = match &parsed.deployment {
                DeploymentConfig::SlurmRemote(slurm) => {
                    let resources =
                        beampipe_orchestration::SchedulerResourceRequest::from_slurm_profile(slurm);
                    json!({
                        "profile": parsed,
                        "resource_request": resources,
                        "sbatch_directives": resources.render_sbatch_directives(),
                    })
                }
                DeploymentConfig::RestRemote(rest) => json!({
                    "profile": parsed,
                    "manager_url": beampipe_orchestration::cancel::rest_endpoint(rest),
                }),
            };
            print_json(&rendered)?;
        }
    }
    Ok(())
}

fn read_profile(path: &Path) -> Result<DeploymentProfile> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_yaml::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn profile_from_row(row: &DeploymentProfileRow) -> Result<DeploymentProfile> {
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

async fn profile_by_name(pool: &PgPool, name: &str) -> Result<DeploymentProfileRow> {
    repo::get_deployment_profile_by_name(pool, name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("deployment profile not found: {name}"))
}

fn scheduler_client(row: &DeploymentProfileRow) -> Result<SshSlurmClient> {
    let profile = profile_from_row(row)?;
    let DeploymentConfig::SlurmRemote(slurm) = profile.deployment else {
        bail!("profile '{}' is not slurm_remote", row.name);
    };
    Ok(SshSlurmClient {
        login_node: slurm.login_node.clone(),
        remote_user: slurm.remote_user.clone(),
        session_dir: slurm.log_dir.clone(),
        account: Some(slurm.account.clone()),
        ssh_port: slurm.ssh_port,
        dlg_root: slurm.dlg_root.clone(),
        deployment: Some(slurm),
    })
}

async fn test_profile(row: &DeploymentProfileRow) -> Result<()> {
    let parsed = profile_from_row(row)?;
    parsed.validate()?;
    match &parsed.deployment {
        DeploymentConfig::SlurmRemote(_) => {
            let client = scheduler_client(row)?;
            print_json(&json!({
                "profile": row.name,
                "connectivity": client.test_connectivity().await?,
                "resource_request": client.resource_request()?,
            }))?;
        }
        DeploymentConfig::RestRemote(_) => {
            let settings = load()?.settings;
            let endpoints = daliuge_endpoints(&settings, Some(row))?;
            let translator = HttpTranslatorClient::new(&endpoints.tm_url)
                .inspect(endpoints.manager_host.as_deref(), endpoints.manager_port)
                .await?;
            let manager = match endpoints.manager_url {
                Some(url) => Some(HttpDimClient::new(url).inspect().await?),
                None => None,
            };
            print_json(&json!({
                "profile": row.name,
                "translator": translator,
                "manager": manager,
            }))?;
        }
    }
    Ok(())
}

pub async fn run_scheduler_command(command: SchedulerCommand) -> Result<()> {
    let (_, pool) = connect().await?;
    match command {
        SchedulerCommand::Status { profile } => {
            let row = profile_by_name(&pool, &profile).await?;
            let client = scheduler_client(&row)?;
            let resources = client.resource_request()?;
            print_json(&json!({
                "profile": row.name,
                "connectivity": client.test_connectivity().await?,
                "queue": client.queue().await?,
                "capacity": client.capacity().await?,
                "resource_request": resources,
                "rendered": resources.render_sbatch_directives(),
            }))?;
        }
        SchedulerCommand::Jobs { limit } => {
            print_json(&repo::list_scheduler_executions(&pool, "slurm", limit, 0).await?)?;
        }
        SchedulerCommand::Cancel { execution } => {
            cancel_execution(&pool, execution).await?;
        }
    }
    Ok(())
}

async fn cancel_execution(pool: &PgPool, id: Uuid) -> Result<()> {
    let execution = repo::get_execution(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("execution not found: {id}"))?;
    let deployment = repo::resolve_execution_deployment(pool, &execution)
        .await?
        .ok_or_else(|| anyhow::anyhow!("execution has no pinned deployment profile"))?;
    let deployment_kind = serde_json::from_value::<DeploymentConfig>(deployment.clone())?;
    let result = cancel_scheduler_session(CancelParams {
        scheduler_job_id: execution.scheduler_job_id.clone(),
        daliuge_session_id: execution.daliuge_session_id.clone(),
        deployment,
    })
    .await?;
    if result.cancelled {
        let state_patch = match deployment_kind {
            DeploymentConfig::RestRemote(_) => beampipe_db::models::ExecutionStatePatch {
                daliuge_state: Some(DaliugeState::Cancelled),
                ..Default::default()
            },
            DeploymentConfig::SlurmRemote(_) => beampipe_db::models::ExecutionStatePatch {
                scheduler_state: Some(SchedulerState::Cancelled),
                ..Default::default()
            },
        };
        repo::apply_execution_state_patch(pool, id, state_patch).await?;
        repo::apply_execution_patch(
            pool,
            id,
            LedgerPatch {
                status: Some(ExecutionStatus::Cancelled),
                ..Default::default()
            },
        )
        .await?;
        repo::insert_provenance_event(
            pool,
            "execution.cancelled",
            &execution.project_module,
            None,
            Some(id),
            Some("operator:cli"),
            Some(&id.to_string()),
            &json!({
                "scheduler_job_id": execution.scheduler_job_id,
                "daliuge_session_id": execution.daliuge_session_id,
            }),
        )
        .await?;
    }
    print_json(&result)
}

pub async fn run_execution_command(command: ExecutionCommand) -> Result<()> {
    let (_, pool) = connect().await?;
    match command {
        ExecutionCommand::Retry { id, reason } => {
            let result =
                repo::retry_execution(&pool, id, "operator:cli", &reason, Some(&id.to_string()))
                    .await?;
            print_json(&json!({
                "status": "accepted",
                "execution_id": result.execution.uuid,
                "job_id": result.job.uuid,
                "retry_count": result.execution.retry_count,
                "stage": result.plan.stage,
                "do_stage": result.plan.do_stage,
                "do_submit": result.plan.do_submit,
            }))?;
        }
        ExecutionCommand::Cancel { id } => cancel_execution(&pool, id).await?,
    }
    Ok(())
}

pub async fn run_graph_command(command: GraphCommand) -> Result<()> {
    let (_, pool) = connect().await?;
    match command {
        GraphCommand::Prepare { project, source } => {
            let preview = beampipe_jobs::prepare_execution_graph(&pool, &project, &source).await?;
            print_json(&preview)?;
        }
        GraphCommand::Diff { execution, full } => {
            let artifacts = repo::list_execution_artifacts(&pool, execution).await?;
            let source = artifacts
                .iter()
                .rev()
                .find(|artifact| artifact.kind == "source_graph")
                .and_then(|artifact| artifact.inline_json.as_ref())
                .ok_or_else(|| anyhow::anyhow!("execution has no inline source_graph artifact"))?;
            let patched = artifacts
                .iter()
                .rev()
                .find(|artifact| artifact.kind == "patched_graph")
                .and_then(|artifact| artifact.inline_json.as_ref())
                .ok_or_else(|| anyhow::anyhow!("execution has no inline patched_graph artifact"))?;
            let execution_row = repo::get_execution(&pool, execution)
                .await?
                .ok_or_else(|| anyhow::anyhow!("execution not found: {execution}"))?;
            let mut output = json!({
                "execution_id": execution,
                "source_graph_sha256": execution_row.source_graph_sha256,
                "patched_graph_sha256": execution_row.patched_graph_sha256,
                "summary": beampipe_jobs::graph_diff_summary(source, patched),
            });
            if full {
                output["source_graph"] = source.clone();
                output["patched_graph"] = patched.clone();
            }
            print_json(&output)?;
        }
    }
    Ok(())
}

pub async fn run_daliuge_command(command: DaliugeCommand) -> Result<()> {
    let (settings, pool) = connect().await?;
    match command {
        DaliugeCommand::Ping { profile } | DaliugeCommand::Inspect { profile } => {
            let row = optional_profile(&pool, profile.as_deref()).await?;
            let endpoints = daliuge_endpoints(&settings, row.as_ref())?;
            let translator = HttpTranslatorClient::new(&endpoints.tm_url)
                .inspect(endpoints.manager_host.as_deref(), endpoints.manager_port)
                .await?;
            let manager = match endpoints.manager_url {
                Some(url) => Some(HttpDimClient::new(url).inspect().await?),
                None => None,
            };
            print_json(&json!({
                "profile": row.map(|row| row.name),
                "translator": translator,
                "manager": manager,
            }))?;
        }
        DaliugeCommand::Sessions { profile } => {
            let client = dim_client(&settings, &pool, profile.as_deref()).await?;
            print_json(&client.sessions().await?)?;
        }
        DaliugeCommand::SessionInspect { id, profile } => {
            let client = dim_client(&settings, &pool, profile.as_deref()).await?;
            print_json(&client.session_observation(&id).await?)?;
        }
        DaliugeCommand::SessionCancel { id, profile } => {
            let client = dim_client(&settings, &pool, profile.as_deref()).await?;
            beampipe_orchestration::DimClient::cancel(&client, &id).await?;
            print_json(&json!({"session_id": id, "cancelled": true}))?;
        }
        DaliugeCommand::Translate { execution } => {
            let row = repo::get_execution(&pool, execution)
                .await?
                .ok_or_else(|| anyhow::anyhow!("execution not found: {execution}"))?;
            let graph = repo::list_execution_artifacts(&pool, execution)
                .await?
                .into_iter()
                .rev()
                .find(|artifact| artifact.kind == "patched_graph")
                .and_then(|artifact| artifact.inline_json)
                .ok_or_else(|| anyhow::anyhow!("execution has no inline patched_graph artifact"))?;
            let translation: beampipe_profiles::DaliugeTranslationConfig = serde_json::from_value(
                repo::resolve_execution_translation(&pool, &row)
                    .await?
                    .ok_or_else(|| {
                        anyhow::anyhow!("execution has no pinned translation profile")
                    })?,
            )?;
            let deployment: DeploymentConfig = serde_json::from_value(
                repo::resolve_execution_deployment(&pool, &row)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("execution has no pinned deployment profile"))?,
            )?;
            let (dim_host, dim_port, slurm_path) = match deployment {
                DeploymentConfig::RestRemote(rest) => (
                    rest.dim_host_for_tm.or(rest.deploy_host),
                    rest.dim_port_for_tm.or(rest.deploy_port),
                    false,
                ),
                DeploymentConfig::SlurmRemote(_) => (None, None, true),
            };
            let tm_url = translation
                .tm_url
                .clone()
                .or(settings.tm_url.clone())
                .ok_or_else(|| anyhow::anyhow!("Translator Manager URL is not configured"))?;
            let translate_config = beampipe_orchestration::translate_config_from_profile(
                &translation,
                dim_host.as_deref(),
                dim_port,
                slurm_path,
            );
            let translated = HttpTranslatorClient::new(tm_url)
                .translate(graph, &translate_config)
                .await?;
            let physical = translated
                .pgt_json
                .clone()
                .unwrap_or_else(|| serde_json::Value::Array(translated.pg_spec.clone()));
            use sha2::{Digest, Sha256};
            let sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&physical)?));
            print_json(&json!({
                "execution_id": execution,
                "dry_run": true,
                "physical_graph_sha256": sha256,
                "roots": translated.roots,
                "physical_graph": physical,
            }))?;
        }
    }
    Ok(())
}

async fn optional_profile(
    pool: &PgPool,
    profile: Option<&str>,
) -> Result<Option<DeploymentProfileRow>> {
    match profile {
        Some(name) => Ok(Some(profile_by_name(pool, name).await?)),
        None => Ok(None),
    }
}

struct DaliugeEndpoints {
    tm_url: String,
    manager_url: Option<String>,
    manager_host: Option<String>,
    manager_port: Option<i32>,
}

fn daliuge_endpoints(
    settings: &Settings,
    row: Option<&DeploymentProfileRow>,
) -> Result<DaliugeEndpoints> {
    let profile = row.map(profile_from_row).transpose()?;
    let tm_url = profile
        .as_ref()
        .and_then(|profile| profile.translation.tm_url.clone())
        .or_else(|| settings.tm_url.clone())
        .ok_or_else(|| anyhow::anyhow!("DALiuGE Translator Manager URL is not configured"))?;
    let rest = profile
        .as_ref()
        .and_then(|profile| match &profile.deployment {
            DeploymentConfig::RestRemote(rest) => Some(rest),
            DeploymentConfig::SlurmRemote(_) => None,
        });
    let manager_url = rest
        .and_then(beampipe_orchestration::cancel::rest_endpoint)
        .or_else(|| settings.dim_url.clone());
    let manager_host = rest.and_then(|rest| {
        rest.dim_host_for_tm
            .clone()
            .or_else(|| rest.deploy_host.clone())
    });
    let manager_port = rest.and_then(|rest| rest.dim_port_for_tm.or(rest.deploy_port));
    Ok(DaliugeEndpoints {
        tm_url,
        manager_url,
        manager_host,
        manager_port,
    })
}

async fn dim_client(
    settings: &Settings,
    pool: &PgPool,
    profile: Option<&str>,
) -> Result<HttpDimClient> {
    let row = optional_profile(pool, profile).await?;
    let endpoint = daliuge_endpoints(settings, row.as_ref())?
        .manager_url
        .ok_or_else(|| anyhow::anyhow!("DALiuGE Data Island Manager URL is not configured"))?;
    Ok(HttpDimClient::new(endpoint))
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

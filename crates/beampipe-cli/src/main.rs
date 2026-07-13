mod bench_tap;
mod console;
mod doctor;
mod init;
mod operator;
mod setup;
mod timeline;

use anyhow::Context;
use beampipe_config::Settings;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "beampipe", version, about = "Beampipe v2 Rust control plane")]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum CliCommand {
    /// Create safe local configuration and production templates.
    Init {
        #[arg(long, default_value = ".")]
        directory: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        production: bool,
    },
    /// Start the API and embedded worker.
    Start {
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        worker: bool,
    },
    /// Run API, optionally with the embedded Postgres job worker.
    Serve {
        /// Run the embedded Postgres job worker in the API process (`false` for API-only).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        worker: bool,
    },
    /// Run worker-only mode or inspect Beampipe control-plane workers.
    Worker {
        #[command(subcommand)]
        command: Option<WorkerCommand>,
    },
    /// Apply SQLx migrations.
    Migrate,
    /// Delete provenance events older than retention window.
    PurgeProvenance {
        /// Retention in days (default from BEAMPIPE_PROVENANCE_RETENTION_DAYS or 90).
        #[arg(long)]
        days: Option<i32>,
    },
    /// Export the v2 OpenAPI document to stdout.
    Openapi {
        #[command(subcommand)]
        command: OpenApiCommand,
    },
    /// Validate or manage project-config YAML/JSON.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Upload WASM survey hook modules.
    Wasm {
        #[command(subcommand)]
        command: WasmCommand,
    },
    /// Interactive first-run setup (use --yes for CI/non-interactive).
    Setup {
        #[command(subcommand)]
        command: Option<SetupCommand>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        database_url: Option<String>,
        #[arg(long)]
        jwt_secret: Option<String>,
        #[arg(long)]
        admin_user: Option<String>,
        #[arg(long)]
        admin_password: Option<String>,
        #[arg(long)]
        admin_email: Option<String>,
        #[arg(long)]
        project_config: Option<PathBuf>,
        #[arg(long)]
        casda_tap_url: Option<String>,
        #[arg(long)]
        tm_url: Option<String>,
        #[arg(long)]
        dim_url: Option<String>,
        #[arg(long)]
        worker_pool: Option<String>,
        #[arg(long, value_parser = ["rest_remote", "slurm_remote"])]
        deployment: Option<String>,
        #[arg(long)]
        profile_name: Option<String>,
        #[arg(long)]
        facility: Option<String>,
        #[arg(long)]
        ssh_host: Option<String>,
        #[arg(long)]
        ssh_user: Option<String>,
        #[arg(long)]
        slurm_account: Option<String>,
        #[arg(long)]
        slurm_partition: Option<String>,
        #[arg(long)]
        dlg_root: Option<String>,
        #[arg(long)]
        remote_home: Option<String>,
        #[arg(long)]
        remote_logs: Option<String>,
        #[arg(long)]
        use_real_backends: bool,
        #[arg(long)]
        skip_admin: bool,
        #[arg(long)]
        skip_upload: bool,
    },
    /// Health and configuration preflight checks.
    Doctor {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        fix: bool,
    },
    /// Explain layered application configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Validate and operate deployment profiles.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Inspect scheduler connectivity and persisted jobs.
    Scheduler {
        #[command(subcommand)]
        command: SchedulerCommand,
    },
    /// Inspect DALiuGE translator, managers, and sessions.
    Daliuge {
        #[command(subcommand)]
        command: DaliugeCommand,
    },
    /// Inspect, retry, or cancel durable executions.
    Execution {
        #[command(subcommand)]
        command: ExecutionCommand,
    },
    /// Prepare and compare immutable DALiuGE graph artifacts.
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },
    /// Queue and backlog summary.
    Status,
    /// Open the live terminal operator console.
    Console {
        #[arg(long, default_value_t = 2_000)]
        refresh_ms: u64,
    },
    /// Operator timelines for executions, sources, and projects.
    Timeline {
        #[command(subcommand)]
        command: TimelineCommand,
    },
    /// Administrative user operations.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Slurm SSH smoke tests.
    Slurm {
        #[command(subcommand)]
        command: SlurmCommand,
    },
    /// Validate JWT, Slurm SSH, CASDA, and database security settings.
    Security {
        #[command(subcommand)]
        command: SecurityCommand,
    },
    /// Export data migration guidance (Python → Rust Postgres).
    MigrateData,
    /// Measure TAP and discovery latency (requires network).
    Bench {
        #[command(subcommand)]
        command: BenchCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SecurityCommand {
    /// Print pass/fail for security configuration (no live SSH).
    Check,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Show each resolved setting, its source, and redacted value.
    Explain,
}

#[derive(Debug, Subcommand)]
enum WorkerCommand {
    List {
        #[arg(long)]
        include_stopped: bool,
    },
    Inspect {
        id: Uuid,
    },
    Drain {
        id: Uuid,
    },
    Resume {
        id: Uuid,
    },
    Pools,
    Leases {
        #[arg(long)]
        worker: Option<Uuid>,
        #[arg(long)]
        include_expired: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ProfileCommand {
    Add {
        #[arg(short, long)]
        file: PathBuf,
    },
    List,
    Validate {
        profile: String,
    },
    Test {
        profile: String,
    },
    Render {
        profile: String,
    },
}

#[derive(Debug, Subcommand)]
enum SchedulerCommand {
    Status {
        #[arg(long)]
        profile: String,
    },
    Jobs {
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
    Cancel {
        execution: Uuid,
    },
}

#[derive(Debug, Subcommand)]
enum DaliugeCommand {
    Ping {
        #[arg(long)]
        profile: Option<String>,
    },
    Inspect {
        #[arg(long)]
        profile: Option<String>,
    },
    Sessions {
        #[arg(long)]
        profile: Option<String>,
    },
    SessionInspect {
        id: String,
        #[arg(long)]
        profile: Option<String>,
    },
    SessionCancel {
        id: String,
        #[arg(long)]
        profile: Option<String>,
    },
    /// Translate a persisted patched graph without creating a DALiuGE session.
    Translate {
        #[arg(long)]
        execution: Uuid,
    },
}

#[derive(Debug, Subcommand)]
enum ExecutionCommand {
    /// Retry only the last safe failed stage; uncertain submissions are refused.
    Retry {
        id: Uuid,
        #[arg(long)]
        reason: String,
    },
    /// Cancel confirmed external work and record the operator action.
    Cancel { id: Uuid },
}

#[derive(Debug, Subcommand)]
enum GraphCommand {
    /// Build a manifest and patched graph without submitting external work.
    Prepare {
        #[arg(long, default_value = "wallaby_hires")]
        project: String,
        #[arg(long, required = true, num_args = 1..)]
        source: Vec<String>,
    },
    /// Summarize source and patched graph artifacts for an execution.
    Diff {
        #[arg(long)]
        execution: Uuid,
        #[arg(long)]
        full: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SlurmCommand {
    /// SSH to login node and run squeue/sacct smoke check.
    Ping {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        user: Option<String>,
        #[arg(long, default_value_t = 22)]
        port: i32,
        #[arg(long)]
        profile: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    /// Benchmark CASDA/Vizier TAP and full discover_source for one source.
    Tap {
        #[arg(long, default_value = "HIPASSJ1313-15")]
        source: String,
        #[arg(long, default_value = "config/wallaby_hires.v2.yaml")]
        config: PathBuf,
        #[arg(long, default_value_t = 3)]
        runs: u32,
        /// Also run N parallel full discoveries per round (simulates batch concurrency).
        #[arg(long)]
        concurrent: Option<usize>,
    },
}

#[derive(Debug, Subcommand)]
enum OpenApiCommand {
    Export,
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    Validate {
        #[arg(short, long)]
        file: PathBuf,
    },
    Upload {
        #[arg(short, long)]
        file: PathBuf,
    },
    Add {
        #[arg(short, long)]
        file: PathBuf,
    },
    Explain {
        #[arg(short, long)]
        file: PathBuf,
    },
    Render {
        #[arg(short, long)]
        file: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum SetupCommand {
    /// Re-run doctor checks (same as `beampipe doctor`).
    Check,
}

#[derive(Debug, Subcommand)]
enum TimelineCommand {
    Execution {
        id: Uuid,
        #[arg(long)]
        table: bool,
    },
    Source {
        id: Uuid,
        #[arg(long)]
        table: bool,
    },
    Project {
        module: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long)]
        table: bool,
    },
}

#[derive(Debug, Subcommand)]
enum WasmCommand {
    Upload {
        #[arg(long)]
        config_id: String,
        #[arg(short, long)]
        file: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    CreateUser {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        email: String,
        #[arg(long, default_value = "Admin")]
        name: String,
        #[arg(long, default_value_t = true)]
        superuser: bool,
    },
}

fn init_tracing() {
    beampipe_metrics::init_recorder();
    beampipe_metrics::tracing_layer::init_subscriber();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();
    match cli.command {
        CliCommand::Init {
            directory,
            force,
            production,
        } => {
            let report = init::run(init::InitOptions {
                directory,
                force,
                production,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        CliCommand::Serve { worker } | CliCommand::Start { worker } => {
            std::env::set_var(
                "BEAMPIPE_PROCESS_ROLE",
                if worker { "scheduler" } else { "api" },
            );
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            if settings.migrate_on_serve {
                beampipe_db::migrate(&pool).await?;
            }
            beampipe_api::serve(settings, pool, worker).await?;
        }
        CliCommand::Worker { command: None } => {
            std::env::set_var("BEAMPIPE_PROCESS_ROLE", "worker");
            let settings = Settings::from_env()?;
            if let Err(errors) = beampipe_orchestration::validate_security(&settings) {
                anyhow::bail!("security validation failed:\n  - {}", errors.join("\n  - "));
            }
            let pool = beampipe_db::connect(&settings.database_url).await?;
            beampipe_metrics::init_recorder();
            if settings.metrics_server_enabled {
                if let Ok(addr) = settings.metrics_bind_addr.parse() {
                    drop(beampipe_metrics::server::spawn_metrics_server(
                        addr,
                        Some(pool.clone()),
                    ));
                }
            }
            let config = beampipe_jobs::WorkerConfig::from_settings(&settings);
            tracing::info!(
                concurrency = config.concurrency,
                scheduler_enabled = config.scheduler_enabled,
                discovery_source_concurrency = config.discovery_source_concurrency,
                "event=worker_only_started"
            );
            let workers = beampipe_jobs::spawn_workers(pool, config);
            shutdown_signal().await;
            workers.shutdown().await;
        }
        CliCommand::Worker {
            command: Some(command),
        } => operator::run_worker_command(command).await?,
        CliCommand::Migrate => {
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            beampipe_db::migrate(&pool).await?;
            println!("migrations applied");
        }
        CliCommand::PurgeProvenance { days } => {
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            let retention = days.unwrap_or(settings.provenance_retention_days);
            let deleted =
                beampipe_db::repo::purge_provenance_events_older_than(&pool, retention).await?;
            println!("purged {deleted} provenance events older than {retention} days");
        }
        CliCommand::Openapi {
            command: OpenApiCommand::Export,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&beampipe_api::export_openapi_json())?
            );
        }
        CliCommand::Project { command } => match command {
            ProjectCommand::Validate { file } => {
                let bytes =
                    std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
                let config = match beampipe_project::ProjectConfig::from_slice(&bytes) {
                    Ok(config) => config,
                    Err(error) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&error.validation_report(&bytes))?
                        );
                        std::process::exit(1);
                    }
                };
                let report = config.validate_report();
                if !report.warnings.is_empty() {
                    for warning in &report.warnings {
                        eprintln!(
                            "warning[{}] {}: {}",
                            warning.code, warning.path, warning.message
                        );
                    }
                }
                println!("{}", serde_json::to_string_pretty(&report)?);
                if !report.valid {
                    std::process::exit(1);
                }
            }
            ProjectCommand::Upload { file } | ProjectCommand::Add { file } => {
                let settings = Settings::from_env()?;
                let pool = beampipe_db::connect(&settings.database_url).await?;
                setup::upload_project_config_file(&pool, &file).await?;
            }
            ProjectCommand::Explain { file } => operator::explain_project(&file)?,
            ProjectCommand::Render { file } => operator::render_project(&file)?,
        },
        CliCommand::Wasm {
            command: WasmCommand::Upload { config_id, file },
        } => {
            use sha2::{Digest, Sha256};
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            let config_row = beampipe_db::repo::get_active_project_config(&pool, &config_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no active project config for {config_id}"))?;
            let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            beampipe_project::WasmHost::default().validate_module(&bytes)?;
            let wasm_sha256 = format!("{:x}", Sha256::digest(&bytes));
            beampipe_db::repo::insert_project_config_wasm(
                &pool,
                config_row.uuid,
                &wasm_sha256,
                &bytes,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "wasm_sha256": wasm_sha256,
                    "project_config_id": config_row.uuid,
                }))?
            );
        }
        CliCommand::Setup {
            command,
            yes,
            database_url,
            jwt_secret,
            admin_user,
            admin_password,
            admin_email,
            project_config,
            casda_tap_url,
            tm_url,
            dim_url,
            worker_pool,
            deployment,
            profile_name,
            facility,
            ssh_host,
            ssh_user,
            slurm_account,
            slurm_partition,
            dlg_root,
            remote_home,
            remote_logs,
            use_real_backends,
            skip_admin,
            skip_upload,
        } => {
            if matches!(command, Some(SetupCommand::Check)) {
                setup::run_setup_check(false, None, false).await?;
            } else {
                setup::run_setup(setup::SetupOptions {
                    yes,
                    database_url,
                    jwt_secret,
                    admin_user,
                    admin_password,
                    admin_email,
                    project_config,
                    casda_tap_url,
                    tm_url,
                    dim_url,
                    worker_pool,
                    deployment,
                    profile_name,
                    facility,
                    ssh_host,
                    ssh_user,
                    slurm_account,
                    slurm_partition,
                    dlg_root,
                    remote_home,
                    remote_logs,
                    use_real_backends,
                    skip_admin,
                    skip_upload,
                })
                .await?;
            }
        }
        CliCommand::Doctor { json, profile, fix } => {
            setup::run_setup_check(json, profile.as_deref(), fix).await?;
        }
        CliCommand::Config { command } => match command {
            ConfigCommand::Explain => operator::explain_config()?,
        },
        CliCommand::Profile { command } => operator::run_profile_command(command).await?,
        CliCommand::Scheduler { command } => operator::run_scheduler_command(command).await?,
        CliCommand::Daliuge { command } => operator::run_daliuge_command(command).await?,
        CliCommand::Execution { command } => operator::run_execution_command(command).await?,
        CliCommand::Graph { command } => operator::run_graph_command(command).await?,
        CliCommand::Status => {
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            let summary = doctor::run_status(&pool).await;
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        CliCommand::Console { refresh_ms } => console::run(refresh_ms).await?,
        CliCommand::Timeline { command } => {
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            match command {
                TimelineCommand::Execution { id, table } => {
                    let t = timeline::execution_timeline(&pool, id).await?;
                    if table {
                        timeline::print_table_execution(&t);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&t)?);
                    }
                }
                TimelineCommand::Source { id, table } => {
                    let t = timeline::source_timeline(&pool, id).await?;
                    if table {
                        timeline::print_table_source(&t);
                    } else {
                        println!("{}", serde_json::to_string_pretty(&t)?);
                    }
                }
                TimelineCommand::Project {
                    module,
                    limit,
                    table,
                } => {
                    let events = timeline::project_timeline(&pool, &module, limit).await?;
                    if table {
                        for e in &events {
                            println!("{} {} {:?}", e.at, e.event_type, e.correlation_id);
                        }
                    } else {
                        println!("{}", serde_json::to_string_pretty(&events)?);
                    }
                }
            }
        }
        CliCommand::Admin {
            command:
                AdminCommand::CreateUser {
                    username,
                    password,
                    email,
                    name,
                    superuser,
                },
        } => {
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            beampipe_db::migrate(&pool).await?;
            let hashed = beampipe_auth::hash_password(&password)?;
            let user =
                beampipe_db::repo::create_user(&pool, &name, &username, &email, &hashed, superuser)
                    .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "id": user.id,
                    "uuid": user.uuid,
                    "username": user.username,
                    "email": user.email,
                    "is_superuser": user.is_superuser,
                }))?
            );
        }
        CliCommand::Slurm {
            command:
                SlurmCommand::Ping {
                    host,
                    user,
                    port,
                    profile,
                },
        } => {
            let (login, remote_user, ssh_port) = if let Some(profile_name) = profile {
                let settings = Settings::from_env()?;
                let pool = beampipe_db::connect(&settings.database_url).await?;
                let row = beampipe_db::repo::get_deployment_profile_by_name(&pool, &profile_name)
                    .await?
                    .ok_or_else(|| {
                        anyhow::anyhow!("deployment profile not found: {profile_name}")
                    })?;
                let dep: beampipe_profiles::SlurmRemoteDeploymentConfig =
                    match serde_json::from_value(row.deployment)? {
                        beampipe_profiles::DeploymentConfig::SlurmRemote(s) => s,
                        _ => anyhow::bail!("profile is not slurm_remote"),
                    };
                (
                    dep.login_node,
                    dep.remote_user
                        .or_else(|| std::env::var("SLURM_REMOTE_USER").ok())
                        .or(user),
                    dep.ssh_port,
                )
            } else {
                (
                    host.ok_or_else(|| anyhow::anyhow!("--host or --profile required"))?,
                    user.or_else(|| std::env::var("SLURM_REMOTE_USER").ok())
                        .or_else(|| std::env::var("USER").ok()),
                    port,
                )
            };
            let remote_user = remote_user.ok_or_else(|| anyhow::anyhow!("remote user required"))?;
            let deployment = beampipe_profiles::SlurmRemoteDeploymentConfig {
                login_node: login,
                ssh_port,
                remote_user: Some(remote_user.clone()),
                account: String::new(),
                home_dir: String::new(),
                log_dir: String::new(),
                exec_prefix: String::new(),
                dlg_root: String::new(),
                venv: None,
                modules: None,
                facility: String::new(),
                job_duration_minutes: 0,
                num_nodes: 1,
                num_islands: 1,
                verbose_level: 0,
                max_threads: 0,
                all_nics: false,
                zerorun: false,
                sleepncopy: false,
                check_with_session: false,
                verify_ssl: None,
                slurm_template: None,
                resources: Default::default(),
                manager_topology: Default::default(),
                container_runtime: None,
                environment_setup: None,
            };
            let target =
                beampipe_orchestration::SlurmTarget::from_deployment(&deployment, &remote_user);
            let mut session = beampipe_orchestration::SlurmSshSession::connect(&target).await?;
            let squeue_out = session
                .run_command("squeue -u $USER -h | head -3")
                .await
                .context("squeue via russh")?;
            let sacct_out = session
                .run_command("sacct -u $USER --format=JobID,State --noheader | head -3")
                .await
                .unwrap_or_else(|e| format!("(sacct failed: {e})"));
            let _ = session.close().await;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "target": format!("{remote_user}@{}", deployment.login_node),
                    "transport": "russh",
                    "squeue_stdout": squeue_out.trim(),
                    "sacct_stdout": sacct_out.trim(),
                }))?
            );
        }
        CliCommand::Security {
            command: SecurityCommand::Check,
        } => {
            let settings = Settings::from_env()?;
            let issues = beampipe_orchestration::collect_security_issues(&settings);
            let slurm_ok = beampipe_orchestration::SlurmSshCredentials::try_resolve_ok();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ok": issues.is_empty(),
                    "beampipe_env": settings.beampipe_env,
                    "slurm_ssh_configured": slurm_ok,
                    "issues": issues,
                }))?
            );
            if !issues.is_empty() {
                std::process::exit(1);
            }
        }
        CliCommand::MigrateData => {
            println!("Python → Rust data migration is not automated yet.");
            println!("Export these tables from Python Postgres and import into Rust schema:");
            println!("  users, source_registry, archive_metadata, batch_execution_records, daliuge_deployment_profile");
            println!("Compare ledger snapshots via GET /api/v2/executions/{{id}}/ledger-snapshot");
        }
        CliCommand::Bench { command } => match command {
            BenchCommand::Tap {
                source,
                config,
                runs,
                concurrent,
            } => {
                bench_tap::run(&source, &config, runs, concurrent).await?;
            }
        },
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("event=shutdown_signal_received");
}

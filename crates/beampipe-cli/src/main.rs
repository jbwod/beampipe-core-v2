mod bench_tap;

use anyhow::Context;
use beampipe_config::Settings;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "beampipe", version, about = "Beampipe v2 Rust control plane")]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Run API, optionally with the embedded Postgres job worker.
    Serve {
        /// Run the embedded Postgres job worker in the API process (`false` for API-only).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        worker: bool,
    },
    /// Run worker-only mode.
    Worker,
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
    /// Print first-run setup guidance.
    Setup,
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
        CliCommand::Serve { worker } => {
            let settings = Settings::from_env()?;
            let pool = beampipe_db::connect(&settings.database_url).await?;
            if settings.migrate_on_serve {
                beampipe_db::migrate(&pool).await?;
            }
            beampipe_api::serve(settings, pool, worker).await?;
        }
        CliCommand::Worker => {
            let settings = Settings::from_env()?;
            if let Err(errors) = beampipe_orchestration::validate_security(&settings) {
                anyhow::bail!("security validation failed:\n  - {}", errors.join("\n  - "));
            }
            let pool = beampipe_db::connect(&settings.database_url).await?;
            beampipe_metrics::init_recorder();
            if settings.metrics_server_enabled {
                if let Ok(addr) = settings.metrics_bind_addr.parse() {
                    let _ =
                        beampipe_metrics::server::spawn_metrics_server(addr, Some(pool.clone()));
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
        CliCommand::Project {
            command: ProjectCommand::Validate { file },
        } => {
            let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            let config = beampipe_project::ProjectConfig::from_slice(&bytes)?;
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
        CliCommand::Setup => {
            println!("Beampipe v2 setup");
            println!("1. Set DATABASE_URL (Postgres connection string)");
            println!("2. Set BEAMPIPE_JWT_SECRET");
            println!("3. Optional: BEAMPIPE_CASDA_TAP_URL, BEAMPIPE_VIZIER_TAP_URL");
            println!("4. Optional: BEAMPIPE_SHAPING_* and BEAMPIPE_REDIS_URL for rate limits");
            println!("5. Optional worker scale: BEAMPIPE_WORKER_CONCURRENCY, BEAMPIPE_WORKER_SCHEDULER_ENABLED, BEAMPIPE_DB_MAX_CONNECTIONS, BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY");
            println!("6. Optional: CASDA_USERNAME, CASDA_PASSWORD for staging");
            println!("7. Run: beampipe migrate");
            println!(
                "8. Run: beampipe admin create-user --username admin --password ... --email ..."
            );
            println!("9. Upload config: beampipe project validate -f config/wallaby_hires.v2.yaml");
            println!("10. Optional WASM: beampipe wasm upload --config-id wallaby_hires --file hook.wasm");
            println!("11. Start API: beampipe serve --worker false");
            println!("12. Start workers: beampipe worker  (scale replicas; set BEAMPIPE_WORKER_SCHEDULER_ENABLED=false on workers when one scheduler pod runs serve --worker true)");
            println!("\nPostgres-only stack: docker compose up -d");
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

use anyhow::{bail, Context, Result};
use beampipe_config::Settings;
use beampipe_db::repo;
use beampipe_profiles::{DeploymentConfig, DeploymentProfile};
use beampipe_project::ProjectConfig;
use crossterm::style::Stylize;
use sqlx::PgPool;
use std::io::{self, IsTerminal, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use uuid::Uuid;

use crate::{
    doctor,
    installation::{self, InstallationState, RuntimeMode as RuntimeKind},
    materialize, runtime,
};

const DEFAULT_CASDA_TAP_URL: &str = "https://casda.csiro.au/casda_vo_tools/tap/sync";
const DEFAULT_TM_URL: &str = "http://localhost:9000";
const DEFAULT_WORKER_POOL: &str = "default";
const DEFAULT_DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/beampipe";
const SETUP_LOGO: &str = include_str!("../../../assets/brand/beampipe-terminal-logo.txt");

#[derive(Debug, Clone, Default)]
pub struct SetupOptions {
    pub yes: bool,
    pub database_url: Option<String>,
    pub jwt_secret: Option<String>,
    pub admin_user: Option<String>,
    pub admin_password: Option<String>,
    pub admin_password_file: Option<PathBuf>,
    pub admin_email: Option<String>,
    pub project_config: Option<PathBuf>,
    pub profile_config: Option<PathBuf>,
    pub ssh_slot: Option<String>,
    pub ssh_private_key: Option<PathBuf>,
    pub ssh_public_key: Option<PathBuf>,
    pub ssh_known_hosts: Option<PathBuf>,
    pub ssh_passphrase_file: Option<PathBuf>,
    pub ssh_acl: bool,
    pub accept_host_key: bool,
    pub casda_tap_url: Option<String>,
    pub tm_url: Option<String>,
    pub worker_pool: Option<String>,
    pub skip_admin: bool,
    pub skip_upload: bool,
    pub docker: bool,
    pub skip_docker: bool,
    pub runtime: Option<String>,
    pub postgres: Option<String>,
    pub api_port: Option<u16>,
    pub postgres_port: Option<u16>,
    pub metrics_port: Option<u16>,
    pub dashboard: bool,
    pub skip_dashboard: bool,
    pub dash_dir: Option<PathBuf>,
    pub dash_repo_url: Option<String>,
    pub directory: Option<PathBuf>,
    pub credentials_dir: Option<PathBuf>,
    pub start: bool,
    /// Write `BEAMPIPE_USE_REAL_BACKENDS=true` during setup.
    pub use_real_backends: bool,
}

#[derive(Debug, Clone, Default)]
pub struct UninstallOptions {
    pub yes: bool,
    pub purge_binary: bool,
    pub keep_volumes: bool,
    pub directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostgresKind {
    Compose,
    Existing,
}

impl PostgresKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Compose => "compose",
            Self::Existing => "existing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HostPorts {
    api: u16,
    postgres: u16,
    metrics: u16,
}

#[derive(Debug, Clone, Copy)]
struct ChoiceItem {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
}

fn stdout_is_tty() -> bool {
    io::stdout().is_terminal()
}

fn format_step(n: usize, total: usize, title: &str) -> String {
    format!("== {n}/{total}  {title} ==")
}

fn print_setup_logo() {
    let logo = SETUP_LOGO.trim_end();
    if stdout_is_tty() {
        println!("{}", logo.cyan());
    } else {
        println!("{logo}");
    }
    println!();
}

fn print_banner(start: bool, yes: bool) {
    print_setup_logo();
    let title = "Beampipe setup";
    if stdout_is_tty() {
        println!("{}", title.bold());
    } else {
        println!("{title}");
    }
    if start {
        print_hint("Writes files, then starts Postgres and the stack.");
    } else {
        print_hint("Writes files. Does not start Postgres or the stack (--no-start).");
    }
    if yes {
        print_hint("Non-interactive (--yes): Next actions are printed as a recipe.");
    } else {
        print_hint(
            "After the stack is up, setup prompts Next actions (live backends, profiles, Slurm, CASDA).",
        );
    }
}

fn print_step(n: usize, total: usize, title: &str) {
    let heading = format_step(n, total, title);
    println!();
    if stdout_is_tty() {
        println!("{}", heading.bold());
    } else {
        println!("{heading}");
    }
}

fn print_hint(text: &str) {
    println!("  {text}");
}

fn parse_choice(input: &str, items: &[ChoiceItem], default_index: usize) -> Option<usize> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Some(default_index);
    }
    if let Ok(number) = trimmed.parse::<usize>() {
        if (1..=items.len()).contains(&number) {
            return Some(number - 1);
        }
        return None;
    }
    items
        .iter()
        .position(|item| item.key.eq_ignore_ascii_case(trimmed))
}

fn print_choice_items(items: &[ChoiceItem], default_index: usize) {
    for (index, item) in items.iter().enumerate() {
        let marker = if index == default_index {
            "  [default]"
        } else {
            ""
        };
        println!("  {}) {}   {}{marker}", index + 1, item.label, item.hint);
    }
}

fn prompt_choice(label: &str, items: &[ChoiceItem], default_index: usize) -> Result<usize> {
    let _ = label;
    loop {
        print_choice_items(items, default_index);
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if let Some(index) = parse_choice(&line, items, default_index) {
            return Ok(index);
        }
        print_hint("Enter a number or the option name.");
    }
}

fn recipe_lines(commands: &[String]) -> Vec<String> {
    let mut lines = vec![String::new(), "Next (nothing was started)".into()];
    lines.extend(commands.iter().cloned());
    lines
}

fn print_recipe(commands: &[String]) {
    for line in recipe_lines(commands) {
        println!("{line}");
    }
}

fn env_override(flag: Option<&str>, env_key: &str, default: &str) -> String {
    flag.filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .or_else(|| {
            std::env::var(env_key)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| default.to_string())
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn resolve_use_real_backends(
    flag: bool,
    process_env: Option<&str>,
    file_env: Option<&str>,
) -> &'static str {
    if flag {
        return "true";
    }
    if let Some(value) = process_env.and_then(parse_env_bool) {
        return if value { "true" } else { "false" };
    }
    if let Some(value) = file_env.and_then(parse_env_bool) {
        return if value { "true" } else { "false" };
    }
    "false"
}

fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

pub async fn run_setup(mut opts: SetupOptions) -> Result<()> {
    let mut root = resolve_operator_root(&opts)?;
    std::fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    root = root
        .canonicalize()
        .with_context(|| format!("resolve {}", root.display()))?;
    let existing_context = installation::InstallationContext::from_home(root.clone())?;
    let env_existed = existing_context.environment_file.is_file();
    let compose_preexisting = root.join("docker-compose.yml").is_file();
    if existing_context.exists() {
        existing_context.activate()?;
    }
    if let Some(state) = existing_context.state.as_ref() {
        if opts.runtime.is_none() && !opts.docker && !opts.skip_docker {
            opts.runtime = Some(state.runtime.as_str().into());
        }
        if opts.postgres.is_none() {
            opts.postgres = Some(state.database_mode.clone());
        }
        println!(
            "Existing installation detected: runtime={}, database={}.",
            state.runtime.as_str(),
            state.database_mode
        );
    }

    print_banner(opts.start, opts.yes);
    preflight_before_materialize(&opts)?;
    std::env::set_current_dir(&root).with_context(|| format!("chdir {}", root.display()))?;
    let env_path = existing_context.environment_file.clone();

    let materialized = materialize::materialize(&root, false)?;
    for path in &materialized.created {
        println!("Created {}", path.display());
    }
    for path in &materialized.replaced {
        println!("Updated managed file {}", path.display());
    }
    if !materialized.bundle_current {
        println!(
            "Preserved operator-modified bundle files. Review them before relying on new release defaults."
        );
    }
    let compose_exists = compose_file_exists(&root);
    let tentative_docker = !matches!(decide_runtime(&opts)?, Some(RuntimeKind::Host));
    let total_steps = setup_step_total(&opts, tentative_docker);
    let mut step = 1;

    print_step(step, total_steps, "How will you run Beampipe?");
    step += 1;
    let runtime = resolve_runtime(&opts, compose_exists)?;
    if runtime == RuntimeKind::Docker && !compose_exists {
        bail!(
            "--runtime docker requires docker-compose.yml in {}",
            root.display()
        );
    }
    print_hint(match runtime {
        RuntimeKind::Docker => "Docker Compose: API, scheduler, and workers in containers.",
        RuntimeKind::Host => "Host binary: beampipe start on this machine.",
    });

    print_step(step, total_steps, "PostgreSQL");
    step += 1;
    let postgres = resolve_postgres(&opts, compose_exists)?;
    if postgres == PostgresKind::Compose && !compose_exists {
        bail!(
            "--postgres compose requires docker-compose.yml in {}",
            root.display()
        );
    }
    let postgres_password = (postgres == PostgresKind::Compose).then(|| {
        std::env::var("BEAMPIPE_POSTGRES_PASSWORD")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if env_existed {
        println!(
                        "Existing Compose database has no managed password setting; preserving the legacy password."
                    );
                    "postgres".into()
                } else {
                    generate_database_password()
                }
            })
    });
    if postgres == PostgresKind::Compose {
        print_hint("Using the installation-managed PostgreSQL service.");
        if !env_existed {
            refuse_stale_compose_postgres_volume(&root)?;
        }
        if opts.start {
            print_hint("Starting Compose Postgres next.");
        } else {
            print_hint("Start it later with: docker compose up -d postgres");
        }
    }

    print_step(step, total_steps, "Host ports");
    step += 1;
    let host_ports = resolve_host_ports(&opts, runtime, postgres)?;
    print_hint(&format!("API http://127.0.0.1:{}/api/v2", host_ports.api));
    if postgres == PostgresKind::Compose {
        print_hint(&format!("PostgreSQL 127.0.0.1:{}", host_ports.postgres));
    }
    if runtime == RuntimeKind::Docker {
        print_hint(&format!("Metrics 127.0.0.1:{}", host_ports.metrics));
    }

    let database_url = if postgres == PostgresKind::Compose
        && opts.database_url.is_none()
        && (!env_existed || opts.postgres_port.is_some())
    {
        compose_host_database_url(
            postgres_password.as_deref().unwrap_or_default(),
            host_ports.postgres,
        )
    } else {
        resolve_database_url(&opts, postgres)?
    };
    if runtime == RuntimeKind::Docker
        && postgres == PostgresKind::Existing
        && (database_url.contains("@localhost") || database_url.contains("@127.0.0.1"))
    {
        print_hint(
            "The external database URL points at the container itself. Use a hostname reachable from Docker, such as host.docker.internal on Docker Desktop.",
        );
    }

    let prepare_docker = runtime == RuntimeKind::Docker;
    preflight_after_choices(&opts, runtime, postgres, host_ports)?;

    let mut dash_dir = None;
    if decide_dashboard(&opts, prepare_docker) != Some(false) {
        print_step(step, total_steps, "Dashboard");
        step += 1;
        if resolve_prepare_dashboard(&opts, prepare_docker)? {
            match prepare_dashboard(&root, &opts, &compose_network_name(&root)) {
                Ok(prepared) => {
                    println!("Prepared Beampipe Dash at {}.", prepared.display());
                    dash_dir = Some(prepared);
                }
                Err(error) => {
                    println!("Dash preparation skipped: {error}");
                }
            }
        }
    } else if opts.dashboard && !prepare_docker {
        println!("--dashboard requires --runtime docker; skipped.");
    }

    print_step(step, total_steps, "Files");
    if !env_path.exists() {
        seed_env_file(&root, &env_path)?;
    } else if !opts.yes {
        print!("`.env` already exists. Continue without overwriting? [Y/n] ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if line.trim().eq_ignore_ascii_case("n") {
            bail!("setup aborted");
        }
    }

    let credential_root =
        installation::resolve_credential_root(&root, opts.credentials_dir.as_deref())?;
    std::fs::create_dir_all(&credential_root)
        .with_context(|| format!("create {}", credential_root.display()))?;
    if credential_root != root.join("credentials/ssh") {
        println!(
            "Using existing SSH credential root {}.",
            credential_root.display()
        );
    }

    let jwt_secret = select_jwt_secret(
        opts.jwt_secret.as_deref(),
        std::env::var("BEAMPIPE_JWT_SECRET").ok().as_deref(),
        env_existed,
    )?;
    if !env_existed && opts.jwt_secret.is_none() {
        println!("Generated a random JWT secret and stored it in .env.");
    }

    let casda_tap_url = env_override(
        opts.casda_tap_url.as_deref(),
        "BEAMPIPE_CASDA_TAP_URL",
        DEFAULT_CASDA_TAP_URL,
    );
    let tm_url = env_override(opts.tm_url.as_deref(), "BEAMPIPE_TM_URL", DEFAULT_TM_URL);
    let worker_pool = env_override(
        opts.worker_pool.as_deref(),
        "BEAMPIPE_WORKER_POOL",
        DEFAULT_WORKER_POOL,
    );
    let mut use_real_backends = resolve_use_real_backends(
        opts.use_real_backends,
        std::env::var("BEAMPIPE_USE_REAL_BACKENDS").ok().as_deref(),
        env_file_value(&env_path, "BEAMPIPE_USE_REAL_BACKENDS").as_deref(),
    )
    .to_string();

    update_env_file(&env_path, "DATABASE_URL", &database_url)?;
    if let Some(password) = postgres_password.as_deref() {
        update_env_file(&env_path, "BEAMPIPE_POSTGRES_PASSWORD", password)?;
    }
    let docker_database_url = if postgres == PostgresKind::Compose {
        format!(
            "postgres://postgres:{}@postgres:5432/beampipe",
            postgres_password.as_deref().unwrap_or_default()
        )
    } else {
        database_url.clone()
    };
    update_env_file(
        &env_path,
        "BEAMPIPE_DATABASE_URL_DOCKER",
        &docker_database_url,
    )?;
    update_env_file(&env_path, "BEAMPIPE_JWT_SECRET", &jwt_secret)?;
    update_env_file(&env_path, "BEAMPIPE_CASDA_TAP_URL", &casda_tap_url)?;
    update_env_file(&env_path, "BEAMPIPE_TM_URL", &tm_url)?;
    update_env_file(&env_path, "BEAMPIPE_WORKER_POOL", &worker_pool)?;
    update_env_file(&env_path, "BEAMPIPE_USE_REAL_BACKENDS", &use_real_backends)?;
    update_env_file(
        &env_path,
        "BEAMPIPE_SSH_CREDENTIALS_HOST",
        &credential_root.display().to_string(),
    )?;
    update_env_file(
        &env_path,
        "BEAMPIPE_SSH_CREDENTIALS_DIR",
        &credential_root.display().to_string(),
    )?;
    persist_host_ports(&env_path, host_ports, runtime)?;
    clear_missing_config_path(&root, &env_path)?;
    std::env::set_var("BEAMPIPE_API_PORT", host_ports.api.to_string());
    std::env::set_var("BEAMPIPE_POSTGRES_PORT", host_ports.postgres.to_string());
    std::env::set_var("BEAMPIPE_METRICS_PORT", host_ports.metrics.to_string());
    if runtime == RuntimeKind::Host {
        std::env::set_var(
            "BEAMPIPE_BIND_ADDR",
            format!("127.0.0.1:{}", host_ports.api),
        );
        std::env::set_var(
            "BEAMPIPE_METRICS_BIND_ADDR",
            format!("127.0.0.1:{}", host_ports.metrics),
        );
    }
    if env_existed {
        ensure_beampipe_version(&root, &env_path)?;
    } else {
        update_env_file(&env_path, "BEAMPIPE_VERSION", env!("CARGO_PKG_VERSION"))?;
    }
    println!("Wrote .env (0600)");

    std::env::set_var("DATABASE_URL", &database_url);
    std::env::set_var("BEAMPIPE_JWT_SECRET", &jwt_secret);
    std::env::set_var("BEAMPIPE_CASDA_TAP_URL", &casda_tap_url);
    std::env::set_var("BEAMPIPE_TM_URL", &tm_url);
    std::env::set_var("BEAMPIPE_WORKER_POOL", &worker_pool);
    std::env::set_var("BEAMPIPE_USE_REAL_BACKENDS", &use_real_backends);
    std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", &credential_root);

    installation::write_state(
        &root,
        &InstallationState {
            schema_version: installation::INSTALLATION_SCHEMA_VERSION,
            beampipe_version: env!("CARGO_PKG_VERSION").into(),
            runtime,
            database_mode: postgres.as_str().into(),
            home: root.clone(),
            environment_file: env_path.clone(),
            config_file: existing_context.config_file.clone(),
            credential_root: credential_root.clone(),
            operator_bundle_version: if materialized.bundle_current {
                env!("CARGO_PKG_VERSION").into()
            } else {
                existing_context
                    .state
                    .as_ref()
                    .map(|state| state.operator_bundle_version.clone())
                    .unwrap_or_else(|| {
                        if compose_preexisting {
                            "legacy-unversioned".into()
                        } else {
                            env!("CARGO_PKG_VERSION").into()
                        }
                    })
            },
            compose_project: existing_context
                .state
                .as_ref()
                .map(|state| state.compose_project.clone())
                .unwrap_or_else(|| installation::compose_project_name(&root)),
        },
    )?;
    println!(
        "Recorded installation state at {}.",
        root.join(installation::INSTALLATION_STATE_FILE).display()
    );

    let mut docker_context = None;
    if prepare_docker || (opts.start && postgres == PostgresKind::Compose) {
        docker_context = prepare_docker_env(&root, &env_path)?;
        if opts.start {
            println!(
                "Prepared Docker Compose (network {}).",
                compose_network_name(&root)
            );
        } else {
            println!(
                "Prepared Docker Compose (network {}). Containers were not started.",
                compose_network_name(&root)
            );
        }
        if let Some(context) = docker_context.as_deref() {
            println!("Docker context: {context}");
        }
    }

    if opts.start && postgres == PostgresKind::Compose {
        if let Some(endpoint) = remote_docker_endpoint() {
            bail!(
                "the active Docker context uses remote endpoint {endpoint}; host-side setup cannot seed its private Compose database. Use --postgres existing with a database reachable from both host and containers, or select a local Docker context"
            );
        }
        require_docker_compose()?;
        if prepare_docker {
            compose_pull_api(&root)?;
        }
        compose_up_postgres(&root)?;
    }

    let (profile_path, mut prepared_profile) =
        match select_and_prepare_profile(&opts, &root, runtime)? {
            Some((path, profile)) => (Some(path), Some(profile)),
            None => (None, None),
        };

    let pool = match beampipe_db::connect(&database_url).await {
        Ok(pool) => Some(pool),
        Err(error) if postgres == PostgresKind::Compose => {
            println!(
                "PostgreSQL is not reachable ({error}). Skipping migrate, admin, upload, and doctor."
            );
            if database_error_is_password_auth(&error) {
                println!(
                    "Compose Postgres kept an older password in its volume. Reset it, then re-run setup:"
                );
                println!(
                    "  docker compose --project-directory {} down --volumes",
                    root.display()
                );
            } else {
                println!("PostgreSQL is not up. Seed is in the recipe.");
            }
            None
        }
        Err(error) => {
            return Err(error).context(
                "database connect (use --postgres compose to print a Compose postgres recipe instead)",
            );
        }
    };

    let mut db_applied = false;
    let mut admin_ready = false;
    if let Some(pool) = pool.as_ref() {
        beampipe_db::migrate(pool).await.context("migrate")?;
        println!("Migrations applied.");
        db_applied = true;

        if !opts.skip_admin {
            create_admin_user(pool, &opts, host_ports.api).await?;
            admin_ready = true;
        }

        let project_path = project_config_path(&root, &opts);

        if project_path.exists() {
            let bytes = std::fs::read(&project_path)
                .with_context(|| format!("read {}", project_path.display()))?;
            let config = ProjectConfig::from_slice(&bytes)?;
            let report = config.validate_report();
            if !report.valid {
                bail!("project config invalid: {:?}", report.errors);
            }
            println!(
                "Validated {} (project_id={})",
                project_path.display(),
                config.metadata.id
            );

            if !opts.skip_upload
                && (opts.yes || prompt_yes_no("Upload project config to database?", true)?)
            {
                upload_project_config(pool, &config, &report.spec_sha256).await?;
                println!("Uploaded project config '{}'.", config.metadata.id);
            }
        } else {
            println!(
                "Project config not found at {}; skipped validate/upload.",
                project_path.display()
            );
        }

        if let Some(profile) = prepared_profile.as_ref() {
            let row = crate::operator::install_profile(pool, profile).await?;
            println!(
                "Installed deployment profile '{}' revision {}.",
                row.name, row.revision
            );
        }

        let settings = Settings::load()?.settings;
        let setup_context = installation::InstallationContext::from_home(root.clone())?;
        let report =
            doctor::run_doctor(pool, &settings, None, Vec::new(), Some(&setup_context)).await;
        doctor::print_human(&report);
        if !report.ok {
            bail!("setup completed with doctor failures; fix checks above");
        }
    } else {
        maybe_validate_project_config(&root, &opts)?;
    }

    let project_path = project_config_path(&root, &opts);
    let commands = next_steps_lines(&SetupNextSteps {
        runtime_docker: prepare_docker,
        compose_postgres: postgres == PostgresKind::Compose,
        docker_context,
        db_applied,
        admin_ready,
        core_home: Some(root.clone()),
        dash_dir: dash_dir.clone(),
        project_file: project_path
            .exists()
            .then(|| display_repo_path(&root, &project_path)),
        profile_file: profile_path
            .as_deref()
            .filter(|path| path.exists())
            .map(|path| display_repo_path(&root, path)),
    });
    if opts.start {
        finish_start(&root, prepare_docker, &opts, host_ports)?;
        if let Some(dash) = dash_dir.as_ref() {
            start_dashboard(&root, dash);
        }
    } else {
        print_recipe(&commands);
    }
    print_setup_summary(
        &root,
        runtime,
        postgres,
        host_ports,
        opts.start,
        prepared_profile.as_ref(),
        &use_real_backends,
    );
    offer_next_actions(&mut NextActions {
        opts: &opts,
        root: &root,
        env_path: &env_path,
        runtime,
        started: opts.start,
        pool: pool.as_ref(),
        prepared_profile: &mut prepared_profile,
        use_real_backends: &mut use_real_backends,
    })
    .await?;
    print_access_summary(&root, runtime, host_ports, opts.start, dash_dir.as_deref());
    Ok(())
}

pub fn run_uninstall(opts: UninstallOptions) -> Result<()> {
    let home = installation::resolve_home(opts.directory.as_deref())?;
    let context = installation::InstallationContext::from_home(home)?;
    if !context.exists() {
        bail!("no Beampipe installation at {}", context.home.display());
    }
    let home = context
        .home
        .canonicalize()
        .with_context(|| format!("resolve {}", context.home.display()))?;
    assert_safe_to_delete(&home)?;
    let context = installation::InstallationContext::from_home(home.clone())?;

    let title = "Beampipe uninstall";
    if stdout_is_tty() {
        println!("{}", title.bold());
    } else {
        println!("{title}");
    }
    print_hint(&format!("Installation: {}", home.display()));
    match context.state.as_ref().map(|state| state.runtime) {
        Some(RuntimeKind::Docker) => {
            print_hint("Stops Compose services and deletes the operator directory.");
        }
        Some(RuntimeKind::Host) => {
            print_hint("Does not stop a host `beampipe start` process. Stop that yourself first.");
        }
        None => print_hint("Deletes the operator directory."),
    }
    if !opts.keep_volumes {
        print_hint("Compose volumes, including managed PostgreSQL data, are deleted.");
    }
    let credential_root = context
        .credential_root
        .canonicalize()
        .unwrap_or_else(|_| context.credential_root.clone());
    if !credential_root.starts_with(&home) {
        print_hint(&format!(
            "SSH credential root {} is outside the installation and will be kept.",
            credential_root.display()
        ));
    }
    if opts.purge_binary {
        print_hint("Also removes ~/.local/bin/beampipe.");
    }

    if !opts.yes && !prompt_yes_no("Delete this installation?", false)? {
        bail!("uninstall aborted");
    }

    if context.home.join("docker-compose.yml").is_file() {
        match runtime::down(&context, !opts.keep_volumes) {
            Ok(()) => println!("Stopped Compose project."),
            Err(error) => println!("Compose teardown skipped: {error}"),
        }
    }

    std::fs::remove_dir_all(&home).with_context(|| format!("remove {}", home.display()))?;
    println!("Removed {}", home.display());

    if opts.purge_binary {
        purge_release_binary()?;
    } else {
        print_hint(
            "The beampipe binary was kept. Pass --purge-binary to remove ~/.local/bin/beampipe.",
        );
    }
    Ok(())
}

fn assert_safe_to_delete(home: &Path) -> Result<()> {
    if !home.is_absolute() {
        bail!("installation home must be an absolute path");
    }
    if home.parent().is_none() {
        bail!("refusing to delete {}", home.display());
    }
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    if home.parent().is_none() {
        bail!("refusing to delete {}", home.display());
    }
    if let Some(user_home) = std::env::var_os("HOME") {
        let user_home = PathBuf::from(user_home);
        let user_home = user_home.canonicalize().unwrap_or(user_home);
        if home == user_home {
            bail!("refusing to delete the user home directory");
        }
        if user_home.starts_with(&home) {
            bail!("refusing to delete a parent of the user home directory");
        }
    }
    Ok(())
}

fn default_release_binary() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin/beampipe"))
}

fn purge_release_binary() -> Result<()> {
    let Some(path) = default_release_binary() else {
        println!("HOME is unset; skipped binary removal.");
        return Ok(());
    };
    if path.file_name().and_then(|name| name.to_str()) != Some("beampipe") {
        bail!("refusing to delete {}", path.display());
    }
    if !path.is_file() {
        println!("No {} to remove.", path.display());
        return Ok(());
    }
    std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    println!("Removed {}", path.display());
    Ok(())
}

pub async fn run_setup_check(json: bool, profile: Option<&str>, fix: bool) -> Result<()> {
    let mut fixes_applied = Vec::new();
    let context = installation::InstallationContext::resolve(None)?;
    if fix {
        let config_dir = context.home.join("config");
        if !config_dir.exists() {
            std::fs::create_dir_all(&config_dir)
                .with_context(|| format!("create {}", config_dir.display()))?;
            fixes_applied.push(format!("created {}", config_dir.display()));
        }
    }

    let mut installation_checks = doctor::installation_checks(&context);
    let settings = match Settings::load() {
        Ok(settings) => settings.settings,
        Err(error) => {
            installation_checks.push(doctor::configuration_error_check(&error.to_string()));
            let report = doctor::DoctorReport::from_checks(installation_checks, fixes_applied);
            print_doctor_report(&report, json)?;
            return Err(error.into());
        }
    };
    let pool = match beampipe_db::connect(&settings.database_url).await {
        Ok(pool) => pool,
        Err(error) => {
            installation_checks.push(doctor::database_unreachable_check(&error.to_string()));
            let report = doctor::DoctorReport::from_checks(installation_checks, fixes_applied);
            print_doctor_report(&report, json)?;
            bail!("doctor found required failures");
        }
    };
    let mut report =
        doctor::run_doctor(&pool, &settings, profile, fixes_applied, Some(&context)).await;
    report.prepend_checks(installation_checks);
    print_doctor_report(&report, json)?;
    if !report.ok {
        bail!("doctor found required failures");
    }
    Ok(())
}

fn print_doctor_report(report: &doctor::DoctorReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        doctor::print_human(report);
    }
    Ok(())
}

pub async fn upload_project_config_file(pool: &PgPool, path: &Path) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let config = ProjectConfig::from_slice(&bytes)?;
    let report = config.validate_report();
    if !report.valid {
        bail!("invalid project config: {:?}", report.errors);
    }
    upload_project_config(pool, &config, &report.spec_sha256).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "project_id": config.metadata.id,
            "spec_sha256": report.spec_sha256,
            "valid": true,
        }))?
    );
    Ok(())
}

async fn upload_project_config(
    pool: &PgPool,
    config: &ProjectConfig,
    spec_sha256: &str,
) -> Result<()> {
    let spec = serde_json::to_value(config)?;
    repo::insert_project_config(pool, &config.metadata.id, spec, spec_sha256).await?;
    Ok(())
}

fn generate_jwt_secret() -> String {
    Uuid::new_v4().simple().to_string() + &Uuid::new_v4().simple().to_string()
}

fn generate_database_password() -> String {
    Uuid::new_v4().simple().to_string() + &Uuid::new_v4().simple().to_string()
}

fn read_secret_file(path: &Path, label: &str) -> Result<String> {
    let value = std::fs::read_to_string(path)
        .with_context(|| format!("read {label} file {}", path.display()))?
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if value.is_empty() {
        bail!("{label} file {} is empty", path.display());
    }
    Ok(value)
}

fn select_jwt_secret(
    explicit: Option<&str>,
    existing: Option<&str>,
    env_existed: bool,
) -> Result<String> {
    if let Some(secret) = explicit.filter(|secret| !secret.trim().is_empty()) {
        if !jwt_secret_is_valid(secret) {
            bail!("--jwt-secret must be at least 32 characters and not a known placeholder");
        }
        return Ok(secret.to_string());
    }
    if let Some(secret) = existing.filter(|secret| !secret.trim().is_empty()) {
        if jwt_secret_is_valid(secret) {
            return Ok(secret.to_string());
        }
        if env_existed {
            bail!(
                "existing BEAMPIPE_JWT_SECRET is weak or a placeholder; setup will not rotate it silently. Supply a new secret explicitly"
            );
        }
    }
    Ok(generate_jwt_secret())
}

fn jwt_secret_is_valid(secret: &str) -> bool {
    let normalized = secret.trim().to_ascii_lowercase();
    secret.len() >= 32
        && ![
            "change-me",
            "secret-key",
            "local-dev-jwt-secret-change-me",
            "replace-with-at-least-32-random-characters",
            "change-me-to-at-least-32-random-characters",
        ]
        .contains(&normalized.as_str())
}

pub fn generate_admin_password() -> String {
    format!("bp-{}", Uuid::new_v4().simple())
}

fn resolve_operator_root(opts: &SetupOptions) -> Result<PathBuf> {
    installation::resolve_home(opts.directory.as_deref())
}

fn require_docker_compose() -> Result<()> {
    let output = Command::new("docker").args(["compose", "version"]).output();
    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("Docker Compose v2 is required. Install Docker Engine and retry. {stderr}");
        }
        Err(error) => {
            bail!("Docker Compose v2 is required. Install Docker Engine and retry. {error}")
        }
    }
}

fn setup_step_total(opts: &SetupOptions, docker: bool) -> usize {
    if decide_dashboard(opts, docker) == Some(false) {
        4
    } else {
        5
    }
}

fn port_in_use(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn require_bind_ports_free(ports: &[(u16, &str)]) -> Result<()> {
    let mut busy = Vec::new();
    for (port, name) in ports {
        if port_in_use(*port) {
            busy.push(format!("{name} (127.0.0.1:{port})"));
        }
    }
    if !busy.is_empty() {
        bail!(
            "bind ports already in use: {}. If this is leftover Beampipe Compose from a failed setup, stop it with `docker compose down --volumes` in the install home, then retry. Or pass --no-start.",
            busy.join(", ")
        );
    }
    Ok(())
}

fn parse_required_port(raw: &str, label: &str) -> Result<u16> {
    installation::parse_host_port(raw)
        .ok_or_else(|| anyhow::anyhow!("{label} must be an integer 1-65535"))
}

fn port_from_sources(
    flag: Option<u16>,
    env_value: Option<&str>,
    default: u16,
    label: &str,
) -> Result<u16> {
    if let Some(port) = flag {
        return parse_required_port(&port.to_string(), label);
    }
    if let Some(value) = env_value.map(str::trim).filter(|value| !value.is_empty()) {
        return parse_required_port(value, label);
    }
    Ok(default)
}

fn port_from_flag_or_env(
    flag: Option<u16>,
    env_key: &str,
    default: u16,
    label: &str,
) -> Result<u16> {
    port_from_sources(flag, std::env::var(env_key).ok().as_deref(), default, label)
}

fn validate_host_ports(ports: HostPorts) -> Result<()> {
    if ports.api == ports.postgres {
        bail!(
            "API port and PostgreSQL port must be different (both {})",
            ports.api
        );
    }
    if ports.api == ports.metrics {
        bail!(
            "API port and metrics port must be different (both {})",
            ports.api
        );
    }
    if ports.postgres == ports.metrics {
        bail!(
            "PostgreSQL port and metrics port must be different (both {})",
            ports.postgres
        );
    }
    Ok(())
}

fn resolved_host_ports(opts: &SetupOptions) -> Result<HostPorts> {
    let ports = HostPorts {
        api: port_from_flag_or_env(
            opts.api_port,
            "BEAMPIPE_API_PORT",
            installation::DEFAULT_API_PORT,
            "API port",
        )?,
        postgres: port_from_flag_or_env(
            opts.postgres_port,
            "BEAMPIPE_POSTGRES_PORT",
            installation::DEFAULT_POSTGRES_PORT,
            "PostgreSQL port",
        )?,
        metrics: port_from_flag_or_env(
            opts.metrics_port,
            "BEAMPIPE_METRICS_PORT",
            installation::DEFAULT_METRICS_PORT,
            "metrics port",
        )?,
    };
    validate_host_ports(ports)?;
    Ok(ports)
}

fn prompt_port(label: &str, default: u16) -> Result<u16> {
    loop {
        let raw = prompt_default(label, &default.to_string())?;
        match parse_required_port(&raw, label) {
            Ok(port) => return Ok(port),
            Err(error) => print_hint(&error.to_string()),
        }
    }
}

fn resolve_host_ports(
    opts: &SetupOptions,
    runtime: RuntimeKind,
    postgres: PostgresKind,
) -> Result<HostPorts> {
    let mut ports = resolved_host_ports(opts)?;
    if opts.yes {
        return Ok(ports);
    }
    loop {
        ports.api = prompt_port("API port", ports.api)?;
        if postgres == PostgresKind::Compose {
            ports.postgres = prompt_port("PostgreSQL port", ports.postgres)?;
        }
        if runtime == RuntimeKind::Docker {
            ports.metrics = prompt_port("Metrics port", ports.metrics)?;
        }
        match validate_host_ports(ports) {
            Ok(()) => return Ok(ports),
            Err(error) => print_hint(&error.to_string()),
        }
    }
}

fn compose_host_database_url(password: &str, postgres_port: u16) -> String {
    format!("postgres://postgres:{password}@localhost:{postgres_port}/beampipe")
}

fn persist_host_ports(env_path: &Path, ports: HostPorts, runtime: RuntimeKind) -> Result<()> {
    update_env_file(env_path, "BEAMPIPE_API_PORT", &ports.api.to_string())?;
    update_env_file(
        env_path,
        "BEAMPIPE_POSTGRES_PORT",
        &ports.postgres.to_string(),
    )?;
    update_env_file(
        env_path,
        "BEAMPIPE_METRICS_PORT",
        &ports.metrics.to_string(),
    )?;
    if runtime == RuntimeKind::Host {
        update_env_file(
            env_path,
            "BEAMPIPE_BIND_ADDR",
            &format!("127.0.0.1:{}", ports.api),
        )?;
        update_env_file(
            env_path,
            "BEAMPIPE_METRICS_BIND_ADDR",
            &format!("127.0.0.1:{}", ports.metrics),
        )?;
    }
    Ok(())
}

fn clear_missing_config_path(root: &Path, env_path: &Path) -> Result<()> {
    let Some(value) = env_file_value(env_path, "BEAMPIPE_CONFIG") else {
        return Ok(());
    };
    let path = PathBuf::from(&value);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    if path.is_file() {
        return Ok(());
    }
    update_env_file(env_path, "BEAMPIPE_CONFIG", "")?;
    println!(
        "Cleared BEAMPIPE_CONFIG; {} is not present.",
        path.display()
    );
    Ok(())
}

fn bind_ports_for_start(
    ports: HostPorts,
    runtime: RuntimeKind,
    postgres: PostgresKind,
) -> Vec<(u16, &'static str)> {
    let mut out = Vec::new();
    if postgres == PostgresKind::Compose {
        out.push((ports.postgres, "PostgreSQL"));
    }
    out.push((ports.api, "API"));
    if runtime == RuntimeKind::Docker {
        out.push((ports.metrics, "metrics"));
    }
    out
}

fn guessed_postgres_for_preflight(opts: &SetupOptions, runtime: RuntimeKind) -> PostgresKind {
    match opts.postgres.as_deref() {
        Some("existing") => PostgresKind::Existing,
        Some("compose") => PostgresKind::Compose,
        _ if runtime == RuntimeKind::Docker => PostgresKind::Compose,
        _ => PostgresKind::Existing,
    }
}

fn preflight_before_materialize(opts: &SetupOptions) -> Result<()> {
    match decide_runtime(opts)? {
        Some(RuntimeKind::Docker) => {
            require_docker_compose()?;
            if opts.yes && opts.start {
                require_bind_ports_free(&bind_ports_for_start(
                    resolved_host_ports(opts)?,
                    RuntimeKind::Docker,
                    guessed_postgres_for_preflight(opts, RuntimeKind::Docker),
                ))?;
            }
        }
        Some(RuntimeKind::Host) if opts.yes && opts.start => {
            require_bind_ports_free(&bind_ports_for_start(
                resolved_host_ports(opts)?,
                RuntimeKind::Host,
                guessed_postgres_for_preflight(opts, RuntimeKind::Host),
            ))?;
        }
        None if opts.start => {
            require_bind_ports_free(&[(resolved_host_ports(opts)?.api, "API")])?;
        }
        _ => {}
    }
    Ok(())
}

fn preflight_after_choices(
    opts: &SetupOptions,
    runtime: RuntimeKind,
    postgres: PostgresKind,
    ports: HostPorts,
) -> Result<()> {
    if runtime == RuntimeKind::Docker {
        require_docker_compose()?;
    }
    if !opts.start {
        return Ok(());
    }
    require_bind_ports_free(&bind_ports_for_start(ports, runtime, postgres))
}

fn host_start_command(root: &Path) -> String {
    format!("  beampipe --home {} start", root.display())
}

fn login_snippet_lines(username: &str, api_port: u16) -> Vec<String> {
    vec![
        format!("  export ADMIN_USER={username}"),
        "  export ADMIN_PASSWORD=\"${ADMIN_PASSWORD:?set to the password setup printed}\"".into(),
        format!("  curl -fsS -X POST http://127.0.0.1:{api_port}/api/v2/login \\"),
        "    -H 'Content-Type: application/json' \\".into(),
        "    -d \"{\\\"username\\\":\\\"${ADMIN_USER}\\\",\\\"password\\\":\\\"${ADMIN_PASSWORD}\\\"}\""
            .into(),
    ]
}

fn print_login_snippet(username: &str, api_port: u16) {
    println!("Login once the API is up:");
    for line in login_snippet_lines(username, api_port) {
        println!("{line}");
    }
}

fn compose_cmd(root: &Path, args: &[&str]) -> Result<()> {
    println!("  docker compose {}", args.join(" "));
    let context = installation::InstallationContext::from_home(root.to_path_buf())?;
    let status = runtime::compose_command(&context)?
        .args(args)
        .status()
        .context("docker compose")?;
    if !status.success() {
        bail!("docker compose {} failed", args.join(" "));
    }
    Ok(())
}

fn compose_pull_api(root: &Path) -> Result<()> {
    println!("  docker compose pull api");
    let context = installation::InstallationContext::from_home(root.to_path_buf())?;
    let status = runtime::compose_command(&context)?
        .args(["pull", "api"])
        .status()
        .context("docker compose pull")?;
    if !status.success() {
        bail!(
            "published image unavailable. Confirm ghcr.io/jbwod/beampipe-core-v2 is public, or run docker login ghcr.io. This installer does not compile from source."
        );
    }
    Ok(())
}

fn compose_up_postgres(root: &Path) -> Result<()> {
    compose_cmd(root, &["up", "-d", "--wait", "postgres"])
}

fn compose_postgres_volume_name(root: &Path) -> String {
    format!(
        "{}_beampipe_pgdata",
        installation::compose_project_name(root)
    )
}

fn docker_volume_exists(name: &str) -> bool {
    Command::new("docker")
        .args(["volume", "inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn refuse_stale_compose_postgres_volume(root: &Path) -> Result<()> {
    let volume = compose_postgres_volume_name(root);
    if !docker_volume_exists(&volume) {
        return Ok(());
    }
    bail!(
        "Compose PostgreSQL volume `{volume}` already exists from a previous install. Postgres keeps the original password in that volume, so a new .env will not authenticate. Reset it, then re-run setup:\n  docker compose --project-directory {} down --volumes",
        root.display()
    );
}

fn database_error_is_password_auth(error: &impl std::fmt::Display) -> bool {
    error
        .to_string()
        .to_ascii_lowercase()
        .contains("password authentication failed")
}

fn check_api_health(api_port: u16) {
    let url = format!("http://127.0.0.1:{api_port}/api/v2/health");
    let result = Command::new("curl").args(["-fsS", &url]).status();
    match result {
        Ok(status) if status.success() => {
            println!("API is up at http://127.0.0.1:{api_port}/api/v2");
        }
        _ => {
            println!("Check {url} when the API is ready.");
        }
    }
}

fn finish_start(
    root: &Path,
    runtime_docker: bool,
    opts: &SetupOptions,
    ports: HostPorts,
) -> Result<()> {
    if runtime_docker {
        let context = installation::InstallationContext::from_home(root.to_path_buf())?;
        runtime::start(&context)?;
        check_api_health(ports.api);
        println!("Beampipe is running from {}.", root.display());
        return Ok(());
    }
    if opts.yes {
        println!("The API is not up yet. PostgreSQL is ready. Start the host process with:");
        println!("{}", host_start_command(root));
        return Ok(());
    }
    if prompt_yes_no("Start beampipe now?", true)? {
        let exe = std::env::current_exe().context("current executable")?;
        let status = Command::new(exe)
            .arg("start")
            .current_dir(root)
            .status()
            .context("beampipe start")?;
        if !status.success() {
            bail!("beampipe start failed");
        }
    } else {
        println!("The API is not up yet. Start the host process with:");
        println!("{}", host_start_command(root));
    }
    Ok(())
}

fn prompt_default(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn prompt_yes_no(label: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "Y/n" } else { "y/N" };
    print!("{label} [{hint}]: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let t = line.trim();
    if t.is_empty() {
        return Ok(default_yes);
    }
    Ok(!t.eq_ignore_ascii_case("n"))
}

fn update_env_file(path: &Path, key: &str, value: &str) -> Result<()> {
    if key.is_empty()
        || !key.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        bail!("environment variable name must contain only A-Z, 0-9, and '_'");
    }
    if value.contains(['\n', '\r']) {
        bail!("environment variable value must be a single line");
    }
    let content = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let prefix = format!("{key}=");
    let mut found = false;
    for line in &mut lines {
        if line.starts_with(&prefix) || line.starts_with(&format!("#{key}=")) {
            *line = format!("{key}={value}");
            found = true;
            break;
        }
    }
    if !found {
        lines.push(format!("{key}={value}"));
    }
    std::fs::write(path, lines.join("\n") + "\n")?;
    set_private_file_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn default_env_skeleton() -> String {
    format!(
        "BEAMPIPE_ENV=development\nBEAMPIPE_VERSION={}\nBEAMPIPE_JWT_SECRET=change-me\nDATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe\n",
        env!("CARGO_PKG_VERSION")
    )
}

fn seed_env_file(root: &Path, env_path: &Path) -> Result<()> {
    let example_path = root.join(".env.example");
    let template_path = root.join(".env.template");
    if example_path.exists() {
        std::fs::copy(&example_path, env_path).context("copy .env.example to .env")?;
        println!("Created .env from .env.example");
    } else if template_path.exists() {
        std::fs::copy(&template_path, env_path).context("copy .env.template to .env")?;
        println!("Created .env from .env.template");
    } else {
        std::fs::write(env_path, default_env_skeleton()).context("write .env")?;
        println!("Created minimal .env");
    }
    Ok(())
}

fn env_file_value(path: &Path, key: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let prefix = format!("{key}=");
    for line in content.lines() {
        if let Some(value) = line.trim().strip_prefix(&prefix) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn ensure_beampipe_version(root: &Path, env_path: &Path) -> Result<()> {
    if !env_value_empty(env_path, "BEAMPIPE_VERSION") {
        return Ok(());
    }
    let version = std::env::var("BEAMPIPE_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| env_file_value(&root.join(".env.example"), "BEAMPIPE_VERSION"))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
    update_env_file(env_path, "BEAMPIPE_VERSION", &version)
}

const DEFAULT_DASH_REPO: &str = "https://github.com/jbwod/beampipe-dash";
const DASH_OVERRIDE_FILE: &str = "compose.beampipe-local.yml";
const DASH_INSTALL_SCRIPT: &str = "scripts/install.sh";

fn env_value_empty(path: &Path, key: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return true;
    };
    let prefix = format!("{key}=");
    for line in content.lines() {
        if let Some(value) = line.strip_prefix(&prefix) {
            return value.trim().is_empty();
        }
    }
    true
}

fn compose_file_exists(root: &Path) -> bool {
    root.join("docker-compose.yml").exists()
}

fn compose_network_name(root: &Path) -> String {
    format!("{}_default", installation::compose_project_name(root))
}

fn parse_runtime(value: &str) -> Result<RuntimeKind> {
    match value.trim() {
        "docker" => Ok(RuntimeKind::Docker),
        "host" => Ok(RuntimeKind::Host),
        other => bail!("runtime must be docker or host, got {other}"),
    }
}

fn parse_postgres(value: &str) -> Result<PostgresKind> {
    match value.trim() {
        "compose" => Ok(PostgresKind::Compose),
        "existing" => Ok(PostgresKind::Existing),
        other => bail!("postgres must be compose or existing, got {other}"),
    }
}

fn decide_runtime(opts: &SetupOptions) -> Result<Option<RuntimeKind>> {
    if let Some(runtime) = opts.runtime.as_deref() {
        return Ok(Some(parse_runtime(runtime)?));
    }
    if opts.docker && opts.skip_docker {
        bail!("--docker and --skip-docker conflict");
    }
    if opts.docker {
        return Ok(Some(RuntimeKind::Docker));
    }
    if opts.skip_docker {
        return Ok(Some(RuntimeKind::Host));
    }
    if opts.yes {
        bail!("--yes requires --runtime docker or --runtime host (or --docker / --skip-docker)");
    }
    Ok(None)
}

fn runtime_choices() -> [ChoiceItem; 2] {
    [
        ChoiceItem {
            key: "docker",
            label: "Docker Compose",
            hint: "API, scheduler, workers in containers",
        },
        ChoiceItem {
            key: "host",
            label: "Host binary",
            hint: "beampipe start on this machine",
        },
    ]
}

fn resolve_runtime(opts: &SetupOptions, compose_exists: bool) -> Result<RuntimeKind> {
    if let Some(runtime) = decide_runtime(opts)? {
        return Ok(runtime);
    }
    let _ = compose_exists;
    let index = prompt_choice("How will you run Beampipe?", &runtime_choices(), 0)?;
    Ok([RuntimeKind::Docker, RuntimeKind::Host][index])
}

fn decide_postgres(opts: &SetupOptions, compose_exists: bool) -> Result<Option<PostgresKind>> {
    if let Some(postgres) = opts.postgres.as_deref() {
        return Ok(Some(parse_postgres(postgres)?));
    }
    if opts.yes {
        return Ok(Some(if compose_exists {
            PostgresKind::Compose
        } else {
            PostgresKind::Existing
        }));
    }
    Ok(None)
}

fn postgres_choices() -> [ChoiceItem; 2] {
    [
        ChoiceItem {
            key: "compose",
            label: "Compose service",
            hint: "docker compose up -d postgres  (recommended)",
        },
        ChoiceItem {
            key: "existing",
            label: "Existing URL",
            hint: "local or remote database you already run",
        },
    ]
}

fn resolve_postgres(opts: &SetupOptions, compose_exists: bool) -> Result<PostgresKind> {
    if let Some(postgres) = decide_postgres(opts, compose_exists)? {
        return Ok(postgres);
    }
    if !compose_exists {
        return Ok(PostgresKind::Existing);
    }
    let index = prompt_choice("PostgreSQL", &postgres_choices(), 0)?;
    Ok([PostgresKind::Compose, PostgresKind::Existing][index])
}

fn resolve_database_url(opts: &SetupOptions, postgres: PostgresKind) -> Result<String> {
    if let Some(url) = opts
        .database_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(url.trim().to_string());
    }
    if postgres == PostgresKind::Compose || opts.yes {
        return Ok(std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.into()));
    }
    prompt_default("DATABASE_URL", DEFAULT_DATABASE_URL)
}

fn decide_dashboard(opts: &SetupOptions, docker: bool) -> Option<bool> {
    if !docker || opts.skip_dashboard {
        return Some(false);
    }
    if opts.dashboard {
        return Some(true);
    }
    if opts.yes {
        return Some(false);
    }
    None
}

fn resolve_prepare_dashboard(opts: &SetupOptions, docker: bool) -> Result<bool> {
    match decide_dashboard(opts, docker) {
        Some(value) => Ok(value),
        None => prompt_yes_no("Install and start Beampipe Dash?", false),
    }
}

fn docker_context_show() -> Option<String> {
    let output = std::process::Command::new("docker")
        .args(["context", "show"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn remote_docker_endpoint() -> Option<String> {
    let output = std::process::Command::new("docker")
        .args([
            "context",
            "inspect",
            "--format",
            "{{.Endpoints.docker.Host}}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let endpoint = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let remote = endpoint.starts_with("ssh://")
        || (endpoint.starts_with("tcp://")
            && !endpoint.contains("127.0.0.1")
            && !endpoint.contains("localhost"));
    remote.then_some(endpoint)
}

fn prepare_docker_env(root: &Path, env_path: &Path) -> Result<Option<String>> {
    if !compose_file_exists(root) {
        bail!("docker-compose.yml not found in {}", root.display());
    }
    if env_value_empty(env_path, "BEAMPIPE_SSH_CREDENTIALS_HOST") {
        let credential_root = installation::resolve_credential_root(root, None)?;
        update_env_file(
            env_path,
            "BEAMPIPE_SSH_CREDENTIALS_HOST",
            &credential_root.display().to_string(),
        )?;
    }
    Ok(docker_context_show())
}

fn default_dash_dir(root: &Path) -> PathBuf {
    root.parent()
        .map(|parent| parent.join("beampipe-dash"))
        .unwrap_or_else(|| PathBuf::from("../beampipe-dash"))
}

fn dash_override_contents(network: &str) -> String {
    format!(
        "\
services:
  dashboard:
    environment:
      BEAMPIPE_API_URL: http://api:8080
    ports: !override
      - \"127.0.0.1:3000:3000\"
    networks:
      - default
      - beampipe-core

networks:
  beampipe-core:
    external: true
    name: {network}
"
    )
}

fn patch_compose_network_name(contents: &str, network: &str) -> String {
    let mut after_external = false;
    let mut patched = false;
    let mut lines = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("external:") {
            after_external = true;
            lines.push(line.to_string());
            continue;
        }
        if !patched && after_external && trimmed.starts_with("name:") {
            let indent_len = line.len() - trimmed.len();
            lines.push(format!("{}name: {network}", &line[..indent_len]));
            patched = true;
            after_external = false;
            continue;
        }
        lines.push(line.to_string());
    }
    if !patched {
        return dash_override_contents(network);
    }
    let mut out = lines.join("\n");
    if contents.ends_with('\n') {
        out.push('\n');
    }
    out.replace("0.0.0.0:3000:3000", "127.0.0.1:3000:3000")
}

fn write_or_patch_dash_override(path: &Path, network: &str) -> Result<()> {
    if path.exists() {
        let contents =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        std::fs::write(path, patch_compose_network_name(&contents, network))
            .with_context(|| format!("patch {}", path.display()))?;
    } else {
        std::fs::write(path, dash_override_contents(network))
            .with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
}

fn git_clone_dash(url: &str, dest: &Path) -> Result<()> {
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(dest)
        .status()
        .with_context(|| format!("run git clone {url}"))?;
    if !status.success() {
        bail!("git clone {url} {} failed with {status}", dest.display());
    }
    Ok(())
}

fn dash_install_script(dash_dir: &Path) -> PathBuf {
    dash_dir.join(DASH_INSTALL_SCRIPT)
}

fn dash_install_recipe_line(core_home: Option<&Path>, dash_dir: &Path) -> String {
    let script = dash_install_script(dash_dir);
    match core_home {
        Some(home) => format!(
            "  sh {} --core-home {} --dash-dir {}",
            script.display(),
            home.display(),
            dash_dir.display()
        ),
        None => format!(
            "  sh {} --dash-dir {}",
            script.display(),
            dash_dir.display()
        ),
    }
}

fn run_dash_install(root: &Path, dash_dir: &Path, start: bool) -> Result<()> {
    let script = dash_install_script(dash_dir);
    let mut command = Command::new("sh");
    command
        .arg(&script)
        .arg("--core-home")
        .arg(root)
        .arg("--dash-dir")
        .arg(dash_dir)
        .arg("--yes");
    if !start {
        command.arg("--no-start");
    }
    let status = command
        .status()
        .with_context(|| format!("run {}", script.display()))?;
    if !status.success() {
        bail!("{} failed with {status}", script.display());
    }
    Ok(())
}

fn try_start_dashboard(root: &Path, dash_dir: &Path) -> Result<()> {
    if dash_install_script(dash_dir).is_file() {
        return run_dash_install(root, dash_dir, true);
    }
    let status = Command::new("docker")
        .current_dir(dash_dir)
        .args([
            "compose",
            "-f",
            "compose.yaml",
            "-f",
            DASH_OVERRIDE_FILE,
            "up",
            "--build",
            "-d",
            "--wait",
        ])
        .status()
        .context("docker compose up dash")?;
    if !status.success() {
        bail!("docker compose up dash failed with {status}");
    }
    println!("Dash is up at http://127.0.0.1:3000");
    Ok(())
}

fn start_dashboard(root: &Path, dash_dir: &Path) {
    if let Err(error) = try_start_dashboard(root, dash_dir) {
        println!("Dash start skipped: {error}");
        println!("Retry with:");
        println!("{}", dash_install_recipe_line(Some(root), dash_dir));
    }
}

fn prepare_dashboard(root: &Path, opts: &SetupOptions, network: &str) -> Result<PathBuf> {
    let dash_dir = opts
        .dash_dir
        .clone()
        .unwrap_or_else(|| default_dash_dir(root));
    let repo_url = opts
        .dash_repo_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or(DEFAULT_DASH_REPO);

    if !dash_dir.exists() {
        println!("Cloning Beampipe Dash into {}.", dash_dir.display());
        if let Err(error) = git_clone_dash(repo_url, &dash_dir) {
            bail!(
                "{error}. Clone it with: git clone {repo_url} {}",
                dash_dir.display()
            );
        }
    }

    if dash_install_script(&dash_dir).is_file() {
        run_dash_install(root, &dash_dir, false)?;
        return Ok(dash_dir);
    }

    let dash_env = dash_dir.join(".env");
    if !dash_env.exists() {
        let example = dash_dir.join(".env.example");
        if example.exists() {
            std::fs::copy(&example, &dash_env)
                .with_context(|| format!("copy {} to .env", example.display()))?;
            println!("Created Dash .env from .env.example");
        } else {
            println!(
                "Dash .env.example not found at {}; skipped .env copy.",
                example.display()
            );
        }
    }

    write_or_patch_dash_override(&dash_dir.join(DASH_OVERRIDE_FILE), network)?;
    Ok(dash_dir)
}

async fn create_admin_user(pool: &PgPool, opts: &SetupOptions, api_port: u16) -> Result<()> {
    if opts.yes {
        return create_admin_user_once(pool, opts, api_port).await;
    }
    loop {
        match create_admin_user_once(pool, opts, api_port).await {
            Ok(()) => return Ok(()),
            Err(error) if is_retryable_admin_error(&error) => {
                print_hint(&error.to_string());
                print_hint("Try a different username, email, or password.");
            }
            Err(error) => return Err(error),
        }
    }
}

async fn create_admin_user_once(pool: &PgPool, opts: &SetupOptions, api_port: u16) -> Result<()> {
    let username = if opts.yes {
        opts.admin_user.clone().unwrap_or_else(|| "admin".into())
    } else {
        prompt_default("Admin username", "admin")?
    };
    if username.trim().is_empty() {
        bail!("admin username cannot be empty");
    }

    if repo::get_user_by_username(pool, &username).await?.is_some() {
        println!("Admin user '{username}' already exists; skipped.");
        print_login_snippet(&username, api_port);
        return Ok(());
    }

    let password = if opts.yes {
        match opts
            .admin_password
            .clone()
            .filter(|password| !password.is_empty())
        {
            Some(password) => {
                eprintln!(
                    "warning: --admin-password can be exposed through shell history; prefer --admin-password-file"
                );
                password
            }
            None => {
                if let Some(path) = opts.admin_password_file.as_deref() {
                    read_secret_file(path, "admin password")?
                } else {
                    let password = generate_admin_password();
                    println!("Generated admin password (shown once): {password}");
                    password
                }
            }
        }
    } else {
        loop {
            let password = rpassword::prompt_password("Admin password (12+ characters): ")?;
            match validate_admin_password(&password) {
                Ok(()) => break password,
                Err(error) => print_hint(&error.to_string()),
            }
        }
    };
    validate_admin_password(&password)?;
    let email = if opts.yes {
        opts.admin_email
            .clone()
            .unwrap_or_else(|| "admin@example.test".into())
    } else {
        prompt_default("Admin email", "admin@example.test")?
    };
    if email.trim().is_empty() {
        bail!("admin email cannot be empty");
    }

    let hash = beampipe_auth::hash_password(&password)?;
    repo::create_user(pool, "Admin", &username, &email, &hash, true).await?;
    println!("Created admin user '{username}'.");
    print_login_snippet(&username, api_port);
    Ok(())
}

fn validate_admin_password(password: &str) -> Result<()> {
    if password.len() < 12 {
        bail!("admin password must be at least 12 characters");
    }
    Ok(())
}

fn is_retryable_admin_error(error: &anyhow::Error) -> bool {
    let text = error.to_string();
    text.contains("must be at least 12 characters")
        || text.contains("cannot be empty")
        || text.contains("duplicate key")
        || text.contains("unique constraint")
}

fn maybe_validate_project_config(root: &Path, opts: &SetupOptions) -> Result<()> {
    let project_path = project_config_path(root, opts);
    if !project_path.exists() {
        println!(
            "Project config not found at {}; skipped validate/upload.",
            project_path.display()
        );
        return Ok(());
    }
    let bytes =
        std::fs::read(&project_path).with_context(|| format!("read {}", project_path.display()))?;
    let config = ProjectConfig::from_slice(&bytes)?;
    let report = config.validate_report();
    if !report.valid {
        bail!("project config invalid: {:?}", report.errors);
    }
    println!(
        "Validated {} (project_id={}). Upload after Postgres is up.",
        project_path.display(),
        config.metadata.id
    );
    Ok(())
}

fn project_config_path(root: &Path, opts: &SetupOptions) -> PathBuf {
    match opts.project_config.as_ref() {
        Some(path) => resolve_explicit_path(path),
        None => root.join("config/wallaby_hires.v2.yaml"),
    }
}

fn resolve_explicit_path(path: &Path) -> PathBuf {
    let path = expand_user_path(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    }
}

fn expand_user_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if text == "~" {
        return PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "~".into()));
    }
    if let Some(rest) = text.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

fn resolve_under_root(root: &Path, raw: &str) -> PathBuf {
    let path = expand_user_path(Path::new(raw));
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn select_and_prepare_profile(
    opts: &SetupOptions,
    root: &Path,
    runtime: RuntimeKind,
) -> Result<Option<(PathBuf, DeploymentProfile)>> {
    if let Some(path) = opts.profile_config.as_ref() {
        let path = resolve_explicit_path(path);
        let profile = prepare_deployment_profile(opts, &path, runtime)?;
        return Ok(Some((path, profile)));
    }
    if opts.yes || !prompt_yes_no("Configure a deployment profile now?", false)? {
        return Ok(None);
    }
    prompt_profile_file(opts, root, runtime)
}

fn prompt_profile_file(
    opts: &SetupOptions,
    root: &Path,
    runtime: RuntimeKind,
) -> Result<Option<(PathBuf, DeploymentProfile)>> {
    let default = root.join("config/deployment_profile.dlg-dim.json");
    let default_display = default.display().to_string();
    loop {
        let raw = prompt_default("Deployment profile file (or skip)", &default_display)?;
        if raw.trim().eq_ignore_ascii_case("skip") {
            return Ok(None);
        }
        let path = resolve_under_root(root, &raw);
        match prepare_deployment_profile(opts, &path, runtime) {
            Ok(profile) => return Ok(Some((path, profile))),
            Err(error) => {
                print_hint(&error.to_string());
                print_hint("Enter another path, or type skip to continue without a profile.");
            }
        }
    }
}

fn prepare_deployment_profile(
    opts: &SetupOptions,
    path: &Path,
    runtime: RuntimeKind,
) -> Result<DeploymentProfile> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut profile: DeploymentProfile =
        serde_yaml::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;

    if let DeploymentConfig::SlurmRemote(slurm) = &mut profile.deployment {
        if let Some(slot) = opts.ssh_slot.as_deref() {
            beampipe_profiles::validate_ssh_credential_name(slot)?;
            slurm.ssh_credential = Some(slot.to_string());
        }
        if opts.ssh_private_key.is_some() && slurm.ssh_credential.is_none() {
            slurm.ssh_credential = Some(profile.name.clone());
        }

        if opts.ssh_private_key.is_none() && !opts.yes {
            configure_slurm_credential_interactive(slurm, &profile.name, runtime)?;
        }
        if let Some(private_key) = opts.ssh_private_key.as_ref() {
            let slot = slurm
                .ssh_credential
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Slurm profile requires an SSH slot"))?;
            crate::slurm_credentials::import(crate::slurm_credentials::ImportOptions {
                slot,
                dir: None,
                private_key: private_key.clone(),
                public_key: opts.ssh_public_key.clone(),
                known_hosts: opts.ssh_known_hosts.clone(),
                passphrase_file: opts.ssh_passphrase_file.clone(),
                host: Some(slurm.login_node.clone()),
                port: u16::try_from(slurm.ssh_port)
                    .context("deployment.ssh_port is outside the supported range")?,
                acl: opts.ssh_acl || (runtime == RuntimeKind::Docker && cfg!(target_os = "linux")),
                force: false,
                accept_host_key: opts.accept_host_key,
            })?;
        }
    }

    profile.validate()?;
    Ok(profile)
}

fn configure_slurm_credential_interactive(
    slurm: &mut beampipe_profiles::SlurmRemoteDeploymentConfig,
    profile_name: &str,
    runtime: RuntimeKind,
) -> Result<()> {
    let choices = [
        ChoiceItem {
            key: "existing",
            label: "Use existing slot",
            hint: "associate an already-managed credential",
        },
        ChoiceItem {
            key: "import",
            label: "Import an existing key",
            hint: "skip upload if the cluster already has this public key",
        },
        ChoiceItem {
            key: "generate",
            label: "Generate a new Beampipe key",
            hint: "you must still install the public key on the login node",
        },
        ChoiceItem {
            key: "later",
            label: "Configure later",
            hint: "install the profile without changing credentials",
        },
    ];
    let choice = prompt_choice("SSH credentials for this profile", &choices, 3)?;
    if choice == 3 {
        return Ok(());
    }
    let default_slot = slurm.ssh_credential.as_deref().unwrap_or(profile_name);
    let slot = prompt_default("SSH credential slot", default_slot)?;
    beampipe_profiles::validate_ssh_credential_name(&slot)?;
    slurm.ssh_credential = Some(slot.clone());
    if choice == 0 {
        crate::slurm_credentials::check(&slot, None)?;
        return Ok(());
    }
    let acl = runtime == RuntimeKind::Docker && cfg!(target_os = "linux");
    if choice == 1 {
        let private_key =
            PathBuf::from(prompt_default("Existing private key", "~/.ssh/id_ed25519")?);
        let private_key = expand_home_path(&private_key)?;
        let imported = crate::slurm_credentials::import(crate::slurm_credentials::ImportOptions {
            slot,
            dir: None,
            private_key,
            public_key: None,
            known_hosts: None,
            passphrase_file: None,
            host: Some(slurm.login_node.clone()),
            port: u16::try_from(slurm.ssh_port)?,
            acl,
            force: false,
            accept_host_key: false,
        })?;
        crate::slurm_credentials::print_init_next_steps(&imported);
    } else {
        let generated = crate::slurm_credentials::init(crate::slurm_credentials::InitOptions {
            slot,
            host: slurm.login_node.clone(),
            port: u16::try_from(slurm.ssh_port)?,
            user: slurm.remote_user.clone(),
            acl,
            ..crate::slurm_credentials::InitOptions::default()
        })?;
        crate::slurm_credentials::print_init_next_steps(&generated);
    }
    Ok(())
}

fn expand_home_path(path: &Path) -> Result<PathBuf> {
    let text = path.to_string_lossy();
    if text == "~" || text.starts_with("~/") {
        let home = std::env::var("HOME").context("HOME is not set")?;
        let suffix = text.strip_prefix("~/").unwrap_or("");
        return Ok(PathBuf::from(home).join(suffix));
    }
    Ok(path.to_path_buf())
}

#[derive(Debug, Default, Clone)]
struct SetupNextSteps {
    runtime_docker: bool,
    compose_postgres: bool,
    docker_context: Option<String>,
    db_applied: bool,
    admin_ready: bool,
    core_home: Option<PathBuf>,
    dash_dir: Option<PathBuf>,
    project_file: Option<String>,
    profile_file: Option<String>,
}

fn display_repo_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn next_action_choices() -> [ChoiceItem; 6] {
    [
        ChoiceItem {
            key: "live",
            label: "Enable live backends",
            hint: "BEAMPIPE_USE_REAL_BACKENDS=true after doctor --profile",
        },
        ChoiceItem {
            key: "profile",
            label: "Add a deployment profile",
            hint: "REST DIM or Slurm JSON from the install config dir",
        },
        ChoiceItem {
            key: "slurm",
            label: "Set up Slurm SSH credentials",
            hint: "generate or import a managed key slot",
        },
        ChoiceItem {
            key: "casda",
            label: "Set CASDA credentials",
            hint: "username and password for staging downloads",
        },
        ChoiceItem {
            key: "doctor",
            label: "Run doctor for a profile",
            hint: "beampipe doctor --profile NAME",
        },
        ChoiceItem {
            key: "done",
            label: "Done",
            hint: "finish setup",
        },
    ]
}

fn next_action_recipe_lines(root: &Path, live_already: bool) -> Vec<String> {
    let home = root.display();
    let dlg = root.join("config/deployment_profile.dlg-dim.json");
    let slurm = root.join("config/deployment_profile.slurm-remote.json");
    let mut lines = vec![
        String::new(),
        "Next actions".into(),
        "  Mock submissions finish immediately and never create a DIM session.".into(),
    ];
    if live_already {
        lines.push("  Live backends are on (BEAMPIPE_USE_REAL_BACKENDS=true).".into());
    } else {
        lines.push("  Enable live TM/DIM or Slurm only after doctor --profile passes:".into());
        lines.push(format!(
            "    set BEAMPIPE_USE_REAL_BACKENDS=true in {home}/.env"
        ));
        lines.push("    beampipe restart".into());
    }
    lines.push(format!("  beampipe profile add -f {}", dlg.display()));
    lines.push("  beampipe doctor --profile dlg-dim".into());
    lines.push("  beampipe slurm credentials init --slot hpc --host LOGIN_NODE".into());
    lines.push(format!(
        "  beampipe profile add -f {} --ssh-slot hpc",
        slurm.display()
    ));
    lines.push("  beampipe doctor --profile slurm-remote".into());
    lines.push(format!(
        "  set CASDA_USERNAME in {home}/.env (Docker: CASDA_PASSWORD; host: CASDA_PASSWORD_FILE={home}/credentials/casda/password)"
    ));
    lines.push("  beampipe restart".into());
    lines
}

fn print_next_action_recipe(root: &Path, live_already: bool) {
    for line in next_action_recipe_lines(root, live_already) {
        println!("{line}");
    }
}

fn next_actions_should_prompt(opts: &SetupOptions) -> bool {
    !opts.yes && stdin_is_tty()
}

struct NextActions<'a> {
    opts: &'a SetupOptions,
    root: &'a Path,
    env_path: &'a Path,
    runtime: RuntimeKind,
    started: bool,
    pool: Option<&'a PgPool>,
    prepared_profile: &'a mut Option<DeploymentProfile>,
    use_real_backends: &'a mut String,
}

async fn offer_next_actions(ctx: &mut NextActions<'_>) -> Result<()> {
    if !next_actions_should_prompt(ctx.opts) {
        print_next_action_recipe(ctx.root, ctx.use_real_backends.as_str() == "true");
        return Ok(());
    }

    println!();
    if stdout_is_tty() {
        println!("{}", "Next actions".bold());
    } else {
        println!("Next actions");
    }
    print_hint("Mock submissions finish immediately and never create a DIM session.");
    print_hint(&format!(
        "BEAMPIPE_USE_REAL_BACKENDS={}",
        ctx.use_real_backends
    ));
    print_hint("Enable live backends only after `beampipe doctor --profile NAME` passes.");

    let items = next_action_choices();
    loop {
        let choice = prompt_choice("Next action", &items, 5)?;
        match choice {
            0 => {
                if let Err(error) = enable_live_backends(
                    ctx.root,
                    ctx.env_path,
                    ctx.runtime,
                    ctx.started,
                    ctx.prepared_profile.as_ref(),
                    ctx.use_real_backends,
                ) {
                    print_hint(&error.to_string());
                }
            }
            1 => match prompt_profile_file(ctx.opts, ctx.root, ctx.runtime) {
                Ok(Some((_, profile))) => {
                    if let Err(error) = install_prepared_profile(ctx.pool, &profile).await {
                        print_hint(&error.to_string());
                    }
                    *ctx.prepared_profile = Some(profile);
                }
                Ok(None) => {}
                Err(error) => print_hint(&error.to_string()),
            },
            2 => {
                if let Err(error) = next_action_slurm_credentials(
                    ctx.opts,
                    ctx.runtime,
                    ctx.prepared_profile.as_mut(),
                ) {
                    print_hint(&error.to_string());
                    continue;
                }
                if let Some(profile) = ctx.prepared_profile.as_ref() {
                    if let Err(error) = install_prepared_profile(ctx.pool, profile).await {
                        print_hint(&error.to_string());
                    }
                }
            }
            3 => {
                if let Err(error) =
                    next_action_casda_credentials(ctx.root, ctx.env_path, ctx.runtime, ctx.started)
                {
                    print_hint(&error.to_string());
                }
            }
            4 => {
                if let Err(error) =
                    next_action_doctor_profile(ctx.root, ctx.pool, ctx.prepared_profile.as_ref())
                        .await
                {
                    print_hint(&error.to_string());
                }
            }
            _ => break,
        }
    }
    Ok(())
}

async fn install_prepared_profile(
    pool: Option<&PgPool>,
    profile: &DeploymentProfile,
) -> Result<()> {
    let Some(pool) = pool else {
        print_hint(&format!(
            "Database is not reachable; install later with `beampipe profile add` ({})",
            profile.name
        ));
        return Ok(());
    };
    let row = crate::operator::install_profile(pool, profile).await?;
    println!(
        "Installed deployment profile '{}' revision {}.",
        row.name, row.revision
    );
    Ok(())
}

fn enable_live_backends(
    root: &Path,
    env_path: &Path,
    runtime: RuntimeKind,
    started: bool,
    profile: Option<&DeploymentProfile>,
    use_real_backends: &mut String,
) -> Result<()> {
    if use_real_backends == "true" {
        print_hint("Live backends are already enabled.");
        return Ok(());
    }
    print_hint("Workers will submit to real TM/DIM or Slurm instead of completing locally.");
    if profile.is_none() {
        print_hint(
            "No profile is loaded yet. Run `beampipe doctor --profile NAME` before enabling live submission.",
        );
    } else {
        print_hint("Do this only after `beampipe doctor --profile NAME` is clean.");
    }
    if !prompt_yes_no("Set BEAMPIPE_USE_REAL_BACKENDS=true?", false)? {
        return Ok(());
    }
    update_env_file(env_path, "BEAMPIPE_USE_REAL_BACKENDS", "true")?;
    std::env::set_var("BEAMPIPE_USE_REAL_BACKENDS", "true");
    *use_real_backends = "true".into();
    println!("Wrote BEAMPIPE_USE_REAL_BACKENDS=true");
    restart_stack_if_needed(root, runtime, started, "the new setting")?;
    Ok(())
}

fn restart_stack_if_needed(
    root: &Path,
    runtime: RuntimeKind,
    started: bool,
    what: &str,
) -> Result<()> {
    if started && runtime == RuntimeKind::Docker {
        let context = installation::InstallationContext::from_home(root.to_path_buf())?;
        match runtime::restart(&context) {
            Ok(()) => println!("Recreated API, scheduler, and worker so they load {what}."),
            Err(error) => print_hint(&format!(
                "Could not recreate the stack ({error}). Run `beampipe restart`."
            )),
        }
    } else if runtime == RuntimeKind::Docker {
        print_hint("When the stack is up: beampipe restart");
    } else {
        print_hint("Restart the host `beampipe start` process to load the new setting.");
    }
    Ok(())
}

fn next_action_slurm_credentials(
    opts: &SetupOptions,
    runtime: RuntimeKind,
    profile: Option<&mut DeploymentProfile>,
) -> Result<()> {
    if let Some(profile) = profile {
        if let DeploymentConfig::SlurmRemote(slurm) = &mut profile.deployment {
            return configure_slurm_credential_interactive(slurm, &profile.name, runtime);
        }
    }
    print_hint(
        "No Slurm profile is loaded. This only creates a managed SSH slot; add a Slurm profile afterwards.",
    );
    standalone_slurm_credentials(opts, runtime)
}

fn standalone_slurm_credentials(opts: &SetupOptions, runtime: RuntimeKind) -> Result<()> {
    let choices = [
        ChoiceItem {
            key: "generate",
            label: "Generate a new Beampipe key",
            hint: "you must still install the public key on the login node",
        },
        ChoiceItem {
            key: "import",
            label: "Import an existing key",
            hint: "skip upload if the cluster already has this public key",
        },
        ChoiceItem {
            key: "later",
            label: "Configure later",
            hint: "leave credentials unchanged",
        },
    ];
    let choice = prompt_choice("SSH credentials", &choices, 2)?;
    if choice == 2 {
        return Ok(());
    }
    let slot = prompt_default("SSH credential slot", "hpc")?;
    beampipe_profiles::validate_ssh_credential_name(&slot)?;
    let host = prompt_nonempty("Login node", "")?;
    let acl = opts.ssh_acl || (runtime == RuntimeKind::Docker && cfg!(target_os = "linux"));
    if choice == 1 {
        let private_key =
            PathBuf::from(prompt_default("Existing private key", "~/.ssh/id_ed25519")?);
        let private_key = expand_home_path(&private_key)?;
        let imported = crate::slurm_credentials::import(crate::slurm_credentials::ImportOptions {
            slot,
            dir: None,
            private_key,
            public_key: None,
            known_hosts: None,
            passphrase_file: None,
            host: Some(host),
            port: 22,
            acl,
            force: false,
            accept_host_key: false,
        })?;
        crate::slurm_credentials::print_init_next_steps(&imported);
    } else {
        let generated = crate::slurm_credentials::init(crate::slurm_credentials::InitOptions {
            slot,
            host,
            port: 22,
            acl,
            ..crate::slurm_credentials::InitOptions::default()
        })?;
        crate::slurm_credentials::print_init_next_steps(&generated);
    }
    Ok(())
}

fn casda_password_file_path(root: &Path) -> PathBuf {
    root.join("credentials/casda/password")
}

fn env_file_encode(value: &str) -> Result<String> {
    if value.contains(['\n', '\r']) {
        bail!("environment variable value must be a single line");
    }
    let needs_quotes = value
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '#' | '"' | '\''));
    if !needs_quotes {
        return Ok(value.to_string());
    }
    if value.contains(['"', '\\']) {
        bail!("value cannot be written to .env because it contains a quote or backslash");
    }
    Ok(format!("\"{value}\""))
}

fn apply_casda_process_env(runtime: RuntimeKind, username: &str, password: &str, file_path: &Path) {
    std::env::set_var("CASDA_USERNAME", username);
    match runtime {
        RuntimeKind::Docker => {
            std::env::set_var("CASDA_PASSWORD", password);
            std::env::remove_var("CASDA_PASSWORD_FILE");
        }
        RuntimeKind::Host => {
            std::env::set_var("CASDA_PASSWORD_FILE", file_path.display().to_string());
            std::env::remove_var("CASDA_PASSWORD");
        }
    }
}

fn write_casda_credentials(
    root: &Path,
    env_path: &Path,
    runtime: RuntimeKind,
    username: &str,
    password: &str,
) -> Result<PathBuf> {
    let username = username.trim();
    if username.is_empty() {
        bail!("CASDA username cannot be empty");
    }
    if password.is_empty() {
        bail!("CASDA password cannot be empty");
    }
    let file_path = casda_password_file_path(root);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&file_path, format!("{password}\n"))
        .with_context(|| format!("write {}", file_path.display()))?;
    set_private_file_permissions(&file_path)?;

    update_env_file(env_path, "CASDA_USERNAME", username)?;
    match runtime {
        RuntimeKind::Docker => {
            // Compose env_file cannot see a host CASDA_PASSWORD_FILE path.
            update_env_file(env_path, "CASDA_PASSWORD", &env_file_encode(password)?)?;
            update_env_file(env_path, "CASDA_PASSWORD_FILE", "")?;
        }
        RuntimeKind::Host => {
            update_env_file(
                env_path,
                "CASDA_PASSWORD_FILE",
                &file_path.display().to_string(),
            )?;
            update_env_file(env_path, "CASDA_PASSWORD", "")?;
        }
    }
    Ok(file_path)
}

fn next_action_casda_credentials(
    root: &Path,
    env_path: &Path,
    runtime: RuntimeKind,
    started: bool,
) -> Result<()> {
    print_hint("Staging downloads need a CSIRO CASDA username and password.");
    print_hint("Discovery TAP stays public. The password is not printed.");
    if runtime == RuntimeKind::Docker {
        print_hint(
            "Docker workers read CASDA_PASSWORD from .env; a host password-file path is not visible in the container.",
        );
    } else {
        print_hint(
            "Host runtime uses CASDA_PASSWORD_FILE pointing at a private file under the install home.",
        );
    }

    let existing_user = env_file_value(env_path, "CASDA_USERNAME").unwrap_or_default();
    if !existing_user.is_empty() {
        print_hint(&format!("CASDA_USERNAME is already set ({existing_user})."));
        if !prompt_yes_no("Replace CASDA credentials?", false)? {
            return Ok(());
        }
    }

    let username = prompt_default("CASDA username", &existing_user)?;
    let username = username.trim().to_string();
    if username.is_empty() {
        print_hint("Skipped CASDA credentials (empty username).");
        return Ok(());
    }

    let password_file = prompt_default("Existing password file (empty to type)", "")?;
    let password = if password_file.trim().is_empty() {
        let first = rpassword::prompt_password("CASDA password: ")?;
        if first.is_empty() {
            bail!("CASDA password cannot be empty");
        }
        let second = rpassword::prompt_password("Confirm CASDA password: ")?;
        if first != second {
            bail!("CASDA passwords do not match");
        }
        first
    } else {
        let path = expand_home_path(&PathBuf::from(password_file.trim()))?;
        read_secret_file(&path, "CASDA password")?
    };

    let stored = write_casda_credentials(root, env_path, runtime, &username, &password)?;
    apply_casda_process_env(runtime, &username, &password, &stored);
    println!("Wrote CASDA username '{username}'.");
    println!("Password file: {}", stored.display());
    restart_stack_if_needed(root, runtime, started, "CASDA credentials")?;
    Ok(())
}

fn prompt_nonempty(label: &str, default: &str) -> Result<String> {
    loop {
        let value = prompt_default(label, default)?;
        if !value.trim().is_empty() {
            return Ok(value.trim().to_string());
        }
        print_hint("This value is required.");
    }
}

async fn next_action_doctor_profile(
    root: &Path,
    pool: Option<&PgPool>,
    prepared_profile: Option<&DeploymentProfile>,
) -> Result<()> {
    let Some(pool) = pool else {
        print_hint("Database is not reachable; run `beampipe doctor --profile NAME` later.");
        return Ok(());
    };
    let profiles = repo::list_deployment_profiles(pool, None, 500, 0).await?;
    if profiles.is_empty() {
        print_hint("No deployment profiles are installed yet. Add one first.");
        return Ok(());
    }
    print_hint("Installed profiles:");
    for profile in &profiles {
        print_hint(&profile.name);
    }
    let default = prepared_profile
        .map(|profile| profile.name.clone())
        .unwrap_or_else(|| profiles[0].name.clone());
    let name = prompt_default("Profile name", &default)?;
    if name.trim().is_empty() {
        return Ok(());
    }
    let settings = Settings::load()?.settings;
    let context = installation::InstallationContext::from_home(root.to_path_buf())?;
    let report = doctor::run_doctor(
        pool,
        &settings,
        Some(name.trim()),
        Vec::new(),
        Some(&context),
    )
    .await;
    doctor::print_human(&report);
    Ok(())
}

fn next_steps_lines(steps: &SetupNextSteps) -> Vec<String> {
    let mut lines = Vec::new();
    if steps.compose_postgres && !steps.runtime_docker {
        lines.push("  docker compose up -d postgres".into());
    }
    if steps.runtime_docker {
        if let Some(context) = &steps.docker_context {
            lines.push(format!("  # docker context: {context}"));
        }
        lines.push("  beampipe start".into());
        if !steps.db_applied {
            lines.push("  beampipe migrate".into());
        }
        if !steps.admin_ready {
            lines.push("  beampipe admin create-user \\".into());
            lines.push("    --username admin --email admin@example.test \\".into());
            lines.push("    --password-file /path/to/protected/admin-password --superuser".into());
        }
        if !steps.db_applied {
            if let Some(project) = &steps.project_file {
                lines.push(format!("  beampipe project add -f {project}"));
            }
            if let Some(profile) = &steps.profile_file {
                lines.push(format!("  beampipe profile add -f {profile}"));
            }
        }
        if let Some(dash_dir) = &steps.dash_dir {
            lines.push(dash_install_recipe_line(
                steps.core_home.as_deref(),
                dash_dir,
            ));
        }
    } else {
        if !steps.db_applied {
            lines.push("  beampipe migrate".into());
        }
        if !steps.admin_ready {
            lines.push("  beampipe admin create-user \\".into());
            lines.push("    --username admin --email admin@example.test \\".into());
            lines.push("    --password-file /path/to/protected/admin-password --superuser".into());
        }
        if !steps.db_applied {
            if let Some(project) = &steps.project_file {
                lines.push(format!("  beampipe project add -f {project}"));
            }
            if let Some(profile) = &steps.profile_file {
                lines.push(format!("  beampipe profile add -f {profile}"));
            }
        }
        lines.push("  beampipe start".into());
    }
    lines
}

const DEFAULT_DASH_PORT: u16 = 3000;

fn dashboard_listen_url(dash_dir: Option<&Path>) -> Option<String> {
    let dir = dash_dir?;
    if !dir.exists() {
        return None;
    }
    let port = env_file_value(&dir.join(".env"), "BEAMPIPE_DASH_PORT")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_DASH_PORT);
    Some(format!("http://127.0.0.1:{port}"))
}

fn collect_role_counts(
    root: &Path,
    runtime: RuntimeKind,
    started: bool,
    api_port: u16,
) -> runtime::RoleCounts {
    if !started {
        return runtime::RoleCounts::default();
    }
    match runtime {
        RuntimeKind::Docker => {
            match installation::InstallationContext::from_home(root.to_path_buf()) {
                Ok(context) => runtime::running_role_counts(&context),
                Err(_) => runtime::RoleCounts::default(),
            }
        }
        RuntimeKind::Host => {
            if port_in_use(api_port) {
                runtime::RoleCounts {
                    api: 1,
                    scheduler: 1,
                    worker: 1,
                }
            } else {
                runtime::RoleCounts::default()
            }
        }
    }
}

fn access_summary_lines(
    api_port: u16,
    dashboard: Option<&str>,
    counts: runtime::RoleCounts,
    started: bool,
) -> Vec<String> {
    let api = format!("http://127.0.0.1:{api_port}/api/v2");
    let docs = format!("{api}/docs");
    let dashboard = dashboard
        .map(str::to_string)
        .unwrap_or_else(|| "not installed".into());
    let status = if started && (counts.api + counts.scheduler + counts.worker) > 0 {
        format!(
            "Beampipe is now up with {} scheduler, {} worker, {} API",
            counts.scheduler, counts.worker, counts.api
        )
    } else if started {
        format!(
            "Beampipe start was requested ({} scheduler, {} worker, {} API)",
            counts.scheduler, counts.worker, counts.api
        )
    } else {
        format!(
            "Beampipe is not started ({} scheduler, {} worker, {} API)",
            counts.scheduler, counts.worker, counts.api
        )
    };
    vec![
        String::new(),
        "Summary".into(),
        format!("  API available here:       {api}"),
        format!("  API docs here:            {docs}"),
        format!("  Dashboard available here: {dashboard}"),
        format!("  {status}"),
    ]
}

fn print_access_summary(
    root: &Path,
    runtime: RuntimeKind,
    ports: HostPorts,
    started: bool,
    dash_dir: Option<&Path>,
) {
    let counts = collect_role_counts(root, runtime, started, ports.api);
    let dashboard = dashboard_listen_url(dash_dir);
    for line in access_summary_lines(ports.api, dashboard.as_deref(), counts, started) {
        println!("{line}");
    }
}

fn print_setup_summary(
    root: &Path,
    runtime: RuntimeKind,
    postgres: PostgresKind,
    ports: HostPorts,
    started: bool,
    profile: Option<&DeploymentProfile>,
    use_real_backends: &str,
) {
    println!("\nBeampipe setup complete");
    println!("  [OK] Home: {}", root.display());
    println!("  [OK] Runtime: {}", runtime.as_str());
    println!("  [OK] Database: {}", postgres.as_str());
    println!("  [OK] API: http://127.0.0.1:{}/api/v2", ports.api);
    println!(
        "  [{}] Services: {}",
        if started { "OK" } else { "--" },
        if started {
            "start requested"
        } else {
            "not started"
        }
    );
    println!(
        "  [{}] Live backends: {}",
        if use_real_backends == "true" {
            "OK"
        } else {
            "--"
        },
        if use_real_backends == "true" {
            "BEAMPIPE_USE_REAL_BACKENDS=true"
        } else {
            "mock (enable after doctor --profile)"
        }
    );
    if let Some(profile) = profile {
        println!("  [OK] Profile: {}", profile.name);
        if let DeploymentConfig::SlurmRemote(slurm) = &profile.deployment {
            println!(
                "  [{}] SSH slot: {}",
                if slurm.ssh_credential.is_some() {
                    "OK"
                } else {
                    "--"
                },
                slurm.ssh_credential.as_deref().unwrap_or("configure later")
            );
        }
    } else {
        println!("  [--] Profile: configure later with `beampipe profile add`");
    }
    println!("\nUseful commands:");
    println!("  beampipe status");
    println!("  beampipe doctor");
    println!("  beampipe logs --follow");
    println!("  beampipe profile list");
    if let Some(binary) = default_release_binary() {
        println!("  Binary: {}", binary.display());
    }
    println!("  If `beampipe` is not found: export PATH=\"$HOME/.local/bin:$PATH\"");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_items() -> [ChoiceItem; 2] {
        runtime_choices()
    }

    fn postgres_items() -> [ChoiceItem; 2] {
        postgres_choices()
    }

    #[test]
    fn parse_choice_accepts_index_key_and_empty_default() {
        let items = runtime_items();
        assert_eq!(parse_choice("", &items, 0), Some(0));
        assert_eq!(parse_choice("1", &items, 1), Some(0));
        assert_eq!(parse_choice("2", &items, 0), Some(1));
        assert_eq!(parse_choice("docker", &items, 1), Some(0));
        assert_eq!(parse_choice("HOST", &items, 0), Some(1));
        assert_eq!(parse_choice("3", &items, 0), None);
        assert_eq!(parse_choice("slurm", &items, 0), None);

        let postgres = postgres_items();
        assert_eq!(parse_choice("", &postgres, 0), Some(0));
        assert_eq!(parse_choice("compose", &postgres, 1), Some(0));
        assert_eq!(parse_choice("existing", &postgres, 0), Some(1));

        let next = next_action_choices();
        assert_eq!(parse_choice("", &next, 5), Some(5));
        assert_eq!(parse_choice("done", &next, 0), Some(5));
        assert_eq!(parse_choice("live", &next, 5), Some(0));
        assert_eq!(parse_choice("profile", &next, 5), Some(1));
        assert_eq!(parse_choice("slurm", &next, 5), Some(2));
        assert_eq!(parse_choice("casda", &next, 5), Some(3));
        assert_eq!(parse_choice("doctor", &next, 5), Some(4));
        assert_eq!(parse_choice("5", &next, 5), Some(4));
        assert_eq!(parse_choice("6", &next, 5), Some(5));
    }

    #[test]
    fn resolve_use_real_backends_prefers_flag_then_env_then_file() {
        assert_eq!(
            resolve_use_real_backends(true, Some("false"), Some("false")),
            "true"
        );
        assert_eq!(
            resolve_use_real_backends(false, Some("true"), Some("false")),
            "true"
        );
        assert_eq!(
            resolve_use_real_backends(false, Some("0"), Some("true")),
            "false"
        );
        assert_eq!(resolve_use_real_backends(false, None, Some("yes")), "true");
        assert_eq!(resolve_use_real_backends(false, None, None), "false");
        assert_eq!(
            resolve_use_real_backends(false, Some("maybe"), Some("no")),
            "false"
        );
    }

    #[test]
    fn next_action_recipe_mentions_live_backends_and_slurm() {
        let root = Path::new("/home/op/beampipe");
        let mock = next_action_recipe_lines(root, false).join("\n");
        assert!(mock.contains("BEAMPIPE_USE_REAL_BACKENDS=true"));
        assert!(mock.contains("beampipe restart"));
        assert!(mock.contains("deployment_profile.dlg-dim.json"));
        assert!(mock.contains("beampipe doctor --profile dlg-dim"));
        assert!(mock.contains("beampipe slurm credentials init"));
        assert!(mock.contains("deployment_profile.slurm-remote.json"));
        assert!(mock.contains("CASDA_USERNAME"));
        assert!(mock.contains("CASDA_PASSWORD_FILE"));
        assert!(!mock.contains("Live backends are on"));

        let live = next_action_recipe_lines(root, true).join("\n");
        assert!(live.contains("Live backends are on"));
        assert!(!live.contains("set BEAMPIPE_USE_REAL_BACKENDS=true"));
    }

    #[test]
    fn write_casda_credentials_uses_env_password_for_docker() {
        let root = tempfile::tempdir().unwrap();
        let env_path = root.path().join(".env");
        std::fs::write(&env_path, "CASDA_PASSWORD_FILE=/old/host/path\n").unwrap();
        let password = "unit-test-casda-secret";
        let file_path = write_casda_credentials(
            root.path(),
            &env_path,
            RuntimeKind::Docker,
            "casda.user@example.test",
            password,
        )
        .unwrap();

        let env = std::fs::read_to_string(&env_path).unwrap();
        assert!(env.contains("CASDA_USERNAME=casda.user@example.test\n"));
        assert!(env.contains(&format!("CASDA_PASSWORD={password}\n")));
        assert!(env.contains("CASDA_PASSWORD_FILE=\n"));
        assert!(!env.contains("/old/host/path"));
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            format!("{password}\n")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn write_casda_credentials_uses_password_file_for_host() {
        let root = tempfile::tempdir().unwrap();
        let env_path = root.path().join(".env");
        std::fs::write(&env_path, "CASDA_PASSWORD=old-inline-password\n").unwrap();
        let password = "unit-test-casda-secret";
        let file_path = write_casda_credentials(
            root.path(),
            &env_path,
            RuntimeKind::Host,
            "casda.user@example.test",
            password,
        )
        .unwrap();

        let env = std::fs::read_to_string(&env_path).unwrap();
        assert!(env.contains("CASDA_USERNAME=casda.user@example.test\n"));
        assert!(env.contains("CASDA_PASSWORD=\n"));
        assert!(env.contains(&format!("CASDA_PASSWORD_FILE={}\n", file_path.display())));
        assert!(!env.contains("old-inline-password"));
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            format!("{password}\n")
        );
    }

    #[test]
    fn env_file_encode_quotes_hash_and_rejects_quotes() {
        assert_eq!(env_file_encode("plain").unwrap(), "plain");
        assert_eq!(env_file_encode("hash#value").unwrap(), "\"hash#value\"");
        assert!(env_file_encode("has\"quote").is_err());
    }

    #[test]
    fn next_actions_are_printed_not_prompted_with_yes() {
        let opts = SetupOptions {
            yes: true,
            ..Default::default()
        };
        assert!(!next_actions_should_prompt(&opts));
    }

    #[test]
    fn access_summary_lists_api_docs_dashboard_and_role_counts() {
        let lines = access_summary_lines(
            18080,
            Some("http://127.0.0.1:3000"),
            runtime::RoleCounts {
                api: 1,
                scheduler: 1,
                worker: 2,
            },
            true,
        );
        let joined = lines.join("\n");
        assert!(joined.contains("API available here:       http://127.0.0.1:18080/api/v2"));
        assert!(joined.contains("API docs here:            http://127.0.0.1:18080/api/v2/docs"));
        assert!(joined.contains("Dashboard available here: http://127.0.0.1:3000"));
        assert!(joined.contains("Beampipe is now up with 1 scheduler, 2 worker, 1 API"));
    }

    #[test]
    fn access_summary_without_dashboard_or_start() {
        let lines = access_summary_lines(18080, None, runtime::RoleCounts::default(), false);
        let joined = lines.join("\n");
        assert!(joined.contains("Dashboard available here: not installed"));
        assert!(joined.contains("Beampipe is not started (0 scheduler, 0 worker, 0 API)"));
    }

    #[test]
    fn dashboard_listen_url_reads_dash_env_port() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(dashboard_listen_url(None), None);
        assert_eq!(
            dashboard_listen_url(Some(&root.path().join("missing"))),
            None
        );
        assert_eq!(
            dashboard_listen_url(Some(root.path())).as_deref(),
            Some("http://127.0.0.1:3000")
        );
        std::fs::write(root.path().join(".env"), "BEAMPIPE_DASH_PORT=3100\n").unwrap();
        assert_eq!(
            dashboard_listen_url(Some(root.path())).as_deref(),
            Some("http://127.0.0.1:3100")
        );
    }

    #[test]
    fn format_step_is_numbered() {
        assert_eq!(
            format_step(1, 4, "How will you run Beampipe?"),
            "== 1/4  How will you run Beampipe? =="
        );
    }

    #[test]
    fn setup_step_total_skips_dashboard_when_it_cannot_apply() {
        let yes_docker = SetupOptions {
            yes: true,
            runtime: Some("docker".into()),
            ..Default::default()
        };
        assert_eq!(setup_step_total(&yes_docker, true), 4);

        let host = SetupOptions {
            yes: true,
            runtime: Some("host".into()),
            ..Default::default()
        };
        assert_eq!(setup_step_total(&host, false), 4);

        let with_dash = SetupOptions {
            yes: true,
            dashboard: true,
            ..Default::default()
        };
        assert_eq!(setup_step_total(&with_dash, true), 5);

        let interactive = SetupOptions::default();
        assert_eq!(setup_step_total(&interactive, true), 5);
    }

    #[test]
    fn host_start_command_selects_installation_without_chdir() {
        let command = host_start_command(Path::new("/home/op/beampipe"));
        assert_eq!(command, "  beampipe --home /home/op/beampipe start");
    }

    #[test]
    fn login_snippet_reads_password_from_the_environment() {
        let joined = login_snippet_lines("admin", installation::DEFAULT_API_PORT).join("\n");
        assert!(joined.contains("export ADMIN_USER=admin"));
        assert!(joined.contains("ADMIN_PASSWORD:?set to the password setup printed"));
        assert!(joined.contains("http://127.0.0.1:18080/api/v2/login"));
        assert!(joined.contains("/api/v2/login"));
        assert!(!joined.contains("replace-this-local-password"));
    }

    #[test]
    fn require_bind_ports_free_names_the_busy_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let error = require_bind_ports_free(&[(port, "API")]).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("API"));
        assert!(message.contains(&port.to_string()));
        assert!(message.contains("--no-start"));
        assert!(message.contains("down --volumes"));
    }

    #[test]
    fn compose_postgres_volume_name_uses_install_home() {
        assert_eq!(
            compose_postgres_volume_name(Path::new("/home/jack/beampipe")),
            "beampipe_beampipe_pgdata"
        );
    }

    #[test]
    fn database_error_is_password_auth_matches_postgres() {
        assert!(database_error_is_password_auth(
            &"error returned from database: password authentication failed for user \"postgres\""
        ));
        assert!(!database_error_is_password_auth(&"connection refused"));
    }

    #[test]
    fn recipe_lines_start_with_print_only_header() {
        let commands = vec!["  docker compose up -d postgres".into()];
        let lines = recipe_lines(&commands);
        assert_eq!(lines[0], "");
        assert_eq!(lines[1], "Next (nothing was started)");
        assert_eq!(lines[2], "  docker compose up -d postgres");
    }

    #[test]
    fn env_updates_are_single_line_and_private() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".env");
        std::fs::write(&path, "DATABASE_URL=old\nUNCHANGED=value\n").unwrap();

        update_env_file(&path, "DATABASE_URL", "postgres://localhost/beampipe").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("DATABASE_URL=postgres://localhost/beampipe\n"));
        assert!(content.contains("UNCHANGED=value\n"));
        assert!(update_env_file(&path, "INJECTED", "value\nSECOND=value").is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn seed_env_prefers_example_and_fills_missing_version() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".env.example"),
            "BEAMPIPE_VERSION=0.2.0\nBEAMPIPE_ENV=development\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join(".env.template"),
            "BEAMPIPE_ENV=development\n",
        )
        .unwrap();

        let env_path = root.path().join(".env");
        seed_env_file(root.path(), &env_path).unwrap();
        ensure_beampipe_version(root.path(), &env_path).unwrap();
        let content = std::fs::read_to_string(&env_path).unwrap();
        assert!(content.contains("BEAMPIPE_VERSION=0.2.0\n"));
        assert!(content.contains("BEAMPIPE_ENV=development\n"));

        let empty = tempfile::tempdir().unwrap();
        let created = empty.path().join(".env");
        std::fs::write(&created, "BEAMPIPE_ENV=development\n").unwrap();
        ensure_beampipe_version(empty.path(), &created).unwrap();
        assert!(std::fs::read_to_string(&created)
            .unwrap()
            .contains(&format!("BEAMPIPE_VERSION={}\n", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn generated_admin_password_is_long_enough() {
        let password = generate_admin_password();
        assert!(password.len() >= 12);
        assert!(password.starts_with("bp-"));
    }

    #[test]
    fn admin_password_rejects_short_secrets() {
        assert!(validate_admin_password("short").is_err());
        assert!(validate_admin_password("12345678901").is_err());
        assert!(validate_admin_password("123456789012").is_ok());
    }

    #[test]
    fn resolve_under_root_joins_relative_profile_paths() {
        let root = Path::new("/home/op/beampipe");
        assert_eq!(
            resolve_under_root(root, "config/deployment_profile.dlg-dim.json"),
            PathBuf::from("/home/op/beampipe/config/deployment_profile.dlg-dim.json")
        );
        assert_eq!(
            resolve_under_root(root, "/tmp/custom.json"),
            PathBuf::from("/tmp/custom.json")
        );
    }

    #[test]
    fn project_config_path_defaults_to_installation_home() {
        let root = Path::new("/home/op/beampipe");
        assert_eq!(
            project_config_path(root, &SetupOptions::default()),
            PathBuf::from("/home/op/beampipe/config/wallaby_hires.v2.yaml")
        );
    }

    #[test]
    fn setup_preserves_an_existing_valid_jwt_secret() {
        let existing = "existing-production-secret-with-more-than-32-characters";
        assert_eq!(
            select_jwt_secret(None, Some(existing), true).unwrap(),
            existing
        );
    }

    #[test]
    fn setup_requires_explicit_replacement_for_a_weak_existing_jwt_secret() {
        let error = select_jwt_secret(None, Some("change-me"), true).unwrap_err();
        assert!(error.to_string().contains("will not rotate it silently"));
    }

    #[test]
    fn explicit_jwt_secret_replaces_an_existing_placeholder() {
        let replacement = "new-production-secret-with-more-than-32-characters";
        assert_eq!(
            select_jwt_secret(Some(replacement), Some("change-me"), true).unwrap(),
            replacement
        );
    }

    #[test]
    fn known_long_jwt_placeholders_are_rejected() {
        let error = select_jwt_secret(
            Some("replace-with-at-least-32-random-characters"),
            None,
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("known placeholder"));
    }

    #[test]
    fn yes_requires_an_explicit_runtime() {
        let error = decide_runtime(&SetupOptions {
            yes: true,
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("--yes requires --runtime"));
    }

    #[test]
    fn yes_with_compose_prepares_docker_env_and_skips_dash() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("docker-compose.yml"), "services: {}\n").unwrap();
        let env = root.path().join(".env");
        std::fs::write(&env, "BEAMPIPE_JWT_SECRET=x\n").unwrap();

        let opts = SetupOptions {
            yes: true,
            runtime: Some("docker".into()),
            ..Default::default()
        };
        assert_eq!(decide_runtime(&opts).unwrap(), Some(RuntimeKind::Docker));
        assert_eq!(
            decide_postgres(&opts, true).unwrap(),
            Some(PostgresKind::Compose)
        );
        assert_eq!(decide_dashboard(&opts, true), Some(false));

        prepare_docker_env(root.path(), &env).unwrap();
        let content = std::fs::read_to_string(&env).unwrap();
        assert!(content.contains(&format!(
            "BEAMPIPE_SSH_CREDENTIALS_HOST={}",
            root.path().join("credentials/ssh").display()
        )));
        assert!(!content.contains("BEAMPIPE_SSH_CREDENTIALS_DIR=/"));
    }

    #[test]
    fn yes_dashboard_prepares_existing_dash_checkout() {
        let workspace = tempfile::tempdir().unwrap();
        let core = workspace.path().join("operator-core");
        let dash = workspace.path().join("beampipe-dash");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::create_dir_all(&dash).unwrap();
        std::fs::write(
            dash.join(".env.example"),
            "BEAMPIPE_API_URL=http://127.0.0.1:8080\n",
        )
        .unwrap();

        let opts = SetupOptions {
            yes: true,
            dashboard: true,
            dash_dir: Some(dash.clone()),
            ..Default::default()
        };
        assert_eq!(decide_dashboard(&opts, true), Some(true));

        let prepared = prepare_dashboard(&core, &opts, &compose_network_name(&core)).unwrap();
        assert_eq!(prepared, dash);
        assert!(dash.join(".env").exists());
        let override_contents =
            std::fs::read_to_string(dash.join("compose.beampipe-local.yml")).unwrap();
        assert!(override_contents.contains("BEAMPIPE_API_URL: http://api:8080"));
        assert!(override_contents.contains("127.0.0.1:3000:3000"));
        assert!(override_contents.contains("name: operator-core_default"));
        assert!(!override_contents.contains("private_key"));
        assert!(!override_contents.contains("passphrase"));
    }

    #[test]
    fn yes_dashboard_prefers_install_script_when_present() {
        let workspace = tempfile::tempdir().unwrap();
        let core = workspace.path().join("operator-core");
        let dash = workspace.path().join("beampipe-dash");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::create_dir_all(dash.join("scripts")).unwrap();
        std::fs::write(
            dash.join("scripts/install.sh"),
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/../install-args\"\n",
        )
        .unwrap();

        let opts = SetupOptions {
            yes: true,
            dashboard: true,
            dash_dir: Some(dash.clone()),
            ..Default::default()
        };
        prepare_dashboard(&core, &opts, &compose_network_name(&core)).unwrap();
        let args = std::fs::read_to_string(dash.join("install-args")).unwrap();
        assert!(args.contains("--core-home"));
        assert!(args.contains(&core.display().to_string()));
        assert!(args.contains("--dash-dir"));
        assert!(args.contains(&dash.display().to_string()));
        assert!(args.contains("--no-start"));
        assert!(args.contains("--yes"));
        assert!(!dash.join("compose.beampipe-local.yml").exists());
    }

    #[test]
    fn try_start_dashboard_runs_install_script() {
        let workspace = tempfile::tempdir().unwrap();
        let core = workspace.path().join("operator-core");
        let dash = workspace.path().join("beampipe-dash");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::create_dir_all(dash.join("scripts")).unwrap();
        std::fs::write(
            dash.join("scripts/install.sh"),
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/../start-args\"\n",
        )
        .unwrap();

        try_start_dashboard(&core, &dash).unwrap();
        let args = std::fs::read_to_string(dash.join("start-args")).unwrap();
        assert!(args.contains("--core-home"));
        assert!(args.contains("--dash-dir"));
        assert!(args.contains("--yes"));
        assert!(!args.contains("--no-start"));
    }

    #[test]
    fn dash_override_patches_existing_network_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compose.beampipe-local.yml");
        std::fs::write(
            &path,
            "networks:\n  beampipe-core:\n    external: true\n    name: old_default\n",
        )
        .unwrap();
        write_or_patch_dash_override(&path, "operator-core_default").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("name: operator-core_default"));
        assert!(!contents.contains("old_default"));
    }

    #[test]
    fn yes_host_existing_postgres_is_explicit() {
        let opts = SetupOptions {
            yes: true,
            runtime: Some("host".into()),
            postgres: Some("existing".into()),
            ..Default::default()
        };
        assert_eq!(decide_runtime(&opts).unwrap(), Some(RuntimeKind::Host));
        assert_eq!(
            decide_postgres(&opts, true).unwrap(),
            Some(PostgresKind::Existing)
        );
    }

    #[test]
    fn yes_skip_docker_selects_host_runtime() {
        let opts = SetupOptions {
            yes: true,
            skip_docker: true,
            ..Default::default()
        };
        assert_eq!(decide_runtime(&opts).unwrap(), Some(RuntimeKind::Host));
        assert_eq!(decide_dashboard(&opts, false), Some(false));
    }

    #[test]
    fn host_recipe_starts_with_compose_postgres_and_beampipe_start() {
        let lines = next_steps_lines(&SetupNextSteps {
            runtime_docker: false,
            compose_postgres: true,
            db_applied: false,
            admin_ready: false,
            project_file: Some("config/wallaby_hires.v2.yaml".into()),
            ..Default::default()
        });
        let joined = lines.join("\n");
        assert!(joined.contains("docker compose up -d postgres"));
        assert!(joined.contains("beampipe migrate"));
        assert!(joined.contains("beampipe project add -f config/wallaby_hires.v2.yaml"));
        assert!(joined.contains("beampipe start"));
        assert!(!joined.contains("profile add"));
        assert!(!joined.contains("docker compose up -d api"));
        assert!(!joined.contains("--deployment"));
    }

    #[test]
    fn host_existing_postgres_omits_compose_up() {
        let lines = next_steps_lines(&SetupNextSteps {
            runtime_docker: false,
            compose_postgres: false,
            db_applied: true,
            admin_ready: true,
            ..Default::default()
        });
        let joined = lines.join("\n");
        assert!(!joined.contains("docker compose up -d postgres"));
        assert!(joined.contains("beampipe start"));
        assert!(!joined.contains("profile add"));
    }

    #[test]
    fn docker_recipe_starts_dash_via_install_script() {
        let lines = next_steps_lines(&SetupNextSteps {
            runtime_docker: true,
            compose_postgres: true,
            db_applied: true,
            admin_ready: true,
            core_home: Some(PathBuf::from("/home/op/beampipe")),
            dash_dir: Some(PathBuf::from("/home/op/beampipe-dash")),
            ..Default::default()
        });
        let joined = lines.join("\n");
        assert!(joined.contains("beampipe start"));
        assert!(joined.contains("scripts/install.sh"));
        assert!(joined.contains("--core-home /home/op/beampipe"));
        assert!(joined.contains("--dash-dir /home/op/beampipe-dash"));
        assert!(!joined.contains("compose.beampipe-local.yml"));
        assert!(!joined.contains("docker compose -f compose.yaml"));
    }

    #[test]
    fn docker_recipe_uses_the_beampipe_lifecycle_facade() {
        let lines = next_steps_lines(&SetupNextSteps {
            runtime_docker: true,
            compose_postgres: true,
            db_applied: false,
            admin_ready: false,
            project_file: Some("config/wallaby_hires.v2.yaml".into()),
            ..Default::default()
        });
        let joined = lines.join("\n");
        assert!(joined.contains("beampipe start"));
        assert!(joined.contains("beampipe migrate"));
        assert!(joined.contains("beampipe project add -f config/wallaby_hires.v2.yaml"));
        assert!(!joined.contains("docker compose up"));
        assert!(!joined.contains("docker compose run"));
        assert!(!joined.contains("profile add"));
        assert!(!joined.contains("compose.beampipe-local.yml"));
        assert!(!joined.contains("--deployment"));
        assert!(!joined.contains("slurm_remote"));
        assert!(!joined.contains("docker compose build api"));
        assert!(!lines
            .iter()
            .any(|line| line.trim() == "docker compose up -d"));
    }

    #[test]
    fn dash_override_binds_localhost() {
        let contents = dash_override_contents("beampipe-core-v2_default");
        assert!(contents.contains("127.0.0.1:3000:3000"));
        assert!(!contents.contains("0.0.0.0:3000:3000"));
    }

    #[test]
    fn host_ports_default_to_operator_api_18080() {
        assert_eq!(
            port_from_sources(None, None, installation::DEFAULT_API_PORT, "API port").unwrap(),
            18080
        );
        assert_eq!(
            port_from_sources(
                None,
                None,
                installation::DEFAULT_POSTGRES_PORT,
                "PostgreSQL port"
            )
            .unwrap(),
            5432
        );
        assert_eq!(
            port_from_sources(
                None,
                None,
                installation::DEFAULT_METRICS_PORT,
                "metrics port"
            )
            .unwrap(),
            9090
        );
    }

    #[test]
    fn host_ports_prefer_flags_over_defaults() {
        let ports = resolved_host_ports(&SetupOptions {
            api_port: Some(18181),
            postgres_port: Some(15432),
            metrics_port: Some(19090),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(ports.api, 18181);
        assert_eq!(ports.postgres, 15432);
        assert_eq!(ports.metrics, 19090);
    }

    #[test]
    fn host_ports_reject_zero_and_collisions() {
        assert!(port_from_sources(Some(0), None, 18080, "API port").is_err());
        assert!(port_from_sources(None, Some("not-a-port"), 18080, "API port").is_err());
        assert_eq!(
            port_from_sources(None, Some("18100"), 18080, "API port").unwrap(),
            18100
        );
        let error = validate_host_ports(HostPorts {
            api: 18080,
            postgres: 18080,
            metrics: 9090,
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("API port and PostgreSQL port"));
    }

    #[test]
    fn compose_database_url_uses_the_postgres_port() {
        assert_eq!(
            compose_host_database_url("secret", 5433),
            "postgres://postgres:secret@localhost:5433/beampipe"
        );
    }

    #[test]
    fn bind_ports_for_start_uses_configured_api_port() {
        let ports = HostPorts {
            api: 18181,
            postgres: 15432,
            metrics: 19090,
        };
        assert_eq!(
            bind_ports_for_start(ports, RuntimeKind::Docker, PostgresKind::Compose),
            vec![(15432, "PostgreSQL"), (18181, "API"), (19090, "metrics"),]
        );
        assert_eq!(
            bind_ports_for_start(ports, RuntimeKind::Host, PostgresKind::Existing),
            vec![(18181, "API")]
        );
    }

    #[test]
    fn persist_host_ports_writes_env_and_host_bind_addrs() {
        let root = tempfile::tempdir().unwrap();
        let env = root.path().join(".env");
        std::fs::write(&env, "BEAMPIPE_ENV=development\n").unwrap();
        persist_host_ports(
            &env,
            HostPorts {
                api: 18181,
                postgres: 15432,
                metrics: 19090,
            },
            RuntimeKind::Host,
        )
        .unwrap();
        let content = std::fs::read_to_string(&env).unwrap();
        assert!(content.contains("BEAMPIPE_API_PORT=18181\n"));
        assert!(content.contains("BEAMPIPE_POSTGRES_PORT=15432\n"));
        assert!(content.contains("BEAMPIPE_METRICS_PORT=19090\n"));
        assert!(content.contains("BEAMPIPE_BIND_ADDR=127.0.0.1:18181\n"));
        assert!(content.contains("BEAMPIPE_METRICS_BIND_ADDR=127.0.0.1:19090\n"));
    }

    #[test]
    fn clear_missing_config_path_blanks_absent_yaml() {
        let root = tempfile::tempdir().unwrap();
        let env = root.path().join(".env");
        std::fs::write(&env, "BEAMPIPE_CONFIG=beampipe.yaml\n").unwrap();
        clear_missing_config_path(root.path(), &env).unwrap();
        assert!(std::fs::read_to_string(&env)
            .unwrap()
            .contains("BEAMPIPE_CONFIG=\n"));
    }

    #[test]
    fn setup_logo_is_embedded_block_art() {
        assert!(!SETUP_LOGO.trim().is_empty());
        assert!(SETUP_LOGO.contains('█'));
    }

    #[test]
    fn uninstall_removes_installation_home_and_keeps_siblings() {
        let parent = tempfile::tempdir().unwrap();
        let home = parent.path().join("beampipe");
        let sibling = parent.path().join("beampipe-dash");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(home.join(".env"), "BEAMPIPE_ENV=development\n").unwrap();
        std::fs::write(sibling.join("package.json"), "{}\n").unwrap();

        run_uninstall(UninstallOptions {
            yes: true,
            purge_binary: false,
            keep_volumes: false,
            directory: Some(home.clone()),
        })
        .unwrap();

        assert!(!home.exists());
        assert!(sibling.join("package.json").is_file());
    }

    #[test]
    fn uninstall_refuses_missing_installation() {
        let directory = tempfile::tempdir().unwrap();
        let error = run_uninstall(UninstallOptions {
            yes: true,
            directory: Some(directory.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("no Beampipe installation"));
    }

    #[test]
    fn assert_safe_to_delete_refuses_root_and_user_home() {
        let error = assert_safe_to_delete(Path::new("/"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing to delete"), "{error}");

        let Some(user_home) = std::env::var_os("HOME") else {
            return;
        };
        let user_home = PathBuf::from(user_home);
        if !user_home.is_absolute() || user_home.parent().is_none() {
            return;
        }
        let error = assert_safe_to_delete(&user_home).unwrap_err().to_string();
        assert!(error.contains("user home"), "{error}");
        if let Some(parent) = user_home.parent() {
            if parent.parent().is_some() {
                let error = assert_safe_to_delete(parent).unwrap_err().to_string();
                assert!(error.contains("parent of the user home"), "{error}");
            }
        }
    }
}

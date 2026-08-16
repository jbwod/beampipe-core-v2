use anyhow::{bail, Context, Result};
use beampipe_config::Settings;
use beampipe_db::repo;
use beampipe_project::ProjectConfig;
use crossterm::style::Stylize;
use sqlx::PgPool;
use std::io::{self, IsTerminal, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
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

#[derive(Debug, Clone, Default)]
pub struct SetupOptions {
    pub yes: bool,
    pub database_url: Option<String>,
    pub jwt_secret: Option<String>,
    pub admin_user: Option<String>,
    pub admin_password: Option<String>,
    pub admin_email: Option<String>,
    pub project_config: Option<PathBuf>,
    pub casda_tap_url: Option<String>,
    pub tm_url: Option<String>,
    pub worker_pool: Option<String>,
    pub skip_admin: bool,
    pub skip_upload: bool,
    pub docker: bool,
    pub skip_docker: bool,
    pub runtime: Option<String>,
    pub postgres: Option<String>,
    pub dashboard: bool,
    pub skip_dashboard: bool,
    pub dash_dir: Option<PathBuf>,
    pub dash_repo_url: Option<String>,
    pub directory: Option<PathBuf>,
    pub credentials_dir: Option<PathBuf>,
    pub start: bool,
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

fn print_banner(start: bool) {
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
    print_hint("Deployment profiles are configured later with beampipe profile add.");
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
        print_hint("Enter 1, 2, or the option name.");
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

    print_banner(opts.start);
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
    let database_url =
        if postgres == PostgresKind::Compose && opts.database_url.is_none() && !env_existed {
            format!(
                "postgres://postgres:{}@localhost:5432/beampipe",
                postgres_password.as_deref().unwrap_or_default()
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
    if postgres == PostgresKind::Compose {
        print_hint("Using the installation-managed PostgreSQL service.");
        if opts.start {
            print_hint("Starting Compose Postgres next.");
        } else {
            print_hint("Start it later with: docker compose up -d postgres");
        }
    }

    let prepare_docker = runtime == RuntimeKind::Docker;
    preflight_after_choices(&opts, runtime, postgres)?;

    let mut dash_dir = None;
    if decide_dashboard(&opts, prepare_docker) != Some(false) {
        print_step(step, total_steps, "Dashboard");
        step += 1;
        if resolve_prepare_dashboard(&opts, prepare_docker)? {
            match prepare_dashboard(&root, &opts, &compose_network_name(&root)) {
                Ok(prepared) => {
                    println!(
                        "Prepared Beampipe Dash at {} (not started).",
                        prepared.display()
                    );
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
    let use_real_backends = std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "false".into());

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
        require_docker_compose()?;
        if prepare_docker {
            compose_pull_api(&root)?;
        }
        compose_up_postgres(&root)?;
    }

    let pool = match beampipe_db::connect(&database_url).await {
        Ok(pool) => Some(pool),
        Err(error) if postgres == PostgresKind::Compose => {
            println!(
                "PostgreSQL is not reachable ({error}). Skipping migrate, admin, upload, and doctor."
            );
            println!("PostgreSQL is not up. Seed is in the recipe.");
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
            create_admin_user(pool, &opts).await?;
            admin_ready = true;
        }

        let project_path = opts
            .project_config
            .clone()
            .unwrap_or_else(|| PathBuf::from("config/wallaby_hires.v2.yaml"));

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

        let settings = Settings::load()?.settings;
        let setup_context = installation::InstallationContext::from_home(root.clone())?;
        let report =
            doctor::run_doctor(pool, &settings, None, Vec::new(), Some(&setup_context)).await;
        doctor::print_human(&report);
        if !report.ok {
            bail!("setup completed with doctor failures; fix checks above");
        }
    } else {
        maybe_validate_project_config(&opts)?;
    }

    let project_path = opts
        .project_config
        .clone()
        .unwrap_or_else(|| PathBuf::from("config/wallaby_hires.v2.yaml"));
    let commands = next_steps_lines(&SetupNextSteps {
        runtime_docker: prepare_docker,
        compose_postgres: postgres == PostgresKind::Compose,
        docker_context,
        db_applied,
        admin_ready,
        dash_dir,
        project_file: project_path
            .exists()
            .then(|| display_repo_path(&root, &project_path)),
    });
    if opts.start {
        finish_start(&root, prepare_docker, &opts)?;
    } else {
        print_recipe(&commands);
    }
    print_hint(
        "When you are ready to connect REST or Slurm, use `beampipe profile add` (see Deployment profiles and SSH).",
    );
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
        3
    } else {
        4
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
            "bind ports already in use: {}. Stop the other process or pass --no-start.",
            busy.join(", ")
        );
    }
    Ok(())
}

fn preflight_before_materialize(opts: &SetupOptions) -> Result<()> {
    match decide_runtime(opts)? {
        Some(RuntimeKind::Docker) => {
            require_docker_compose()?;
            if opts.start {
                require_bind_ports_free(&[(5432, "PostgreSQL"), (8080, "API"), (9090, "metrics")])?;
            }
        }
        Some(RuntimeKind::Host) if opts.start => {
            let mut ports = vec![(8080, "API")];
            if matches!(opts.postgres.as_deref(), Some("compose")) {
                ports.insert(0, (5432, "PostgreSQL"));
            }
            require_bind_ports_free(&ports)?;
        }
        None if opts.start => {
            require_bind_ports_free(&[(8080, "API")])?;
        }
        _ => {}
    }
    Ok(())
}

fn preflight_after_choices(
    opts: &SetupOptions,
    runtime: RuntimeKind,
    postgres: PostgresKind,
) -> Result<()> {
    if runtime == RuntimeKind::Docker {
        require_docker_compose()?;
    }
    if !opts.start {
        return Ok(());
    }
    let mut ports = Vec::new();
    if postgres == PostgresKind::Compose {
        ports.push((5432, "PostgreSQL"));
    }
    ports.push((8080, "API"));
    if runtime == RuntimeKind::Docker {
        ports.push((9090, "metrics"));
    }
    require_bind_ports_free(&ports)
}

fn host_start_command(root: &Path) -> String {
    format!("  beampipe --home {} start", root.display())
}

fn login_snippet_lines(username: &str) -> Vec<String> {
    vec![
        format!("  export ADMIN_USER={username}"),
        "  export ADMIN_PASSWORD=\"${ADMIN_PASSWORD:?set to the password setup printed}\"".into(),
        "  curl -fsS -X POST http://127.0.0.1:8080/api/v2/login \\".into(),
        "    -H 'Content-Type: application/json' \\".into(),
        "    -d \"{\\\"username\\\":\\\"${ADMIN_USER}\\\",\\\"password\\\":\\\"${ADMIN_PASSWORD}\\\"}\""
            .into(),
    ]
}

fn print_login_snippet(username: &str) {
    println!("Login once the API is up:");
    for line in login_snippet_lines(username) {
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

fn check_api_health() {
    let result = Command::new("curl")
        .args(["-fsS", "http://127.0.0.1:8080/api/v2/health"])
        .status();
    match result {
        Ok(status) if status.success() => {
            println!("API is up at http://127.0.0.1:8080/api/v2");
        }
        _ => {
            println!("Check http://127.0.0.1:8080/api/v2/health when the API is ready.");
        }
    }
}

fn finish_start(root: &Path, runtime_docker: bool, opts: &SetupOptions) -> Result<()> {
    if runtime_docker {
        let context = installation::InstallationContext::from_home(root.to_path_buf())?;
        runtime::start(&context)?;
        check_api_health();
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
    "BEAMPIPE_ENV=development\nBEAMPIPE_VERSION=0.1.0\nBEAMPIPE_JWT_SECRET=change-me\nDATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe\n".into()
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
        .unwrap_or_else(|| "0.1.0".into());
    update_env_file(env_path, "BEAMPIPE_VERSION", &version)
}

const DEFAULT_DASH_REPO: &str = "https://github.com/jbwod/beampipe-dash";
const DASH_OVERRIDE_FILE: &str = "compose.beampipe-local.yml";

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
        None => prompt_yes_no("Prepare Beampipe Dash?", false),
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

async fn create_admin_user(pool: &PgPool, opts: &SetupOptions) -> Result<()> {
    let username = if opts.yes {
        opts.admin_user.clone().unwrap_or_else(|| "admin".into())
    } else {
        prompt_default("Admin username", "admin")?
    };

    if repo::get_user_by_username(pool, &username).await?.is_some() {
        println!("Admin user '{username}' already exists; skipped.");
        print_login_snippet(&username);
        return Ok(());
    }

    let password = if opts.yes {
        match opts
            .admin_password
            .clone()
            .filter(|password| !password.is_empty())
        {
            Some(password) => password,
            None => {
                let password = generate_admin_password();
                println!("Generated admin password (shown once): {password}");
                password
            }
        }
    } else {
        rpassword::prompt_password("Admin password (12+ characters): ")?
    };
    let email = if opts.yes {
        opts.admin_email
            .clone()
            .unwrap_or_else(|| "admin@example.test".into())
    } else {
        prompt_default("Admin email", "admin@example.test")?
    };

    if password.len() < 12 {
        bail!("admin password must be at least 12 characters");
    }

    let hash = beampipe_auth::hash_password(&password)?;
    repo::create_user(pool, "Admin", &username, &email, &hash, true).await?;
    println!("Created admin user '{username}'.");
    print_login_snippet(&username);
    Ok(())
}

fn maybe_validate_project_config(opts: &SetupOptions) -> Result<()> {
    let project_path = opts
        .project_config
        .clone()
        .unwrap_or_else(|| PathBuf::from("config/wallaby_hires.v2.yaml"));
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

#[derive(Debug, Default, Clone)]
struct SetupNextSteps {
    runtime_docker: bool,
    compose_postgres: bool,
    docker_context: Option<String>,
    db_applied: bool,
    admin_ready: bool,
    dash_dir: Option<PathBuf>,
    project_file: Option<String>,
}

fn display_repo_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn next_steps_lines(steps: &SetupNextSteps) -> Vec<String> {
    let mut lines = Vec::new();
    if steps.compose_postgres {
        lines.push("  docker compose up -d postgres".into());
    }
    if steps.runtime_docker {
        if let Some(context) = &steps.docker_context {
            lines.push(format!("  # docker context: {context}"));
        }
        if !steps.db_applied {
            lines.push("  docker compose run --rm api migrate".into());
        }
        if !steps.admin_ready {
            lines.push("  docker compose run --rm api admin create-user \\".into());
            lines.push("    --username admin --email admin@example.test \\".into());
            lines.push("    --password 'replace-this-immediately' --superuser".into());
        }
        if !steps.db_applied {
            if let Some(project) = &steps.project_file {
                lines.push(format!(
                    "  docker compose run --rm api project add -f {project}"
                ));
            }
        }
        lines.push("  docker compose up -d api scheduler worker".into());
        if let Some(dash_dir) = &steps.dash_dir {
            lines.push(format!(
                "  cd {} && docker compose -f compose.yaml -f {DASH_OVERRIDE_FILE} up --build -d",
                dash_dir.display()
            ));
        }
    } else {
        if !steps.db_applied {
            lines.push("  beampipe migrate".into());
        }
        if !steps.admin_ready {
            lines.push("  beampipe admin create-user \\".into());
            lines.push("    --username admin --email admin@example.test \\".into());
            lines.push("    --password 'replace-this-immediately' --superuser".into());
        }
        if !steps.db_applied {
            if let Some(project) = &steps.project_file {
                lines.push(format!("  beampipe project add -f {project}"));
            }
        }
        lines.push("  beampipe start".into());
    }
    lines
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
        assert_eq!(setup_step_total(&yes_docker, true), 3);

        let host = SetupOptions {
            yes: true,
            runtime: Some("host".into()),
            ..Default::default()
        };
        assert_eq!(setup_step_total(&host, false), 3);

        let with_dash = SetupOptions {
            yes: true,
            dashboard: true,
            ..Default::default()
        };
        assert_eq!(setup_step_total(&with_dash, true), 4);

        let interactive = SetupOptions::default();
        assert_eq!(setup_step_total(&interactive, true), 4);
    }

    #[test]
    fn host_start_command_selects_installation_without_chdir() {
        let command = host_start_command(Path::new("/home/op/beampipe"));
        assert_eq!(command, "  beampipe --home /home/op/beampipe start");
    }

    #[test]
    fn login_snippet_reads_password_from_the_environment() {
        let joined = login_snippet_lines("admin").join("\n");
        assert!(joined.contains("export ADMIN_USER=admin"));
        assert!(joined.contains("ADMIN_PASSWORD:?set to the password setup printed"));
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
            .contains("BEAMPIPE_VERSION=0.1.0\n"));
    }

    #[test]
    fn generated_admin_password_is_long_enough() {
        let password = generate_admin_password();
        assert!(password.len() >= 12);
        assert!(password.starts_with("bp-"));
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
    fn docker_recipe_starts_with_postgres_and_does_not_assume_dash() {
        let lines = next_steps_lines(&SetupNextSteps {
            runtime_docker: true,
            compose_postgres: true,
            db_applied: false,
            admin_ready: false,
            project_file: Some("config/wallaby_hires.v2.yaml".into()),
            ..Default::default()
        });
        let joined = lines.join("\n");
        assert!(joined.contains("docker compose up -d postgres"));
        assert!(joined.contains("docker compose run --rm api migrate"));
        assert!(joined
            .contains("docker compose run --rm api project add -f config/wallaby_hires.v2.yaml"));
        assert!(joined.contains("docker compose up -d api scheduler worker"));
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
}

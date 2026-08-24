use anyhow::{bail, Context, Result};
use beampipe_config::Settings;
use beampipe_db::repo;
use beampipe_profiles::{DeploymentConfig, DeploymentProfile};
use beampipe_project::{ProjectConfig, StagingProvider};
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

const CASDA_STAGING_CAPABILITY: &str = "staging:casda_uws";
const DEFAULT_WORKER_CAPABILITIES: &str =
    "discovery:tap,manifest:generic,translation:daliuge,verification:output_inventory";
const DEFAULT_TM_URL: &str = "http://localhost:9000";
const DEFAULT_WORKER_POOL: &str = "default";
const DEFAULT_DATABASE_URL: &str = "postgres://postgres:postgres@localhost:5432/beampipe";
const DEFAULT_INTERACTIVE_DASHBOARD: bool = true;
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
    pub wallaby_sample: bool,
    pub profile_config: Option<PathBuf>,
    pub ssh_slot: Option<String>,
    pub ssh_private_key: Option<PathBuf>,
    pub ssh_public_key: Option<PathBuf>,
    pub ssh_known_hosts: Option<PathBuf>,
    pub ssh_passphrase_file: Option<PathBuf>,
    pub ssh_acl: bool,
    pub accept_host_key: bool,
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

#[derive(Debug, Clone)]
struct SelectedProjectConfig {
    path: PathBuf,
    config: ProjectConfig,
    spec_sha256: String,
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

fn color_enabled_for(stdout_tty: bool, no_color: bool, term: Option<&str>) -> bool {
    stdout_tty && !no_color && !matches!(term, Some(value) if value.eq_ignore_ascii_case("dumb"))
}

fn color_enabled() -> bool {
    color_enabled_for(
        stdout_is_tty(),
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var("TERM").ok().as_deref(),
    )
}

fn interactive_setup(yes: bool) -> bool {
    !yes && stdin_is_tty() && stdout_is_tty()
}

fn full_logo_fits(interactive: bool, terminal_width: Option<u16>) -> bool {
    interactive && terminal_width.is_some_and(|width| width >= 90)
}

fn format_step(n: usize, total: usize, title: &str) -> String {
    format!("STEP {n} OF {total}  {title}")
}

fn print_setup_logo(interactive: bool) {
    let terminal_width = crossterm::terminal::size().ok().map(|(width, _)| width);
    if !full_logo_fits(interactive, terminal_width) {
        return;
    }
    let logo = SETUP_LOGO.trim_end();
    if color_enabled() {
        println!("{}", logo.cyan());
    } else {
        println!("{logo}");
    }
    println!();
}

fn print_banner(start: bool, yes: bool) {
    let interactive = interactive_setup(yes);
    print_setup_logo(interactive);
    let title = format!("BEAMPIPE  /  SETUP  v{}", env!("CARGO_PKG_VERSION"));
    if color_enabled() {
        println!("{}", title.bold().cyan());
    } else {
        println!("{title}");
    }
    println!("{}", "─".repeat(62));
    print_hint("A guided setup for the API, scheduler, workers, and database.");
    if start {
        print_hint("Services will start only after configuration and safety checks pass.");
    } else {
        print_hint("Configure only: no services will be started (--no-start).");
    }
    if yes {
        print_hint("Automatic mode: supplied values and safe defaults will be used (--yes).");
    } else if interactive {
        print_hint("Press Enter to accept the highlighted default. Live backends stay off by default.");
    }
}

fn print_step(n: usize, total: usize, title: &str) {
    let heading = format_step(n, total, title);
    println!();
    if color_enabled() {
        println!("{}", heading.bold().cyan());
    } else {
        println!("{heading}");
    }
}

fn print_hint(text: &str) {
    println!("  {text}");
}

fn print_status(label: &str, detail: impl std::fmt::Display) {
    if color_enabled() {
        println!("  {} {:<20} {detail}", "[done]".green().bold(), label);
    } else {
        println!("  [done] {label:<20} {detail}");
    }
}

fn print_pending(label: &str, detail: impl std::fmt::Display) {
    if color_enabled() {
        println!("  {} {:<20} {detail}", "[later]".yellow().bold(), label);
    } else {
        println!("  [later] {label:<19} {detail}");
    }
}

fn print_section(title: &str) {
    println!();
    if color_enabled() {
        println!("{}", title.bold());
    } else {
        println!("{title}");
    }
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

fn read_prompt_line(label: &str) -> Result<String> {
    let mut line = String::new();
    let bytes = io::stdin().read_line(&mut line)?;
    if bytes == 0 {
        bail!(
            "input ended while waiting for {label}; rerun in a terminal or use --yes with explicit runtime options"
        );
    }
    Ok(line)
}

fn print_choice_items(items: &[ChoiceItem], default_index: usize) {
    for (index, item) in items.iter().enumerate() {
        let default = index == default_index;
        let marker = if default { " (default)" } else { "" };
        if color_enabled() && default {
            println!(
                "  {}) {}{}",
                index + 1,
                item.label.bold(),
                marker.cyan()
            );
        } else {
            println!("  {}) {}{marker}", index + 1, item.label);
        }
        println!("     {}", item.hint);
    }
}

fn prompt_choice(label: &str, items: &[ChoiceItem], default_index: usize) -> Result<usize> {
    loop {
        print_choice_items(items, default_index);
        print!("Choose {label} [{}]: ", default_index + 1);
        io::stdout().flush()?;
        let line = read_prompt_line(label)?;
        if let Some(index) = parse_choice(&line, items, default_index) {
            return Ok(index);
        }
        print_hint("Enter a number or the option name.");
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

fn add_capability(configured: &str, capability: &str) -> String {
    let mut capabilities = configured
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !capabilities.iter().any(|value| value == capability) {
        capabilities.push(capability.to_string());
    }
    capabilities.join(",")
}

fn with_project_staging_capabilities(
    configured: &str,
    project_config: Option<&ProjectConfig>,
) -> String {
    let Some(project_config) = project_config else {
        return configured.to_string();
    };
    let required = match &project_config.staging.provider {
        StagingProvider::None => &[][..],
        StagingProvider::CasdaUws => &[CASDA_STAGING_CAPABILITY][..],
    };
    required
        .iter()
        .fold(configured.to_string(), |capabilities, capability| {
            add_capability(&capabilities, capability)
        })
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

fn validate_live_request(requested: bool, profile_path: Option<&Path>) -> Result<()> {
    if requested && profile_path.is_none() {
        bail!(
            "--use-real-backends requires --profile-config (or an interactively selected profile) so setup can run profile doctor before enabling submission"
        );
    }
    Ok(())
}

fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

fn validate_setup_input(yes: bool, input_is_tty: bool) -> Result<()> {
    if !yes && !input_is_tty {
        bail!(
            "interactive setup requires a terminal; rerun in a terminal or use --yes with explicit runtime options"
        );
    }
    Ok(())
}

fn resolve_path_from(base: &Path, path: &Path) -> PathBuf {
    let path = expand_user_path(path);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn resolve_setup_option_paths(opts: &mut SetupOptions, launch_dir: &Path) {
    for path in [
        &mut opts.admin_password_file,
        &mut opts.project_config,
        &mut opts.profile_config,
        &mut opts.ssh_private_key,
        &mut opts.ssh_public_key,
        &mut opts.ssh_known_hosts,
        &mut opts.ssh_passphrase_file,
        &mut opts.dash_dir,
        &mut opts.directory,
        &mut opts.credentials_dir,
    ] {
        if let Some(value) = path.as_mut() {
            *value = resolve_path_from(launch_dir, value);
        }
    }
}

fn core_roles_running(counts: runtime::RoleCounts) -> bool {
    counts.api > 0 && counts.scheduler > 0 && counts.worker > 0
}

fn installation_services_running(context: &installation::InstallationContext) -> bool {
    if !context.exists()
        || !context
            .state
            .as_ref()
            .is_some_and(|state| state.runtime == RuntimeKind::Docker)
    {
        return false;
    }
    core_roles_running(runtime::running_role_counts(context))
}

pub async fn run_setup(mut opts: SetupOptions) -> Result<()> {
    validate_setup_input(opts.yes, stdin_is_tty())?;
    let launch_dir = std::env::current_dir().context("current directory")?;
    resolve_setup_option_paths(&mut opts, &launch_dir);
    let mut root = resolve_operator_root(&opts)?;

    let root_was_missing = !root.exists();
    if root_was_missing {
        if opts.yes {
            // Reject invalid unattended invocations before creating the target.
            decide_runtime(&opts)?;
        }
        print_banner(opts.start, opts.yes);
        print_hint(&format!("Installation home: {}", root.display()));
        if interactive_setup(opts.yes) && !prompt_yes_no("Set up Beampipe here?", true)? {
            bail!("setup aborted before creating the installation directory");
        }
    }
    std::fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
    root = root
        .canonicalize()
        .with_context(|| format!("resolve {}", root.display()))?;
    let existing_context = installation::InstallationContext::from_home(root.clone())?;
    let existing_services_running = installation_services_running(&existing_context);
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

    if !root_was_missing {
        print_banner(opts.start, opts.yes);
        print_hint(&format!("Installation home: {}", root.display()));
    }
    if !root_was_missing
        && interactive_setup(opts.yes)
        && !prompt_yes_no(
            if existing_context.exists() {
                "Update this Beampipe installation?"
            } else {
                "Set up Beampipe here?"
            },
            true,
        )?
    {
        bail!("setup aborted before writing installation files");
    }
    preflight_before_materialize(&opts, existing_services_running)?;
    std::env::set_current_dir(&root).with_context(|| format!("chdir {}", root.display()))?;
    let env_path = existing_context.environment_file.clone();

    let materialized = materialize::materialize(&root, false, opts.wallaby_sample)?;
    print_status(
        "Installation files",
        format!(
            "{} created, {} updated",
            materialized.created.len(),
            materialized.replaced.len()
        ),
    );
    if !materialized.bundle_current {
        println!(
            "Preserved operator-modified bundle files. Review them before relying on new release defaults."
        );
    }
    let mut selected_project = load_selected_project_config(&root, &opts)?;
    let compose_exists = compose_file_exists(&root);
    let tentative_docker = !matches!(decide_runtime(&opts)?, Some(RuntimeKind::Host));
    let total_steps = setup_step_total(&opts, tentative_docker);
    let mut step = 1;

    print_step(step, total_steps, "Runtime");
    step += 1;
    let runtime = resolve_runtime(&opts, compose_exists)?;
    if runtime == RuntimeKind::Docker && !compose_exists {
        bail!(
            "--runtime docker requires docker-compose.yml in {}",
            root.display()
        );
    }
    print_status(
        "Selected",
        match runtime {
            RuntimeKind::Docker => "Docker Compose (API, scheduler, and workers)",
            RuntimeKind::Host => "Host binary (beampipe start)",
        },
    );

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
        print_status("Selected", "Managed Compose PostgreSQL");
        if !env_existed {
            refuse_stale_compose_postgres_volume(&root)?;
        }
        if opts.start {
            print_hint("Starting Compose Postgres next.");
        } else {
            print_hint("Start it later with: docker compose up -d postgres");
        }
    } else {
        print_status("Selected", "Existing PostgreSQL URL");
    }

    print_step(step, total_steps, "Network");
    step += 1;
    let host_ports = resolve_host_ports(&opts, runtime, postgres)?;
    print_status(
        "API",
        format!("http://127.0.0.1:{}/api/v2", host_ports.api),
    );
    if postgres == PostgresKind::Compose {
        print_status(
            "PostgreSQL",
            format!("127.0.0.1:{}", host_ports.postgres),
        );
    }
    if runtime == RuntimeKind::Docker {
        print_status("Metrics", format!("127.0.0.1:{}", host_ports.metrics));
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
    preflight_after_choices(
        &opts,
        runtime,
        postgres,
        host_ports,
        existing_services_running,
    )?;

    print_step(step, total_steps, "Dashboard");
    step += 1;
    let mut prepare_dash = false;
    if prepare_docker {
        if decide_dashboard(&opts, prepare_docker) != Some(false) {
            prepare_dash = resolve_prepare_dashboard(&opts, prepare_docker)?;
        }
        if prepare_dash {
            print_status("Dashboard", "will be installed");
        } else {
            print_pending("Dashboard", "skipped");
        }
    } else {
        if opts.dashboard {
            bail!("--dashboard requires --runtime docker");
        }
        print_pending("Dashboard", "requires Docker runtime; skipped");
    }

    print_step(step, total_steps, "Project and deployment");
    step += 1;
    if selected_project.is_none() && interactive_setup(opts.yes) {
        selected_project = prompt_project_config(&root)?;
    }
    if let Some(selected) = selected_project.as_ref() {
        print_status("Project", &selected.config.metadata.id);
    } else {
        print_pending("Project", "configure later");
    }
    let selected_profile_path = select_profile_path(&opts, &root)?;
    if let Some(path) = selected_profile_path.as_ref() {
        print_status("Profile file", path.display());
    } else {
        print_pending("Profile", "configure later");
    }
    validate_live_request(opts.use_real_backends, selected_profile_path.as_deref())?;

    let activate_live_after_doctor = opts.use_real_backends;
    let mut use_real_backends = resolve_use_real_backends(
        opts.use_real_backends,
        std::env::var("BEAMPIPE_USE_REAL_BACKENDS").ok().as_deref(),
        env_file_value(&env_path, "BEAMPIPE_USE_REAL_BACKENDS").as_deref(),
    )
    .to_string();
    if activate_live_after_doctor {
        use_real_backends = "false".into();
    }

    print_step(step, total_steps, "Review");
    step += 1;
    print_setup_plan(&SetupPlan {
        root: &root,
        runtime,
        postgres,
        ports: host_ports,
        dashboard: prepare_dash,
        start: opts.start,
        project_id: selected_project
            .as_ref()
            .map(|selected| selected.config.metadata.id.as_str()),
        profile_path: selected_profile_path.as_deref(),
        live_backends: use_real_backends == "true",
        live_requested: activate_live_after_doctor,
    });
    if !opts.yes && !prompt_yes_no("Continue with this configuration?", true)? {
        bail!("setup aborted before environment configuration");
    }

    print_step(step, total_steps, "Configure");
    step += 1;
    let mut dash_dir = None;
    if prepare_dash {
        match prepare_dashboard(&root, &opts, &compose_network_name(&root)) {
            Ok(prepared) => {
                print_status("Dashboard", prepared.display());
                dash_dir = Some(prepared);
            }
            Err(error) if !opts.dashboard => {
                print_pending("Dashboard", format!("not prepared: {error}"));
            }
            Err(error) => return Err(error).context("prepare requested dashboard"),
        }
    }

    if !env_path.exists() {
        seed_env_file(&root, &env_path)?;
    } else if !opts.yes
        && !prompt_yes_no("Update Beampipe-managed settings in the existing `.env`?", true)?
    {
        bail!("setup aborted");
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
    let existing_grafana_password = std::env::var("BEAMPIPE_GRAFANA_ADMIN_PASSWORD")
        .ok()
        .or_else(|| env_file_value(&env_path, "BEAMPIPE_GRAFANA_ADMIN_PASSWORD"));
    let grafana_admin_password =
        select_grafana_admin_password(existing_grafana_password.as_deref(), env_existed)?;
    if !env_existed
        && !grafana_password_is_valid(existing_grafana_password.as_deref().unwrap_or(""))
    {
        println!("Generated a random Grafana administrator password and stored it in .env.");
    }

    let mut backend_capabilities = std::env::var("BEAMPIPE_BACKEND_CAPABILITIES")
        .ok()
        .or_else(|| env_file_value(&env_path, "BEAMPIPE_BACKEND_CAPABILITIES"))
        .unwrap_or_default();
    backend_capabilities = with_project_staging_capabilities(
        &backend_capabilities,
        selected_project.as_ref().map(|selected| &selected.config),
    );
    let mut worker_capabilities = std::env::var("BEAMPIPE_WORKER_CAPABILITIES")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env_file_value(&env_path, "BEAMPIPE_WORKER_CAPABILITIES"))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_WORKER_CAPABILITIES.to_string());
    worker_capabilities = with_project_staging_capabilities(
        &worker_capabilities,
        selected_project.as_ref().map(|selected| &selected.config),
    );
    let tm_url = env_override(opts.tm_url.as_deref(), "BEAMPIPE_TM_URL", DEFAULT_TM_URL);
    let worker_pool = env_override(
        opts.worker_pool.as_deref(),
        "BEAMPIPE_WORKER_POOL",
        DEFAULT_WORKER_POOL,
    );
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
    update_env_file(
        &env_path,
        "BEAMPIPE_GRAFANA_ADMIN_PASSWORD",
        &grafana_admin_password,
    )?;
    update_env_file(
        &env_path,
        "BEAMPIPE_BACKEND_CAPABILITIES",
        &backend_capabilities,
    )?;
    update_env_file(
        &env_path,
        "BEAMPIPE_WORKER_CAPABILITIES",
        &worker_capabilities,
    )?;
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
    std::env::set_var("BEAMPIPE_GRAFANA_ADMIN_PASSWORD", &grafana_admin_password);
    std::env::set_var("BEAMPIPE_BACKEND_CAPABILITIES", &backend_capabilities);
    std::env::set_var("BEAMPIPE_WORKER_CAPABILITIES", &worker_capabilities);
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

    let profile_path = selected_profile_path;
    let mut prepared_profile = profile_path
        .as_deref()
        .map(|path| prepare_deployment_profile(&opts, path, runtime))
        .transpose()?;
    if let Some(deployment_capability) =
        prepared_profile
            .as_ref()
            .map(|profile| match &profile.deployment {
                DeploymentConfig::SlurmRemote(_) => "deployment:slurm_remote",
                DeploymentConfig::RestRemote(_) => "deployment:daliuge_rest",
            })
    {
        backend_capabilities = add_capability(&backend_capabilities, deployment_capability);
        worker_capabilities = add_capability(&worker_capabilities, deployment_capability);
        update_env_file(
            &env_path,
            "BEAMPIPE_BACKEND_CAPABILITIES",
            &backend_capabilities,
        )?;
        update_env_file(
            &env_path,
            "BEAMPIPE_WORKER_CAPABILITIES",
            &worker_capabilities,
        )?;
        std::env::set_var("BEAMPIPE_BACKEND_CAPABILITIES", &backend_capabilities);
        std::env::set_var("BEAMPIPE_WORKER_CAPABILITIES", &worker_capabilities);
    }

    print_step(step, total_steps, "Initialize and verify");

    let pool = match beampipe_db::connect(&database_url).await {
        Ok(pool) => Some(pool),
        Err(error) if postgres == PostgresKind::Compose => {
            println!(
                "PostgreSQL is not reachable ({error}). Skipping migrate, admin, upload, and doctor."
            );
            if database_error_is_password_auth(&error) {
                println!(
                    "Compose PostgreSQL kept an older password in its existing volume. Setup will not delete it. Restore the original BEAMPIPE_POSTGRES_PASSWORD or back up and reset that database deliberately, then rerun setup."
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
    let mut project_uploaded = false;
    if let Some(pool) = pool.as_ref() {
        beampipe_db::migrate(pool).await.context("migrate")?;
        println!("Migrations applied.");
        db_applied = true;

        if !opts.skip_admin {
            create_admin_user(pool, &opts, host_ports.api, &root).await?;
            admin_ready = true;
        }

        if let Some(selected) = selected_project.as_ref() {
            println!(
                "Validated {} (project_id={})",
                selected.path.display(),
                selected.config.metadata.id
            );

            if !opts.skip_upload
                && (opts.yes || prompt_yes_no("Upload project config to database?", true)?)
            {
                upload_project_config(pool, &selected.config, &selected.spec_sha256).await?;
                println!("Uploaded project config '{}'.", selected.config.metadata.id);
                project_uploaded = true;
            }
        } else {
            println!("No project selected; skipped project validation and upload.");
        }

        if let Some(profile) = prepared_profile.as_ref() {
            let row = crate::operator::install_profile(pool, profile).await?;
            println!(
                "Installed deployment profile '{}' revision {}.",
                row.name, row.revision
            );
        }

        let doctor_profile = if activate_live_after_doctor {
            Some(
                prepared_profile
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "--use-real-backends requires a deployment profile so setup can run its doctor checks"
                        )
                    })?
                    .name
                    .as_str(),
            )
        } else {
            None
        };
        let settings = Settings::load()?.settings;
        let setup_context = installation::InstallationContext::from_home(root.clone())?;
        let report = doctor::run_doctor(
            pool,
            &settings,
            doctor_profile,
            Vec::new(),
            Some(&setup_context),
        )
        .await;
        doctor::print_human(&report);
        if !report.ok {
            bail!("setup completed with doctor failures; fix checks above");
        }
        if activate_live_after_doctor {
            update_env_file(&env_path, "BEAMPIPE_USE_REAL_BACKENDS", "true")?;
            std::env::set_var("BEAMPIPE_USE_REAL_BACKENDS", "true");
            use_real_backends = "true".into();
            print_status("Live backends", "enabled after profile doctor passed");
        }
    } else if let Some(selected) = selected_project.as_ref() {
        println!(
            "Validated {} (project_id={}). Upload after Postgres is up.",
            selected.path.display(),
            selected.config.metadata.id
        );
    } else {
        println!("No project selected; skipped project validation and upload.");
    }

    if activate_live_after_doctor && use_real_backends != "true" {
        bail!(
            "--use-real-backends was not enabled because PostgreSQL/profile doctor checks did not run; it remains false"
        );
    }

    let commands = next_steps_lines(&SetupNextSteps {
        runtime_docker: prepare_docker,
        compose_postgres: postgres == PostgresKind::Compose,
        docker_context,
        db_applied,
        admin_ready,
        project_uploaded,
        core_home: Some(root.clone()),
        dash_dir: dash_dir.clone(),
        project_file: selected_project
            .as_ref()
            .map(|selected| selected.path.display().to_string()),
        profile_file: profile_path
            .as_deref()
            .filter(|path| path.exists())
            .map(|path| path.display().to_string()),
    });
    let mut core_started = existing_services_running;
    if opts.start {
        core_started = finish_start(&root, prepare_docker, &opts, host_ports)?;
        if let Some(dash) = dash_dir.as_ref() {
            start_dashboard(&root, dash);
        }
    }
    let mut casda_staging = selected_project
        .as_ref()
        .is_some_and(|selected| selected.config.staging.provider == StagingProvider::CasdaUws);
    offer_next_actions(&mut NextActions {
        opts: &opts,
        root: &root,
        env_path: &env_path,
        runtime,
        started: core_started,
        pool: pool.as_ref(),
        prepared_profile: &mut prepared_profile,
        use_real_backends: &mut use_real_backends,
        casda_staging: &mut casda_staging,
    })
    .await?;
    print_setup_summary(
        &root,
        runtime,
        postgres,
        host_ports,
        core_started,
        prepared_profile.as_ref(),
        &use_real_backends,
    );
    print_access_summary(
        &root,
        runtime,
        host_ports,
        core_started,
        dash_dir.as_deref(),
    );
    print_final_next_steps(
        if core_started { &[] } else { &commands },
        &root,
        use_real_backends == "true",
        prepared_profile
            .as_ref()
            .is_some_and(|profile| matches!(&profile.deployment, DeploymentConfig::SlurmRemote(_))),
        casda_staging,
        opts.wallaby_sample,
    );
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

fn select_grafana_admin_password(existing: Option<&str>, env_existed: bool) -> Result<String> {
    if let Some(password) = existing.filter(|value| grafana_password_is_valid(value)) {
        return Ok(password.to_string());
    }
    if env_existed {
        bail!(
            "existing BEAMPIPE_GRAFANA_ADMIN_PASSWORD is empty, weak, or a placeholder; set a new password explicitly"
        );
    }
    Ok(generate_admin_password())
}

fn grafana_password_is_valid(password: &str) -> bool {
    let normalized = password.trim().to_ascii_lowercase();
    password.len() >= 12
        && ![
            "change-me-before-use",
            "replace-with-a-random-password",
            "changeme",
        ]
        .contains(&normalized.as_str())
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
    let _ = (opts, docker);
    8
}

struct SetupPlan<'a> {
    root: &'a Path,
    runtime: RuntimeKind,
    postgres: PostgresKind,
    ports: HostPorts,
    dashboard: bool,
    start: bool,
    project_id: Option<&'a str>,
    profile_path: Option<&'a Path>,
    live_backends: bool,
    live_requested: bool,
}

fn setup_plan_lines(plan: &SetupPlan<'_>) -> Vec<String> {
    let runtime = match plan.runtime {
        RuntimeKind::Docker => "Docker Compose",
        RuntimeKind::Host => "host binary",
    };
    let database = match plan.postgres {
        PostgresKind::Compose => "managed Compose PostgreSQL",
        PostgresKind::Existing => "existing PostgreSQL URL",
    };
    let mut ports = format!("API {}", plan.ports.api);
    if plan.postgres == PostgresKind::Compose {
        ports.push_str(&format!(", PostgreSQL {}", plan.ports.postgres));
    }
    if plan.runtime == RuntimeKind::Docker {
        ports.push_str(&format!(", metrics {}", plan.ports.metrics));
    }
    vec![
        format!("Installation       {}", plan.root.display()),
        format!("Runtime            {runtime}"),
        format!("Database           {database}"),
        format!("Host ports         {ports}"),
        format!(
            "Dashboard          {}",
            if plan.dashboard { "install" } else { "skip" }
        ),
        format!(
            "Project            {}",
            plan.project_id.unwrap_or("configure later")
        ),
        format!(
            "Profile            {}",
            plan.profile_path
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "configure later".into())
        ),
        format!(
            "Live backends       {}",
            if plan.live_requested {
                "enable only after profile doctor passes"
            } else if plan.live_backends {
                "enabled"
            } else {
                "off (safe default)"
            }
        ),
        format!(
            "Services            {}",
            if plan.start && plan.runtime == RuntimeKind::Docker {
                "start after checks"
            } else if plan.start {
                "start PostgreSQL if managed; host command follows"
            } else {
                "configure only"
            }
        ),
    ]
}

fn print_setup_plan(plan: &SetupPlan<'_>) {
    for line in setup_plan_lines(plan) {
        print_hint(&line);
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
            "bind ports already in use: {}. Stop the service that owns the port or choose a different setup port. For a running Beampipe installation, use `beampipe stop`; use --no-start to configure without binding ports.",
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

fn preflight_before_materialize(
    opts: &SetupOptions,
    existing_services_running: bool,
) -> Result<()> {
    match decide_runtime(opts)? {
        Some(RuntimeKind::Docker) => {
            require_docker_compose()?;
            if opts.yes && opts.start && !existing_services_running {
                require_bind_ports_free(&bind_ports_for_start(
                    resolved_host_ports(opts)?,
                    RuntimeKind::Docker,
                    guessed_postgres_for_preflight(opts, RuntimeKind::Docker),
                ))?;
            }
        }
        Some(RuntimeKind::Host) if opts.yes && opts.start && !existing_services_running => {
            require_bind_ports_free(&bind_ports_for_start(
                resolved_host_ports(opts)?,
                RuntimeKind::Host,
                guessed_postgres_for_preflight(opts, RuntimeKind::Host),
            ))?;
        }
        None if opts.start && !existing_services_running => {
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
    existing_services_running: bool,
) -> Result<()> {
    if runtime == RuntimeKind::Docker {
        require_docker_compose()?;
    }
    if !opts.start || existing_services_running {
        return Ok(());
    }
    require_bind_ports_free(&bind_ports_for_start(ports, runtime, postgres))
}

fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\"'\"'"))
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}

fn beampipe_recipe_command(root: Option<&Path>, args: &str) -> String {
    match root {
        Some(root) => format!("  beampipe --home {} {args}", shell_quote_path(root)),
        None => format!("  beampipe {args}"),
    }
}

fn host_start_command(root: &Path) -> String {
    beampipe_recipe_command(Some(root), "start")
}

fn print_login_snippet(username: &str, api_port: u16) {
    print_hint(&format!(
        "Sign in as '{username}' through Beampipe Dash or http://127.0.0.1:{api_port}/api/v2/docs."
    ));
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
        "Compose PostgreSQL volume `{volume}` already exists from a previous install, but this installation has no .env password to reuse. Setup will not delete the volume. Restore its original BEAMPIPE_POSTGRES_PASSWORD or back up and reset that database deliberately, then rerun setup"
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
    _opts: &SetupOptions,
    ports: HostPorts,
) -> Result<bool> {
    if runtime_docker {
        let context = installation::InstallationContext::from_home(root.to_path_buf())?;
        runtime::start(&context)?;
        check_api_health(ports.api);
        print_status("Core services", "started");
        return Ok(true);
    }
    print_pending(
        "Host process",
        "run the foreground start command shown in Next actions",
    );
    Ok(false)
}

fn prompt_default(label: &str, default: &str) -> Result<String> {
    print!("{label} [{default}]: ");
    io::stdout().flush()?;
    let line = read_prompt_line(label)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn parse_yes_no(input: &str, default_yes: bool) -> Option<bool> {
    match input.trim().to_ascii_lowercase().as_str() {
        "" => Some(default_yes),
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

fn prompt_yes_no(label: &str, default_yes: bool) -> Result<bool> {
    let hint = if default_yes { "Y/n" } else { "y/N" };
    loop {
        print!("{label} [{hint}]: ");
        io::stdout().flush()?;
        let line = read_prompt_line(label)?;
        if let Some(answer) = parse_yes_no(&line, default_yes) {
            return Ok(answer);
        }
        print_hint("Enter yes or no.");
    }
}

fn update_env_file(path: &Path, key: &str, value: &str) -> Result<()> {
    update_env_values(path, &[(key, value)])
}

fn update_env_values(path: &Path, updates: &[(&str, &str)]) -> Result<()> {
    let mut encoded = Vec::with_capacity(updates.len());
    for &(key, value) in updates {
        if key.is_empty()
            || !key.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            })
        {
            bail!("environment variable name must contain only A-Z, 0-9, and '_'");
        }
        if encoded.iter().any(|(existing, _)| existing == key) {
            bail!("environment variable {key} was supplied more than once");
        }
        encoded.push((key.to_string(), env_file_encode(value)?));
    }

    let content = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();

    for (key, value) in encoded {
        let prefix = format!("{key}=");
        let commented_prefix = format!("#{key}=");
        let replacement = format!("{key}={value}");
        let mut found = false;
        let mut updated = Vec::with_capacity(lines.len() + 1);
        for line in lines {
            if line.starts_with(&prefix) || line.starts_with(&commented_prefix) {
                if !found {
                    updated.push(replacement.clone());
                    found = true;
                }
            } else {
                updated.push(line);
            }
        }
        if !found {
            updated.push(replacement);
        }
        lines = updated;
    }

    write_private_file_atomic(path, &(lines.join("\n") + "\n"))
}

fn write_private_file_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("beampipe-env");
    let temporary = parent.join(format!(".{file_name}.tmp-{}", Uuid::new_v4().simple()));
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        set_private_file_permissions(&temporary)?;
        drop(file);
        std::fs::rename(&temporary, path)
            .with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
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
    dotenvy::from_path_iter(path)
        .ok()?
        .filter_map(|entry| entry.ok())
        .find_map(|(candidate, value)| {
            (candidate == key && !value.is_empty()).then_some(value)
        })
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
        None => prompt_yes_no(
            "Install Beampipe Dash?",
            DEFAULT_INTERACTIVE_DASHBOARD,
        ),
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
            shell_quote_path(&script),
            shell_quote_path(home),
            shell_quote_path(dash_dir)
        ),
        None => format!(
            "  sh {} --dash-dir {}",
            shell_quote_path(&script),
            shell_quote_path(dash_dir)
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
                "{error}. Clone it with: git clone {} {}",
                shell_quote(repo_url),
                shell_quote_path(&dash_dir)
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

async fn create_admin_user(
    pool: &PgPool,
    opts: &SetupOptions,
    api_port: u16,
    root: &Path,
) -> Result<()> {
    if opts.yes {
        return create_admin_user_once(pool, opts, api_port, root).await;
    }
    loop {
        match create_admin_user_once(pool, opts, api_port, root).await {
            Ok(()) => return Ok(()),
            Err(error) if is_retryable_admin_error(&error) => {
                print_hint(&error.to_string());
                print_hint("Try a different username, email, or password.");
            }
            Err(error) => return Err(error),
        }
    }
}

fn select_admin_text(
    explicit: Option<&str>,
    unattended: bool,
    label: &str,
    default: &str,
) -> Result<String> {
    match explicit {
        Some(value) => Ok(value.to_string()),
        None if unattended => Ok(default.to_string()),
        None => prompt_default(label, default),
    }
}

fn explicit_admin_password(opts: &SetupOptions) -> Result<Option<String>> {
    if let Some(password) = opts
        .admin_password
        .as_deref()
        .filter(|password| !password.is_empty())
    {
        return Ok(Some(password.to_string()));
    }
    opts.admin_password_file
        .as_deref()
        .map(|path| read_secret_file(path, "admin password"))
        .transpose()
}

async fn create_admin_user_once(
    pool: &PgPool,
    opts: &SetupOptions,
    api_port: u16,
    root: &Path,
) -> Result<()> {
    let username = select_admin_text(
        opts.admin_user.as_deref(),
        opts.yes,
        "Admin username",
        "admin",
    )?;
    if username.trim().is_empty() {
        bail!("admin username cannot be empty");
    }

    if repo::get_user_by_username(pool, &username).await?.is_some() {
        println!("Admin user '{username}' already exists; skipped.");
        print_login_snippet(&username, api_port);
        return Ok(());
    }

    let explicit_password = explicit_admin_password(opts)?;
    if opts
        .admin_password
        .as_deref()
        .is_some_and(|password| !password.is_empty())
    {
        eprintln!(
            "warning: --admin-password can be exposed through shell history; prefer --admin-password-file"
        );
    }
    let password = match explicit_password {
        Some(password) => password,
        None if opts.yes => {
            let password = generate_admin_password();
            let path = write_generated_admin_password(root, &password)?;
            println!(
                "Generated admin password and stored it at {} (0600).",
                path.display()
            );
            password
        }
        None => loop {
            let password = rpassword::prompt_password("Admin password (12+ characters): ")?;
            if let Err(error) = validate_admin_password(&password) {
                print_hint(&error.to_string());
                continue;
            }
            let confirmation = rpassword::prompt_password("Confirm admin password: ")?;
            if password != confirmation {
                print_hint("Passwords do not match. Try again.");
                continue;
            }
            break password;
        },
    };
    validate_admin_password(&password)?;
    let email = select_admin_text(
        opts.admin_email.as_deref(),
        opts.yes,
        "Admin email",
        "admin@example.test",
    )?;
    if email.trim().is_empty() {
        bail!("admin email cannot be empty");
    }

    let hash = beampipe_auth::hash_password(&password)?;
    repo::create_user(pool, "Admin", &username, &email, &hash, true).await?;
    println!("Created admin user '{username}'.");
    print_login_snippet(&username, api_port);
    Ok(())
}

fn write_generated_admin_password(root: &Path, password: &str) -> Result<PathBuf> {
    let path = root.join("credentials/admin/password");
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("admin password path has no parent"))?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    write_private_file_atomic(&path, &format!("{password}\n"))?;
    Ok(path)
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

fn load_selected_project_config(
    root: &Path,
    opts: &SetupOptions,
) -> Result<Option<SelectedProjectConfig>> {
    let Some(project_path) = project_config_path(root, opts) else {
        return Ok(None);
    };
    load_project_config(&project_path).map(Some)
}

fn load_project_config(project_path: &Path) -> Result<SelectedProjectConfig> {
    if !project_path.exists() {
        bail!(
            "selected project config was not found at {}",
            project_path.display()
        );
    }
    let bytes =
        std::fs::read(&project_path).with_context(|| format!("read {}", project_path.display()))?;
    let config = ProjectConfig::from_slice(&bytes)?;
    let report = config.validate_report();
    if !report.valid {
        bail!("project config invalid: {:?}", report.errors);
    }
    Ok(SelectedProjectConfig {
        path: project_path.to_path_buf(),
        config,
        spec_sha256: report.spec_sha256,
    })
}

fn prompt_project_config(root: &Path) -> Result<Option<SelectedProjectConfig>> {
    print_hint("Choose a project YAML/JSON now, or add one later with `beampipe project add`.");
    loop {
        let raw = prompt_default("Project config file (or skip)", "skip")?;
        if raw.eq_ignore_ascii_case("skip") {
            return Ok(None);
        }
        let path = resolve_under_root(root, &raw);
        match load_project_config(&path) {
            Ok(selected) => return Ok(Some(selected)),
            Err(error) => {
                print_hint(&error.to_string());
                print_hint("Enter another path, or type skip to continue.");
            }
        }
    }
}

fn project_config_path(root: &Path, opts: &SetupOptions) -> Option<PathBuf> {
    match opts.project_config.as_ref() {
        Some(path) => Some(resolve_explicit_path(path)),
        None if opts.wallaby_sample => Some(root.join("config/wallaby_hires.v2.yaml")),
        None => None,
    }
}

fn resolve_explicit_path(path: &Path) -> PathBuf {
    std::env::current_dir()
        .map(|cwd| resolve_path_from(&cwd, path))
        .unwrap_or_else(|_| expand_user_path(path))
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

fn select_profile_path(opts: &SetupOptions, root: &Path) -> Result<Option<PathBuf>> {
    if let Some(path) = opts.profile_config.as_ref() {
        let path = resolve_explicit_path(path);
        validate_profile_file(&path)?;
        return Ok(Some(path));
    }
    if opts.yes || !prompt_yes_no("Configure a deployment profile now?", false)? {
        return Ok(None);
    }
    prompt_profile_path(opts, root)
}

fn validate_profile_file(path: &Path) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let profile: DeploymentProfile =
        serde_yaml::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    Ok(profile.validate()?)
}

fn prompt_profile_path(opts: &SetupOptions, root: &Path) -> Result<Option<PathBuf>> {
    let default_display = if opts.wallaby_sample {
        root.join("config/deployment_profile.dlg-dim.json")
            .display()
            .to_string()
    } else {
        "skip".to_string()
    };
    loop {
        let raw = prompt_default("Deployment profile file (or skip)", &default_display)?;
        if raw.trim().eq_ignore_ascii_case("skip") {
            return Ok(None);
        }
        let path = resolve_under_root(root, &raw);
        match validate_profile_file(&path) {
            Ok(()) => return Ok(Some(path)),
            Err(error) => {
                print_hint(&error.to_string());
                print_hint("Enter another path, or type skip to continue without a profile.");
            }
        }
    }
}

fn prompt_profile_file(
    opts: &SetupOptions,
    root: &Path,
    runtime: RuntimeKind,
) -> Result<Option<(PathBuf, DeploymentProfile)>> {
    let default_display = if opts.wallaby_sample {
        root.join("config/deployment_profile.dlg-dim.json")
            .display()
            .to_string()
    } else {
        "skip".to_string()
    };
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
                non_interactive: opts.yes,
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
            non_interactive: false,
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
    project_uploaded: bool,
    core_home: Option<PathBuf>,
    dash_dir: Option<PathBuf>,
    project_file: Option<String>,
    profile_file: Option<String>,
}

fn next_action_choices(slurm_profile: bool, casda_staging: bool) -> Vec<ChoiceItem> {
    let mut choices = vec![
        ChoiceItem {
            key: "project",
            label: "Add or change project",
            hint: "validate and upload a project YAML/JSON",
        },
        ChoiceItem {
            key: "profile",
            label: "Add a deployment profile",
            hint: "REST DIM or Slurm JSON from the install config dir",
        },
    ];
    if slurm_profile {
        choices.push(ChoiceItem {
            key: "slurm",
            label: "Set up Slurm SSH credentials",
            hint: "generate or import the selected profile's managed key slot",
        });
    }
    if casda_staging {
        choices.push(ChoiceItem {
            key: "casda",
            label: "Set CASDA credentials",
            hint: "username and password required by the selected project",
        });
    }
    choices.extend([
        ChoiceItem {
            key: "doctor",
            label: "Run doctor for a profile",
            hint: "beampipe doctor --profile NAME",
        },
        ChoiceItem {
            key: "live",
            label: "Enable live backends",
            hint: "available only after profile doctor passes",
        },
        ChoiceItem {
            key: "done",
            label: "Done",
            hint: "finish setup",
        },
    ]);
    choices
}

fn next_action_recipe_lines(
    root: &Path,
    live_already: bool,
    slurm_profile: bool,
    casda_staging: bool,
    wallaby_sample: bool,
) -> Vec<String> {
    let home = root.display();
    let mut lines = vec![
        "Check the installation:".into(),
        beampipe_recipe_command(Some(root), "status"),
        beampipe_recipe_command(Some(root), "doctor"),
        "Add or update workload contracts when needed:".into(),
        beampipe_recipe_command(Some(root), "project add -f PROJECT_CONFIG"),
        beampipe_recipe_command(Some(root), "profile add -f PROFILE_CONFIG"),
        beampipe_recipe_command(Some(root), "doctor --profile PROFILE_NAME"),
    ];
    if slurm_profile {
        lines.push("Complete the selected Slurm profile's SSH credential slot:".into());
        lines.push(beampipe_recipe_command(
            Some(root),
            "slurm credentials init --slot SLOT --host LOGIN_NODE",
        ));
    }
    if casda_staging {
        lines.push("Add the CASDA credentials required by the selected project:".into());
        lines.push(format!(
            "  set CASDA_USERNAME in {home}/.env and store the password under {home}/credentials/casda"
        ));
    }
    if live_already {
        lines.push("Live backends are enabled (BEAMPIPE_USE_REAL_BACKENDS=true).".into());
    } else {
        lines.push("Keep mock submission on until the profile doctor passes, then:".into());
        lines.push(format!(
            "  set BEAMPIPE_USE_REAL_BACKENDS=true in {home}/.env"
        ));
        lines.push(beampipe_recipe_command(Some(root), "restart"));
    }
    if wallaby_sample {
        let dlg = root.join("config/deployment_profile.dlg-dim.json");
        let slurm = root.join("config/deployment_profile.slurm-remote.json");
        lines.push("WALLABY HiRes sample profiles:".into());
        lines.push(beampipe_recipe_command(
            Some(root),
            &format!("profile add -f {}", shell_quote_path(&dlg)),
        ));
        lines.push(beampipe_recipe_command(
            Some(root),
            "doctor --profile dlg-dim",
        ));
        lines.push(beampipe_recipe_command(
            Some(root),
            &format!("profile add -f {} --ssh-slot hpc", shell_quote_path(&slurm)),
        ));
    }
    lines
}

fn print_final_next_steps(
    startup_commands: &[String],
    root: &Path,
    live_already: bool,
    slurm_profile: bool,
    casda_staging: bool,
    wallaby_sample: bool,
) {
    print_section("NEXT ACTIONS");
    if !startup_commands.is_empty() {
        print_hint("Start the services when you are ready:");
        for command in startup_commands {
            println!("{command}");
        }
        println!();
    }
    for line in next_action_recipe_lines(
        root,
        live_already,
        slurm_profile,
        casda_staging,
        wallaby_sample,
    ) {
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
    casda_staging: &'a mut bool,
}

async fn offer_next_actions(ctx: &mut NextActions<'_>) -> Result<()> {
    if !next_actions_should_prompt(ctx.opts) {
        return Ok(());
    }

    print_section("OPTIONAL CONFIGURATION");
    print_hint("Mock submissions finish immediately and never create a DIM session.");
    print_hint(&format!(
        "BEAMPIPE_USE_REAL_BACKENDS={}",
        ctx.use_real_backends
    ));
    print_hint("Enable live backends only after `beampipe doctor --profile NAME` passes.");

    loop {
        let slurm_profile = ctx.prepared_profile.as_ref().is_some_and(|profile| {
            matches!(profile.deployment, DeploymentConfig::SlurmRemote(_))
        });
        let items = next_action_choices(slurm_profile, *ctx.casda_staging);
        let default_index = items.len() - 1;
        let choice = prompt_choice("Next action", &items, default_index)?;
        match items[choice].key {
            "project" => match next_action_project(ctx).await {
                Ok(()) => {}
                Err(error) => print_hint(&error.to_string()),
            },
            "live" => {
                if let Err(error) = enable_live_backends(
                    ctx.root,
                    ctx.env_path,
                    ctx.runtime,
                    ctx.started,
                    ctx.pool,
                    ctx.prepared_profile.as_ref(),
                    ctx.use_real_backends,
                )
                .await
                {
                    print_hint(&error.to_string());
                }
            }
            "profile" => match prompt_profile_file(ctx.opts, ctx.root, ctx.runtime) {
                Ok(Some((_, profile))) => {
                    if let Err(error) = install_prepared_profile(ctx.pool, &profile).await {
                        print_hint(&error.to_string());
                    }
                    *ctx.prepared_profile = Some(profile);
                }
                Ok(None) => {}
                Err(error) => print_hint(&error.to_string()),
            },
            "slurm" => {
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
            "casda" => {
                if let Err(error) =
                    next_action_casda_credentials(ctx.root, ctx.env_path, ctx.runtime, ctx.started)
                {
                    print_hint(&error.to_string());
                }
            }
            "doctor" => {
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

async fn next_action_project(ctx: &mut NextActions<'_>) -> Result<()> {
    let Some(selected) = prompt_project_config(ctx.root)? else {
        return Ok(());
    };
    let pool = ctx
        .pool
        .ok_or_else(|| anyhow::anyhow!("PostgreSQL is not reachable; add the project later"))?;
    upload_project_config(pool, &selected.config, &selected.spec_sha256).await?;
    println!("Uploaded project config '{}'.", selected.config.metadata.id);

    let backend = env_file_value(ctx.env_path, "BEAMPIPE_BACKEND_CAPABILITIES")
        .unwrap_or_default();
    let worker = env_file_value(ctx.env_path, "BEAMPIPE_WORKER_CAPABILITIES")
        .unwrap_or_else(|| DEFAULT_WORKER_CAPABILITIES.into());
    let backend = with_project_staging_capabilities(&backend, Some(&selected.config));
    let worker = with_project_staging_capabilities(&worker, Some(&selected.config));
    update_env_file(ctx.env_path, "BEAMPIPE_BACKEND_CAPABILITIES", &backend)?;
    update_env_file(ctx.env_path, "BEAMPIPE_WORKER_CAPABILITIES", &worker)?;
    std::env::set_var("BEAMPIPE_BACKEND_CAPABILITIES", &backend);
    std::env::set_var("BEAMPIPE_WORKER_CAPABILITIES", &worker);
    *ctx.casda_staging = selected.config.staging.provider == StagingProvider::CasdaUws;
    restart_stack_if_needed(
        ctx.root,
        ctx.runtime,
        ctx.started,
        "the selected project's capabilities",
    )?;
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

async fn enable_live_backends(
    root: &Path,
    env_path: &Path,
    runtime: RuntimeKind,
    started: bool,
    pool: Option<&PgPool>,
    profile: Option<&DeploymentProfile>,
    use_real_backends: &mut String,
) -> Result<()> {
    if use_real_backends == "true" {
        print_hint("Live backends are already enabled.");
        return Ok(());
    }
    print_hint("Workers will submit to real TM/DIM or Slurm instead of completing locally.");
    let profile = profile.ok_or_else(|| {
        anyhow::anyhow!("add a deployment profile before enabling live backends")
    })?;
    let pool = pool.ok_or_else(|| {
        anyhow::anyhow!("PostgreSQL must be reachable before enabling live backends")
    })?;
    print_hint(&format!(
        "Running required checks for profile '{}' before enabling submission.",
        profile.name
    ));
    let settings = Settings::load()?.settings;
    let context = installation::InstallationContext::from_home(root.to_path_buf())?;
    let report = doctor::run_doctor(
        pool,
        &settings,
        Some(&profile.name),
        Vec::new(),
        Some(&context),
    )
    .await;
    doctor::print_human(&report);
    if !report.ok {
        bail!(
            "profile doctor failed; live backends remain disabled until every required check passes"
        );
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
                "Could not recreate the stack ({error}). Run `{}`.",
                beampipe_recipe_command(Some(root), "restart").trim()
            )),
        }
    } else if runtime == RuntimeKind::Docker {
        print_hint(&format!(
            "When the stack is up: {}",
            beampipe_recipe_command(Some(root), "restart").trim()
        ));
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
            non_interactive: false,
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
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push('"');
    for character in value.chars() {
        match character {
            '\\' => encoded.push_str("\\\\"),
            '"' => encoded.push_str("\\\""),
            '$' => encoded.push_str("\\$"),
            _ => encoded.push(character),
        }
    }
    encoded.push('"');
    Ok(encoded)
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
    // Validate every value before writing either the private credential or .env.
    env_file_encode(username)?;
    env_file_encode(password)?;
    let file_path = casda_password_file_path(root);
    let file_path_value = file_path.display().to_string();
    env_file_encode(&file_path_value)?;
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    write_private_file_atomic(&file_path, &format!("{password}\n"))?;

    match runtime {
        RuntimeKind::Docker => {
            // Compose env_file cannot see a host CASDA_PASSWORD_FILE path.
            update_env_values(
                env_path,
                &[
                    ("CASDA_USERNAME", username),
                    ("CASDA_PASSWORD", password),
                    ("CASDA_PASSWORD_FILE", ""),
                ],
            )?;
        }
        RuntimeKind::Host => {
            update_env_values(
                env_path,
                &[
                    ("CASDA_USERNAME", username),
                    ("CASDA_PASSWORD_FILE", &file_path_value),
                    ("CASDA_PASSWORD", ""),
                ],
            )?;
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
        if let Some(root) = steps.core_home.as_deref() {
            lines.push(format!(
                "  docker compose --project-directory {} up -d postgres",
                shell_quote_path(root)
            ));
        } else {
            lines.push("  docker compose up -d postgres".into());
        }
    }
    if steps.runtime_docker {
        if let Some(context) = &steps.docker_context {
            lines.push(format!("  # docker context: {context}"));
        }
        lines.push(match steps.core_home.as_deref() {
            Some(root) => host_start_command(root),
            None => beampipe_recipe_command(None, "start"),
        });
        if !steps.db_applied {
            lines.push(beampipe_recipe_command(
                steps.core_home.as_deref(),
                "migrate",
            ));
        }
        if !steps.admin_ready {
            lines.push(format!(
                "{} \\",
                beampipe_recipe_command(steps.core_home.as_deref(), "admin create-user")
            ));
            lines.push("    --username admin --email admin@example.test \\".into());
            lines.push("    --password-file /path/to/protected/admin-password --superuser".into());
        }
        if !steps.project_uploaded {
            if let Some(project) = &steps.project_file {
                lines.push(beampipe_recipe_command(
                    steps.core_home.as_deref(),
                    &format!("project add -f {}", shell_quote(project)),
                ));
            }
        }
        if !steps.db_applied {
            if let Some(profile) = &steps.profile_file {
                lines.push(beampipe_recipe_command(
                    steps.core_home.as_deref(),
                    &format!("profile add -f {}", shell_quote(profile)),
                ));
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
            lines.push(beampipe_recipe_command(
                steps.core_home.as_deref(),
                "migrate",
            ));
        }
        if !steps.admin_ready {
            lines.push(format!(
                "{} \\",
                beampipe_recipe_command(steps.core_home.as_deref(), "admin create-user")
            ));
            lines.push("    --username admin --email admin@example.test \\".into());
            lines.push("    --password-file /path/to/protected/admin-password --superuser".into());
        }
        if !steps.project_uploaded {
            if let Some(project) = &steps.project_file {
                lines.push(beampipe_recipe_command(
                    steps.core_home.as_deref(),
                    &format!("project add -f {}", shell_quote(project)),
                ));
            }
        }
        if !steps.db_applied {
            if let Some(profile) = &steps.profile_file {
                lines.push(beampipe_recipe_command(
                    steps.core_home.as_deref(),
                    &format!("profile add -f {}", shell_quote(profile)),
                ));
            }
        }
        lines.push(beampipe_recipe_command(
            steps.core_home.as_deref(),
            "start",
        ));
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
    let api_url = format!("http://127.0.0.1:{api_port}/api/v2");
    let docs_url = format!("{api_url}/docs");
    let api = if started {
        api_url
    } else {
        format!("{api_url}  (after start)")
    };
    let docs = if started {
        docs_url
    } else {
        format!("{docs_url}  (after start)")
    };
    let dashboard = dashboard
        .map(|url| {
            if started {
                url.to_string()
            } else {
                format!("{url}  (after start)")
            }
        })
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
        "ACCESS".into(),
        format!("  API             {api}"),
        format!("  API docs        {docs}"),
        format!("  Dashboard       {dashboard}"),
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
    println!();
    for line in access_summary_lines(ports.api, dashboard.as_deref(), counts, started) {
        if line == "ACCESS" && color_enabled() {
            println!("{}", line.bold());
        } else {
            println!("{line}");
        }
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
    let _ = ports;
    print_section("SETUP COMPLETE");
    print_status("Home", root.display());
    print_status("Runtime", runtime.as_str());
    print_status("Database", postgres.as_str());
    if started {
        print_status("Core services", "running");
    } else {
        print_pending("Core services", "not started");
    }
    if use_real_backends == "true" {
        print_status("Live backends", "enabled");
    } else {
        print_pending("Live backends", "mock mode until profile doctor passes");
    }
    if let Some(profile) = profile {
        print_status("Profile", &profile.name);
        if let DeploymentConfig::SlurmRemote(slurm) = &profile.deployment {
            if let Some(slot) = slurm.ssh_credential.as_deref() {
                print_status("SSH slot", slot);
            } else {
                print_pending("SSH slot", "configure later");
            }
        }
    } else {
        print_pending("Profile", "configure later");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_setup_project(path: &Path, project_id: &str, staging_provider: &str) {
        let yaml = format!(
            r#"apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata:
  id: {project_id}
adapters:
  required: [catalog]
  endpoints:
    catalog:
      url: https://catalog.example.invalid/tap
staging:
  provider: {staging_provider}
"#
        );
        std::fs::write(path, yaml).unwrap();
    }

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

        let next = next_action_choices(true, true);
        assert_eq!(parse_choice("", &next, 6), Some(6));
        assert_eq!(parse_choice("done", &next, 0), Some(6));
        assert_eq!(parse_choice("project", &next, 6), Some(0));
        assert_eq!(parse_choice("profile", &next, 6), Some(1));
        assert_eq!(parse_choice("slurm", &next, 6), Some(2));
        assert_eq!(parse_choice("casda", &next, 6), Some(3));
        assert_eq!(parse_choice("doctor", &next, 6), Some(4));
        assert_eq!(parse_choice("live", &next, 6), Some(5));
        assert_eq!(parse_choice("7", &next, 6), Some(6));
    }

    #[test]
    fn core_running_requires_every_runtime_role() {
        assert!(core_roles_running(runtime::RoleCounts {
            api: 1,
            scheduler: 1,
            worker: 1,
        }));
        assert!(!core_roles_running(runtime::RoleCounts {
            api: 0,
            scheduler: 0,
            worker: 0,
        }));
        assert!(!core_roles_running(runtime::RoleCounts {
            api: 1,
            scheduler: 1,
            worker: 0,
        }));
    }

    #[tokio::test]
    async fn invalid_unattended_setup_does_not_create_its_home() {
        let parent = tempfile::tempdir().unwrap();
        let home = parent.path().join("not-created");
        let error = run_setup(SetupOptions {
            yes: true,
            directory: Some(home.clone()),
            ..SetupOptions::default()
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("--yes requires --runtime"));
        assert!(!home.exists());
    }

    #[test]
    fn yes_no_parser_accepts_only_explicit_answers() {
        assert_eq!(parse_yes_no("", true), Some(true));
        assert_eq!(parse_yes_no("", false), Some(false));
        assert_eq!(parse_yes_no("y", false), Some(true));
        assert_eq!(parse_yes_no("YES", false), Some(true));
        assert_eq!(parse_yes_no("n", true), Some(false));
        assert_eq!(parse_yes_no("No", true), Some(false));
        assert_eq!(parse_yes_no("banana", true), None);
    }

    #[test]
    fn styling_respects_no_color_and_limited_terminals() {
        assert!(color_enabled_for(true, false, Some("xterm-256color")));
        assert!(!color_enabled_for(true, true, Some("xterm-256color")));
        assert!(!color_enabled_for(true, false, Some("dumb")));
        assert!(!color_enabled_for(false, false, Some("xterm-256color")));
        assert!(full_logo_fits(true, Some(90)));
        assert!(!full_logo_fits(true, Some(89)));
        assert!(!full_logo_fits(false, Some(120)));
    }

    #[test]
    fn noninteractive_input_requires_yes_before_setup_writes() {
        let error = validate_setup_input(false, false).unwrap_err();
        assert!(error.to_string().contains("--yes"));
        assert!(validate_setup_input(true, false).is_ok());
        assert!(validate_setup_input(false, true).is_ok());
    }

    #[test]
    fn setup_paths_are_resolved_against_the_launch_directory() {
        let launch = Path::new("/launch/worktree");
        let mut opts = SetupOptions {
            project_config: Some("projects/example.yaml".into()),
            profile_config: Some("profiles/local.json".into()),
            admin_password_file: Some("secrets/admin".into()),
            directory: Some("operator".into()),
            ..Default::default()
        };
        resolve_setup_option_paths(&mut opts, launch);
        assert_eq!(
            opts.project_config.as_deref(),
            Some(Path::new("/launch/worktree/projects/example.yaml"))
        );
        assert_eq!(
            opts.profile_config.as_deref(),
            Some(Path::new("/launch/worktree/profiles/local.json"))
        );
        assert_eq!(
            opts.admin_password_file.as_deref(),
            Some(Path::new("/launch/worktree/secrets/admin"))
        );
        assert_eq!(
            opts.directory.as_deref(),
            Some(Path::new("/launch/worktree/operator"))
        );
    }

    #[test]
    fn capability_actions_follow_the_selected_contracts() {
        let neutral = next_action_choices(false, false);
        assert!(neutral.iter().all(|item| item.key != "slurm"));
        assert!(neutral.iter().all(|item| item.key != "casda"));

        let configured = next_action_choices(true, true);
        assert!(configured.iter().any(|item| item.key == "slurm"));
        assert!(configured.iter().any(|item| item.key == "casda"));
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
    fn explicit_live_request_requires_a_profile_before_configuration() {
        let error = validate_live_request(true, None).unwrap_err();
        assert!(error.to_string().contains("--profile-config"));
        assert!(validate_live_request(true, Some(Path::new("profile.json"))).is_ok());
        assert!(validate_live_request(false, None).is_ok());
    }

    #[test]
    fn capabilities_are_added_without_duplicates() {
        let defaults = DEFAULT_WORKER_CAPABILITIES.split(',').collect::<Vec<_>>();
        assert_eq!(
            defaults,
            vec![
                "discovery:tap",
                "manifest:generic",
                "translation:daliuge",
                "verification:output_inventory"
            ]
        );
        assert!(!defaults
            .iter()
            .any(|value| value.starts_with("deployment:")));
        assert_eq!(
            add_capability("staging:casda_uws", "deployment:slurm_remote"),
            "staging:casda_uws,deployment:slurm_remote"
        );
        assert_eq!(
            add_capability(
                "deployment:slurm_remote,staging:casda_uws",
                "deployment:slurm_remote"
            ),
            "deployment:slurm_remote,staging:casda_uws"
        );
    }

    #[test]
    fn custom_casda_project_adds_staging_capability_to_backend_and_worker() {
        let root = tempfile::tempdir().unwrap();
        let project_path = root.path().join("custom-casda.yaml");
        write_setup_project(&project_path, "custom_archive", "casda_uws");
        let opts = SetupOptions {
            project_config: Some(project_path.clone()),
            ..Default::default()
        };
        let selected = load_selected_project_config(root.path(), &opts)
            .unwrap()
            .unwrap();

        assert_eq!(selected.path, project_path);
        assert_eq!(selected.config.metadata.id, "custom_archive");
        assert_eq!(
            with_project_staging_capabilities("custom:backend", Some(&selected.config),),
            "custom:backend,staging:casda_uws"
        );
        assert_eq!(
            with_project_staging_capabilities(
                "custom:worker,staging:casda_uws",
                Some(&selected.config),
            ),
            "custom:worker,staging:casda_uws"
        );
    }

    #[test]
    fn custom_no_staging_project_preserves_capabilities_without_provider_addition() {
        let root = tempfile::tempdir().unwrap();
        let project_path = root.path().join("custom-none.yaml");
        write_setup_project(&project_path, "custom_archive", "none");
        let opts = SetupOptions {
            project_config: Some(project_path),
            ..Default::default()
        };
        let selected = load_selected_project_config(root.path(), &opts)
            .unwrap()
            .unwrap();

        assert_eq!(
            with_project_staging_capabilities("custom:backend", Some(&selected.config),),
            "custom:backend"
        );
        assert_eq!(
            with_project_staging_capabilities("custom:worker", Some(&selected.config)),
            "custom:worker"
        );
    }

    #[test]
    fn sample_staging_capability_comes_from_validated_sample_config() {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        write_setup_project(
            &config_dir.join("wallaby_hires.v2.yaml"),
            "sample_project",
            "casda_uws",
        );
        let opts = SetupOptions {
            wallaby_sample: true,
            ..Default::default()
        };
        let selected = load_selected_project_config(root.path(), &opts)
            .unwrap()
            .unwrap();

        assert_eq!(selected.config.metadata.id, "sample_project");
        assert_eq!(
            with_project_staging_capabilities("", Some(&selected.config)),
            "staging:casda_uws"
        );
    }

    #[test]
    fn next_action_recipe_is_neutral_unless_wallaby_is_selected() {
        let root = Path::new("/home/op/beampipe");
        let mock = next_action_recipe_lines(root, false, false, false, false).join("\n");
        assert!(mock.contains("BEAMPIPE_USE_REAL_BACKENDS=true"));
        assert!(mock.contains("PROJECT_CONFIG"));
        assert!(mock.contains("PROFILE_CONFIG"));
        assert!(!mock.contains("wallaby"));
        assert!(!mock.contains("CASDA"));
        assert!(!mock.contains("Live backends are on"));

        let wallaby = next_action_recipe_lines(root, false, true, true, true).join("\n");
        assert!(wallaby.contains("deployment_profile.dlg-dim.json"));
        assert!(wallaby.contains("slurm credentials init"));
        assert!(wallaby.contains("CASDA_USERNAME"));

        let live = next_action_recipe_lines(root, true, false, false, false).join("\n");
        assert!(live.contains("Live backends are enabled"));
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
        assert_eq!(
            env_file_value(&env_path, "CASDA_USERNAME").as_deref(),
            Some("casda.user@example.test")
        );
        assert_eq!(
            env_file_value(&env_path, "CASDA_PASSWORD").as_deref(),
            Some(password)
        );
        assert_eq!(env_file_value(&env_path, "CASDA_PASSWORD_FILE"), None);
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

        assert_eq!(
            env_file_value(&env_path, "CASDA_USERNAME").as_deref(),
            Some("casda.user@example.test")
        );
        assert_eq!(env_file_value(&env_path, "CASDA_PASSWORD"), None);
        assert_eq!(
            env_file_value(&env_path, "CASDA_PASSWORD_FILE").as_deref(),
            Some(file_path.to_string_lossy().as_ref())
        );
        let env = std::fs::read_to_string(&env_path).unwrap();
        assert!(!env.contains("old-inline-password"));
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            format!("{password}\n")
        );
    }

    #[test]
    fn env_file_encoding_round_trips_special_characters() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".env");
        let values = [
            "plain",
            "space # value",
            "dollar$HOME and ${USER}",
            "single' and double\" quotes",
            "backslash\\tail",
        ];
        for (index, value) in values.iter().enumerate() {
            let key = format!("SECRET_{index}");
            update_env_file(&path, &key, value).unwrap();
            assert_eq!(env_file_value(&path, &key).as_deref(), Some(*value));
        }
        assert!(env_file_encode("line one\nline two").is_err());
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
        assert!(joined.contains("ACCESS"));
        assert!(joined.contains("API             http://127.0.0.1:18080/api/v2"));
        assert!(joined.contains("API docs        http://127.0.0.1:18080/api/v2/docs"));
        assert!(joined.contains("Dashboard       http://127.0.0.1:3000"));
        assert!(joined.contains("Beampipe is now up with 1 scheduler, 2 worker, 1 API"));
    }

    #[test]
    fn access_summary_without_dashboard_or_start() {
        let lines = access_summary_lines(18080, None, runtime::RoleCounts::default(), false);
        let joined = lines.join("\n");
        assert!(joined.contains("Dashboard       not installed"));
        assert!(joined.contains("http://127.0.0.1:18080/api/v2  (after start)"));
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
            "STEP 1 OF 4  How will you run Beampipe?"
        );
    }

    #[test]
    fn setup_uses_a_stable_eight_step_progress_sequence() {
        let yes_docker = SetupOptions {
            yes: true,
            runtime: Some("docker".into()),
            ..Default::default()
        };
        assert_eq!(setup_step_total(&yes_docker, true), 8);

        let host = SetupOptions {
            yes: true,
            runtime: Some("host".into()),
            ..Default::default()
        };
        assert_eq!(setup_step_total(&host, false), 8);

        let with_dash = SetupOptions {
            yes: true,
            dashboard: true,
            ..Default::default()
        };
        assert_eq!(setup_step_total(&with_dash, true), 8);

        let interactive = SetupOptions::default();
        assert_eq!(setup_step_total(&interactive, true), 8);
    }

    #[test]
    fn review_plan_is_complete_and_marks_safe_defaults() {
        let lines = setup_plan_lines(&SetupPlan {
            root: Path::new("/home/op/beampipe"),
            runtime: RuntimeKind::Docker,
            postgres: PostgresKind::Compose,
            ports: HostPorts {
                api: 18080,
                postgres: 5432,
                metrics: 9090,
            },
            dashboard: true,
            start: true,
            project_id: None,
            profile_path: None,
            live_backends: false,
            live_requested: false,
        })
        .join("\n");
        assert!(lines.contains("/home/op/beampipe"));
        assert!(lines.contains("Docker Compose"));
        assert!(lines.contains("API 18080, PostgreSQL 5432, metrics 9090"));
        assert!(lines.contains("Dashboard          install"));
        assert!(lines.contains("Project            configure later"));
        assert!(lines.contains("Profile            configure later"));
        assert!(lines.contains("Live backends       off (safe default)"));
        assert!(lines.contains("Services            start after checks"));
    }

    #[test]
    fn host_start_command_selects_installation_without_chdir() {
        let command = host_start_command(Path::new("/home/op/beampipe"));
        assert_eq!(command, "  beampipe --home '/home/op/beampipe' start");
    }

    #[test]
    fn shell_quoted_paths_round_trip_spaces_and_single_quotes() {
        let path = "/tmp/Beampipe operator's files";
        let quoted = shell_quote(path);
        let output = Command::new("sh")
            .args(["-c", &format!("printf '%s' {quoted}")])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), path);
        assert_eq!(
            host_start_command(Path::new(path)),
            "  beampipe --home '/tmp/Beampipe operator'\"'\"'s files' start"
        );
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
        assert!(message.contains("beampipe stop"));
        assert!(!message.contains("down --volumes"));
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
    fn env_updates_are_single_line_and_private() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".env");
        std::fs::write(&path, "DATABASE_URL=old\nUNCHANGED=value\n").unwrap();

        update_env_file(&path, "DATABASE_URL", "postgres://localhost/beampipe").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            env_file_value(&path, "DATABASE_URL").as_deref(),
            Some("postgres://localhost/beampipe")
        );
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
        assert_eq!(
            env_file_value(&created, "BEAMPIPE_VERSION").as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn generated_admin_password_is_long_enough() {
        let password = generate_admin_password();
        assert!(password.len() >= 12);
        assert!(password.starts_with("bp-"));
    }

    #[test]
    fn explicit_admin_identity_values_win_in_guided_and_unattended_modes() {
        for unattended in [false, true] {
            assert_eq!(
                select_admin_text(
                    Some("operator"),
                    unattended,
                    "Admin username",
                    "admin"
                )
                .unwrap(),
                "operator"
            );
            assert_eq!(
                select_admin_text(
                    Some("operator@example.test"),
                    unattended,
                    "Admin email",
                    "admin@example.test"
                )
                .unwrap(),
                "operator@example.test"
            );
        }
    }

    #[test]
    fn explicit_admin_password_file_is_mode_independent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("admin-password");
        std::fs::write(&path, "a-guided-secret-123\n").unwrap();
        for yes in [false, true] {
            let options = SetupOptions {
                yes,
                admin_password_file: Some(path.clone()),
                ..SetupOptions::default()
            };
            assert_eq!(
                explicit_admin_password(&options).unwrap().as_deref(),
                Some("a-guided-secret-123")
            );
        }
    }

    #[test]
    fn generated_admin_password_is_stored_privately() {
        let root = tempfile::tempdir().unwrap();
        let path = write_generated_admin_password(root.path(), "unit-test-password").unwrap();
        assert_eq!(path, root.path().join("credentials/admin/password"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "unit-test-password\n"
        );
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
    fn project_config_is_not_implicitly_selected() {
        let root = Path::new("/home/op/beampipe");
        assert_eq!(project_config_path(root, &SetupOptions::default()), None);
    }

    #[test]
    fn wallaby_sample_selects_its_materialized_project() {
        let root = Path::new("/home/op/beampipe");
        let opts = SetupOptions {
            wallaby_sample: true,
            ..SetupOptions::default()
        };
        assert_eq!(
            project_config_path(root, &opts),
            Some(PathBuf::from(
                "/home/op/beampipe/config/wallaby_hires.v2.yaml"
            ))
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
    fn setup_generates_a_grafana_password_for_a_new_install() {
        let password =
            select_grafana_admin_password(Some("replace-with-a-random-password"), false).unwrap();
        assert!(grafana_password_is_valid(&password));
        assert_ne!(password, "replace-with-a-random-password");
    }

    #[test]
    fn setup_refuses_an_existing_grafana_placeholder() {
        let error = select_grafana_admin_password(Some("replace-with-a-random-password"), true)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("BEAMPIPE_GRAFANA_ADMIN_PASSWORD"));
    }

    #[test]
    fn setup_preserves_an_existing_grafana_password() {
        let password = "an-existing-random-grafana-password";
        assert_eq!(
            select_grafana_admin_password(Some(password), true).unwrap(),
            password
        );
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
        assert_eq!(
            env_file_value(&env, "BEAMPIPE_SSH_CREDENTIALS_HOST").as_deref(),
            Some(root.path().join("credentials/ssh").to_string_lossy().as_ref())
        );
        assert_eq!(env_file_value(&env, "BEAMPIPE_SSH_CREDENTIALS_DIR"), None);
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
            core_home: Some(PathBuf::from("/home/op/beampipe")),
            project_file: Some("config/wallaby_hires.v2.yaml".into()),
            ..Default::default()
        });
        let joined = lines.join("\n");
        assert!(joined.contains("up -d postgres"));
        assert!(joined.contains("--home '/home/op/beampipe' migrate"));
        assert!(joined.contains("project add -f 'config/wallaby_hires.v2.yaml'"));
        assert!(joined.contains("--home '/home/op/beampipe' start"));
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
    fn skipped_project_upload_is_kept_in_the_recipe_after_migration() {
        let pending = next_steps_lines(&SetupNextSteps {
            runtime_docker: false,
            db_applied: true,
            admin_ready: true,
            project_uploaded: false,
            core_home: Some(PathBuf::from("/home/op/beampipe")),
            project_file: Some("config/project.yaml".into()),
            ..Default::default()
        })
        .join("\n");
        assert!(pending.contains("project add -f 'config/project.yaml'"));

        let uploaded = next_steps_lines(&SetupNextSteps {
            runtime_docker: false,
            db_applied: true,
            admin_ready: true,
            project_uploaded: true,
            project_file: Some("config/project.yaml".into()),
            ..Default::default()
        })
        .join("\n");
        assert!(!uploaded.contains("beampipe project add"));
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
        assert!(joined.contains("--home '/home/op/beampipe' start"));
        assert!(joined.contains("scripts/install.sh"));
        assert!(joined.contains("--core-home '/home/op/beampipe'"));
        assert!(joined.contains("--dash-dir '/home/op/beampipe-dash'"));
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
            core_home: Some(PathBuf::from("/home/op/beampipe")),
            project_file: Some("config/wallaby_hires.v2.yaml".into()),
            ..Default::default()
        });
        let joined = lines.join("\n");
        assert!(joined.contains("--home '/home/op/beampipe' start"));
        assert!(joined.contains("--home '/home/op/beampipe' migrate"));
        assert!(joined.contains("project add -f 'config/wallaby_hires.v2.yaml'"));
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
        assert_eq!(env_file_value(&env, "BEAMPIPE_API_PORT").as_deref(), Some("18181"));
        assert_eq!(
            env_file_value(&env, "BEAMPIPE_POSTGRES_PORT").as_deref(),
            Some("15432")
        );
        assert_eq!(
            env_file_value(&env, "BEAMPIPE_METRICS_PORT").as_deref(),
            Some("19090")
        );
        assert_eq!(
            env_file_value(&env, "BEAMPIPE_BIND_ADDR").as_deref(),
            Some("127.0.0.1:18181")
        );
        assert_eq!(
            env_file_value(&env, "BEAMPIPE_METRICS_BIND_ADDR").as_deref(),
            Some("127.0.0.1:19090")
        );
    }

    #[test]
    fn clear_missing_config_path_blanks_absent_yaml() {
        let root = tempfile::tempdir().unwrap();
        let env = root.path().join(".env");
        std::fs::write(&env, "BEAMPIPE_CONFIG=beampipe.yaml\n").unwrap();
        clear_missing_config_path(root.path(), &env).unwrap();
        assert_eq!(env_file_value(&env, "BEAMPIPE_CONFIG"), None);
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

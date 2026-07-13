use anyhow::{bail, Context, Result};
use beampipe_config::Settings;
use beampipe_db::repo;
use beampipe_profiles::{
    DaliugeManagerTopologyConfig, DaliugeTranslationConfig, DeploymentConfig, DeploymentProfile,
    RestRemoteDeploymentConfig, SlurmRemoteDeploymentConfig, SlurmResourceConfig,
};
use beampipe_project::ProjectConfig;
use sqlx::PgPool;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::doctor;

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
    pub dim_url: Option<String>,
    pub worker_pool: Option<String>,
    pub deployment: Option<String>,
    pub profile_name: Option<String>,
    pub facility: Option<String>,
    pub ssh_host: Option<String>,
    pub ssh_user: Option<String>,
    pub slurm_account: Option<String>,
    pub slurm_partition: Option<String>,
    pub dlg_root: Option<String>,
    pub remote_home: Option<String>,
    pub remote_logs: Option<String>,
    pub use_real_backends: bool,
    pub skip_admin: bool,
    pub skip_upload: bool,
}

#[derive(Debug)]
struct IntegrationSetup {
    casda_tap_url: String,
    tm_url: String,
    dim_url: Option<String>,
    worker_pool: String,
    use_real_backends: bool,
    profile: DeploymentProfile,
}

fn collect_integration_setup(opts: &SetupOptions) -> Result<IntegrationSetup> {
    let casda_tap_url = setup_value(
        opts.casda_tap_url.as_deref(),
        opts.yes,
        "CASDA TAP URL",
        "https://casda.csiro.au/casda_vo_tools/tap/sync",
    )?;
    let tm_url = setup_value(
        opts.tm_url.as_deref(),
        opts.yes,
        "DALiuGE Translator Manager URL",
        "http://localhost:9000",
    )?;
    let worker_pool = setup_value(
        opts.worker_pool.as_deref(),
        opts.yes,
        "Beampipe worker pool",
        "default",
    )?;
    let deployment_kind = setup_value(
        opts.deployment.as_deref(),
        opts.yes,
        "DALiuGE deployment strategy (rest_remote/slurm_remote)",
        "rest_remote",
    )?;
    if !matches!(deployment_kind.as_str(), "rest_remote" | "slurm_remote") {
        bail!("deployment strategy must be rest_remote or slurm_remote");
    }
    let profile_name = setup_value(
        opts.profile_name.as_deref(),
        opts.yes,
        "Deployment profile name",
        if deployment_kind == "slurm_remote" {
            "setonix"
        } else {
            "local-daliuge"
        },
    )?;
    let use_real_backends = if opts.use_real_backends || opts.yes {
        opts.use_real_backends
    } else {
        prompt_yes_no("Use live CASDA, DALiuGE, and scheduler backends?", false)?
    };

    let (deployment, dim_url) = if deployment_kind == "slurm_remote" {
        let facility = setup_value(
            opts.facility.as_deref(),
            opts.yes,
            "HPC facility",
            "setonix",
        )?;
        let login_node = required_setup_value(
            opts.ssh_host.as_deref(),
            opts.yes,
            "SLURM SSH login host",
            "setonix.pawsey.org.au",
        )?;
        let default_user = std::env::var("USER").unwrap_or_else(|_| "operator".into());
        let remote_user = required_setup_value(
            opts.ssh_user.as_deref(),
            opts.yes,
            "SLURM SSH user",
            &default_user,
        )?;
        let account =
            required_setup_value(opts.slurm_account.as_deref(), opts.yes, "SLURM account", "")?;
        let partition = setup_value(
            opts.slurm_partition.as_deref(),
            opts.yes,
            "SLURM partition",
            "work",
        )?;
        let default_home = format!("/scratch/{account}");
        let home_dir = setup_value(
            opts.remote_home.as_deref(),
            opts.yes,
            "Remote home/scratch path",
            &default_home,
        )?;
        let default_dlg_root = format!("{}/{remote_user}/dlg", home_dir.trim_end_matches('/'));
        let dlg_root = setup_value(
            opts.dlg_root.as_deref(),
            opts.yes,
            "Remote DLG_ROOT",
            &default_dlg_root,
        )?;
        let default_logs = format!("{}/log", dlg_root.trim_end_matches('/'));
        let log_dir = setup_value(
            opts.remote_logs.as_deref(),
            opts.yes,
            "Remote log directory",
            &default_logs,
        )?;
        (
            DeploymentConfig::SlurmRemote(SlurmRemoteDeploymentConfig {
                login_node,
                ssh_port: 22,
                remote_user: Some(remote_user),
                account,
                home_dir,
                log_dir,
                exec_prefix: "srun -l".into(),
                dlg_root,
                venv: None,
                modules: None,
                facility,
                job_duration_minutes: 60,
                num_nodes: 1,
                num_islands: 1,
                verbose_level: 1,
                max_threads: 0,
                all_nics: false,
                zerorun: false,
                sleepncopy: false,
                check_with_session: false,
                verify_ssl: Some(true),
                slurm_template: None,
                resources: SlurmResourceConfig {
                    partition: Some(partition),
                    nodes: Some(1),
                    tasks: Some(1),
                    cpus_per_task: Some(1),
                    memory: None,
                    wall_time_minutes: Some(60),
                    constraint: None,
                    quality_of_service: None,
                },
                manager_topology: DaliugeManagerTopologyConfig {
                    nodes: Some(1),
                    islands: Some(1),
                    co_host_dim: false,
                },
                container_runtime: None,
                environment_setup: None,
            }),
            opts.dim_url.clone(),
        )
    } else {
        let dim_url = setup_value(
            opts.dim_url.as_deref(),
            opts.yes,
            "DALiuGE Data Island Manager URL",
            "http://localhost:8001",
        )?;
        let (host, port, use_https) = parse_http_host_port(&dim_url)?;
        (
            DeploymentConfig::RestRemote(RestRemoteDeploymentConfig {
                dim_host_for_tm: Some(host.clone()),
                dim_port_for_tm: Some(port),
                deploy_host: Some(host),
                deploy_port: Some(port),
                use_https,
                verify_ssl: true,
            }),
            Some(dim_url),
        )
    };
    let profile = DeploymentProfile {
        name: profile_name,
        description: Some(format!("Generated by beampipe setup ({deployment_kind})")),
        project_module: Some("wallaby_hires".into()),
        is_default: true,
        max_concurrent_executions: None,
        translation: DaliugeTranslationConfig {
            num_islands: if deployment_kind == "slurm_remote" {
                1
            } else {
                0
            },
            tm_url: Some(tm_url.clone()),
            ..Default::default()
        },
        deployment,
    };
    profile.validate()?;
    Ok(IntegrationSetup {
        casda_tap_url,
        tm_url,
        dim_url,
        worker_pool,
        use_real_backends,
        profile,
    })
}

fn setup_value(
    value: Option<&str>,
    non_interactive: bool,
    label: &str,
    default: &str,
) -> Result<String> {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        return Ok(value.trim().to_string());
    }
    if non_interactive {
        return Ok(default.to_string());
    }
    prompt_default(label, default)
}

fn required_setup_value(
    value: Option<&str>,
    non_interactive: bool,
    label: &str,
    default: &str,
) -> Result<String> {
    let value = setup_value(value, non_interactive, label, default)?;
    if value.trim().is_empty() {
        bail!("{label} is required");
    }
    Ok(value)
}

fn parse_http_host_port(value: &str) -> Result<(String, i32, bool)> {
    let value = value.trim().trim_end_matches('/');
    let (use_https, authority) = if let Some(authority) = value.strip_prefix("https://") {
        (true, authority)
    } else if let Some(authority) = value.strip_prefix("http://") {
        (false, authority)
    } else {
        (false, value)
    };
    if authority.contains('/') || authority.contains('?') || authority.contains('#') {
        bail!("DALiuGE manager URL must contain only a scheme, host, and optional port");
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') => {
            let port = port
                .parse::<i32>()
                .with_context(|| format!("invalid DALiuGE manager port '{port}'"))?;
            (host, port)
        }
        _ => (authority, if use_https { 443 } else { 8001 }),
    };
    if host.trim().is_empty() || !(1..=65535).contains(&port) {
        bail!("DALiuGE manager URL has an invalid host or port");
    }
    Ok((host.to_string(), port, use_https))
}

fn install_profile_file(
    root: &Path,
    profile: &DeploymentProfile,
) -> Result<(PathBuf, DeploymentProfile)> {
    profile.validate()?;
    let path = root
        .join("config")
        .join(format!("deployment_profile.{}.json", profile.name));
    if path.exists() {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read existing profile {}", path.display()))?;
        let existing: DeploymentProfile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse existing profile {}", path.display()))?;
        existing.validate()?;
        if serde_json::to_value(&existing)? != serde_json::to_value(profile)? {
            bail!(
                "{} already contains a different profile; setup will not overwrite it",
                path.display()
            );
        }
        return Ok((path, existing));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(profile)? + "\n")
        .with_context(|| format!("write profile {}", path.display()))?;
    Ok((path, profile.clone()))
}

async fn install_profile_record(pool: &PgPool, profile: &DeploymentProfile) -> Result<()> {
    if let Some(existing) = repo::get_deployment_profile_by_name(pool, &profile.name).await? {
        let existing_spec = serde_json::json!({
            "name": existing.name,
            "description": existing.description,
            "project_module": existing.project_module,
            "is_default": existing.is_default,
            "max_concurrent_executions": existing.max_concurrent_executions,
            "translation": existing.translation,
            "deployment": existing.deployment,
        });
        let requested_spec = serde_json::json!({
            "name": profile.name,
            "description": profile.description,
            "project_module": profile.project_module,
            "is_default": profile.is_default,
            "max_concurrent_executions": profile.max_concurrent_executions,
            "translation": profile.translation,
            "deployment": profile.deployment,
        });
        if existing_spec != requested_spec {
            bail!(
                "deployment profile '{}' already exists with different settings; update it explicitly",
                profile.name
            );
        }
        println!(
            "Deployment profile '{}' already installed; skipped.",
            profile.name
        );
        return Ok(());
    }
    repo::create_deployment_profile(
        pool,
        &profile.name,
        profile.description.as_deref(),
        profile.project_module.as_deref(),
        profile.is_default,
        profile.max_concurrent_executions,
        serde_json::to_value(&profile.translation)?,
        serde_json::to_value(&profile.deployment)?,
    )
    .await?;
    println!("Installed deployment profile '{}'.", profile.name);
    Ok(())
}

pub async fn run_setup(opts: SetupOptions) -> Result<()> {
    let root = std::env::current_dir().context("cwd")?;
    let env_path = root.join(".env");
    let template_path = root.join(".env.template");

    if !env_path.exists() {
        if template_path.exists() {
            std::fs::copy(&template_path, &env_path).context("copy .env.template to .env")?;
            println!("Created .env from .env.template");
        } else {
            std::fs::write(&env_path, default_env_skeleton()).context("write .env")?;
            println!("Created minimal .env");
        }
    } else if !opts.yes {
        print!("`.env` already exists. Continue without overwriting? [Y/n] ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if line.trim().eq_ignore_ascii_case("n") {
            bail!("setup aborted");
        }
    }

    let database_url = if let Some(url) = opts.database_url.clone() {
        url
    } else if opts.yes {
        std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/beampipe".into())
    } else {
        prompt_default(
            "DATABASE_URL",
            "postgres://postgres:postgres@localhost:5432/beampipe",
        )?
    };

    let jwt_secret = opts
        .jwt_secret
        .clone()
        .or_else(|| std::env::var("BEAMPIPE_JWT_SECRET").ok())
        .filter(|secret| secret.len() >= 32 && secret != "change-me")
        .unwrap_or_else(|| {
            if !opts.yes {
                println!("Generated a random JWT secret and stored it in .env.");
            }
            generate_jwt_secret()
        });

    let integration = collect_integration_setup(&opts)?;
    let (profile_path, deployment_profile) = install_profile_file(&root, &integration.profile)?;

    update_env_file(&env_path, "DATABASE_URL", &database_url)?;
    update_env_file(&env_path, "BEAMPIPE_JWT_SECRET", &jwt_secret)?;
    update_env_file(
        &env_path,
        "BEAMPIPE_CASDA_TAP_URL",
        &integration.casda_tap_url,
    )?;
    update_env_file(&env_path, "BEAMPIPE_TM_URL", &integration.tm_url)?;
    if let Some(dim_url) = integration.dim_url.as_deref() {
        update_env_file(&env_path, "BEAMPIPE_DIM_URL", dim_url)?;
    }
    update_env_file(&env_path, "BEAMPIPE_WORKER_POOL", &integration.worker_pool)?;
    update_env_file(
        &env_path,
        "BEAMPIPE_USE_REAL_BACKENDS",
        if integration.use_real_backends {
            "true"
        } else {
            "false"
        },
    )?;

    std::env::set_var("DATABASE_URL", &database_url);
    std::env::set_var("BEAMPIPE_JWT_SECRET", &jwt_secret);
    std::env::set_var("BEAMPIPE_CASDA_TAP_URL", &integration.casda_tap_url);
    std::env::set_var("BEAMPIPE_TM_URL", &integration.tm_url);
    std::env::set_var("BEAMPIPE_WORKER_POOL", &integration.worker_pool);
    std::env::set_var(
        "BEAMPIPE_USE_REAL_BACKENDS",
        integration.use_real_backends.to_string(),
    );
    if let Some(dim_url) = integration.dim_url.as_deref() {
        std::env::set_var("BEAMPIPE_DIM_URL", dim_url);
    }

    let pool = beampipe_db::connect(&database_url)
        .await
        .context("database connect")?;
    beampipe_db::migrate(&pool).await.context("migrate")?;
    println!("Migrations applied.");

    install_profile_record(&pool, &deployment_profile).await?;
    println!("Deployment profile: {}", profile_path.display());

    if !opts.skip_admin {
        let (username, password, email) = if opts.yes {
            let password = opts
                .admin_password
                .clone()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "--admin-password is required with --yes when creating admin user"
                    )
                })?;
            (
                opts.admin_user.clone().unwrap_or_else(|| "admin".into()),
                password,
                opts.admin_email
                    .clone()
                    .unwrap_or_else(|| "admin@example.test".into()),
            )
        } else {
            let username = prompt_default("Admin username", "admin")?;
            let password = rpassword::prompt_password("Admin password: ")?;
            let email = prompt_default("Admin email", "admin@example.test")?;
            (username, password, email)
        };

        if password.len() < 12 {
            bail!("admin password must be at least 12 characters");
        }

        if repo::get_user_by_username(&pool, &username)
            .await?
            .is_none()
        {
            let hash = beampipe_auth::hash_password(&password)?;
            repo::create_user(&pool, "Admin", &username, &email, &hash, true).await?;
            println!("Created admin user '{username}'.");
        } else {
            println!("Admin user '{username}' already exists; skipped.");
        }
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
            upload_project_config(&pool, &config, &report.spec_sha256).await?;
            println!("Uploaded project config '{}'.", config.metadata.id);
        }
    } else {
        println!(
            "Project config not found at {}; skipped validate/upload.",
            project_path.display()
        );
    }

    let settings = Settings::load()?.settings;
    let report = doctor::run_doctor(&pool, &settings, None, Vec::new()).await;
    doctor::print_human(&report);
    if !report.ok {
        bail!("setup completed with doctor failures; fix checks above");
    }

    print_next_steps();
    Ok(())
}

pub async fn run_setup_check(json: bool, profile: Option<&str>, fix: bool) -> Result<()> {
    let mut fixes_applied = Vec::new();
    if fix {
        let config_dir = std::env::current_dir()?.join("config");
        if !config_dir.exists() {
            std::fs::create_dir_all(&config_dir)
                .with_context(|| format!("create {}", config_dir.display()))?;
            fixes_applied.push(format!("created {}", config_dir.display()));
        }
    }

    let settings = Settings::load()?.settings;
    let pool = match beampipe_db::connect(&settings.database_url).await {
        Ok(pool) => pool,
        Err(error) => {
            let report =
                doctor::DoctorReport::database_unreachable(&error.to_string(), fixes_applied);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                doctor::print_human(&report);
            }
            std::process::exit(1);
        }
    };
    let report = doctor::run_doctor(&pool, &settings, profile, fixes_applied).await;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        doctor::print_human(&report);
    }
    if !report.ok {
        std::process::exit(1);
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
    "BEAMPIPE_ENV=development\nBEAMPIPE_JWT_SECRET=change-me\nDATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe\n".into()
}

fn print_next_steps() {
    println!("\nSetup complete. Next steps:");
    println!("  beampipe doctor");
    println!("  beampipe serve --worker false    # API only");
    println!("  beampipe worker                  # workers (BEAMPIPE_WORKER_SCHEDULER_ENABLED=false on replicas)");
    println!("  docker compose up -d             # Postgres + stack");
    println!("  See deploy/ssh/README.md for Slurm SSH when BEAMPIPE_USE_REAL_BACKENDS=true");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_interactive_rest_setup_builds_a_tls_verifying_profile() {
        let integration = collect_integration_setup(&SetupOptions {
            yes: true,
            tm_url: Some("https://tm.example.org".into()),
            dim_url: Some("https://dim.example.org:8443".into()),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(integration.tm_url, "https://tm.example.org");
        assert_eq!(
            integration.dim_url.as_deref(),
            Some("https://dim.example.org:8443")
        );
        let DeploymentConfig::RestRemote(deployment) = integration.profile.deployment else {
            panic!("expected REST deployment profile");
        };
        assert!(deployment.use_https);
        assert!(deployment.verify_ssl);
        assert_eq!(deployment.deploy_host.as_deref(), Some("dim.example.org"));
        assert_eq!(deployment.deploy_port, Some(8443));
    }

    #[test]
    fn slurm_setup_requires_an_account_in_non_interactive_mode() {
        let error = collect_integration_setup(&SetupOptions {
            yes: true,
            deployment: Some("slurm_remote".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("SLURM account is required"));
    }

    #[test]
    fn profile_install_is_idempotent_and_refuses_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let profile = collect_integration_setup(&SetupOptions {
            yes: true,
            ..Default::default()
        })
        .unwrap()
        .profile;

        let (path, _) = install_profile_file(root.path(), &profile).unwrap();
        let original = std::fs::read(&path).unwrap();
        install_profile_file(root.path(), &profile).unwrap();

        let mut changed = profile;
        changed.description = Some("different operator-owned profile".into());
        let error = install_profile_file(root.path(), &changed).unwrap_err();
        assert!(error.to_string().contains("will not overwrite"));
        assert_eq!(std::fs::read(path).unwrap(), original);
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
}

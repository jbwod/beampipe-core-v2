use anyhow::{bail, Context, Result};
use beampipe_config::Settings;
use beampipe_db::repo;
use beampipe_project::ProjectConfig;
use sqlx::PgPool;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::doctor;

#[derive(Debug, Clone)]
pub struct SetupOptions {
    pub yes: bool,
    pub database_url: Option<String>,
    pub jwt_secret: Option<String>,
    pub admin_user: Option<String>,
    pub admin_password: Option<String>,
    pub admin_email: Option<String>,
    pub project_config: Option<PathBuf>,
    pub skip_admin: bool,
    pub skip_upload: bool,
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

    let jwt_secret = if let Some(s) = opts.jwt_secret.clone() {
        s
    } else if opts.yes {
        std::env::var("BEAMPIPE_JWT_SECRET").unwrap_or_else(|_| generate_jwt_secret())
    } else {
        let generated = generate_jwt_secret();
        prompt_default("BEAMPIPE_JWT_SECRET", &generated)?
    };

    update_env_file(&env_path, "DATABASE_URL", &database_url)?;
    update_env_file(&env_path, "BEAMPIPE_JWT_SECRET", &jwt_secret)?;

    std::env::set_var("DATABASE_URL", &database_url);
    std::env::set_var("BEAMPIPE_JWT_SECRET", &jwt_secret);

    let pool = beampipe_db::connect(&database_url)
        .await
        .context("database connect")?;
    beampipe_db::migrate(&pool).await.context("migrate")?;
    println!("Migrations applied.");

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
            let password = prompt_default("Admin password", "change-me")?;
            let email = prompt_default("Admin email", "admin@example.test")?;
            (username, password, email)
        };

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

    let settings = Settings::from_env()?;
    let report = doctor::run_doctor(&pool, &settings).await;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.ok {
        bail!("setup completed with doctor failures; fix checks above");
    }

    print_next_steps();
    Ok(())
}

pub async fn run_setup_check() -> Result<()> {
    let settings = Settings::from_env()?;
    let pool = beampipe_db::connect(&settings.database_url).await?;
    let report = doctor::run_doctor(&pool, &settings).await;
    println!("{}", serde_json::to_string_pretty(&report)?);
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
    Uuid::now_v7().to_string() + &Uuid::now_v7().to_string()
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

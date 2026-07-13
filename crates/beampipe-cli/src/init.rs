use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct InitOptions {
    pub directory: PathBuf,
    pub force: bool,
    pub production: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct InitReport {
    pub directory: PathBuf,
    pub created: Vec<PathBuf>,
    pub replaced: Vec<PathBuf>,
    pub next_steps: Vec<String>,
}

pub fn run(options: InitOptions) -> Result<InitReport> {
    let root = absolute_path(&options.directory)?;
    let config_dir = root.join("config");
    let targets = [
        root.join("beampipe.yaml"),
        root.join(".env.example"),
        config_dir.join("beampipe.development.yaml"),
        config_dir.join("beampipe.production.yaml"),
    ];
    if !options.force {
        let existing: Vec<_> = targets.iter().filter(|path| path.exists()).collect();
        if !existing.is_empty() {
            bail!(
                "configuration already exists at {}; use --force only when replacement is intentional",
                existing
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("create {}", config_dir.display()))?;

    let mut report = InitReport {
        directory: root.clone(),
        created: Vec::new(),
        replaced: Vec::new(),
        next_steps: vec![
            "review beampipe.yaml and .env.example".into(),
            "set BEAMPIPE_JWT_SECRET outside committed configuration".into(),
            "run `beampipe doctor --json`".into(),
            "run `beampipe setup` to migrate PostgreSQL and create an administrator".into(),
        ],
    };

    write_managed(
        &root.join("beampipe.yaml"),
        if options.production {
            production_config()
        } else {
            development_config()
        },
        options.force,
        &mut report,
    )?;
    write_managed(
        &root.join(".env.example"),
        if options.production {
            production_env()
        } else {
            development_env()
        },
        options.force,
        &mut report,
    )?;
    write_managed(
        &config_dir.join("beampipe.development.yaml"),
        development_config(),
        options.force,
        &mut report,
    )?;
    write_managed(
        &config_dir.join("beampipe.production.yaml"),
        production_config(),
        options.force,
        &mut report,
    )?;
    Ok(report)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("current directory")?
            .join(path))
    }
}

fn write_managed(path: &Path, content: &str, force: bool, report: &mut InitReport) -> Result<()> {
    let existed = path.exists();
    if existed && !force {
        bail!(
            "{} already exists; use --force only when replacement is intentional",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    if existed {
        report.replaced.push(path.to_path_buf());
    } else {
        report.created.push(path.to_path_buf());
    }
    Ok(())
}

fn development_config() -> &'static str {
    r#"apiVersion: beampipe.dev/config/v1
kind: BeampipeConfig
environment: development
database:
  url: postgres://postgres:postgres@localhost:5432/beampipe
  max_connections: 10
api:
  bind_addr: 127.0.0.1:8080
auth:
  jwt_secret: replace-with-at-least-32-random-characters
worker:
  instance_name: null
  pool: default
  concurrency: 2
  heartbeat_interval_seconds: 10
  lock_seconds: 60
  scheduler_enabled: true
  capabilities:
    - casda-discovery
    - manifest-generation
    - daliuge-translation
    - daliuge-deployment
    - slurm-remote
    - output-verification
integrations:
  use_real_backends: false
  casda_tap_url: null
  vizier_tap_url: null
  tm_url: http://localhost:9000
  dim_url: http://localhost:8001
metrics:
  server_enabled: true
telemetry:
  log_json: false
"#
}

fn production_config() -> &'static str {
    r#"apiVersion: beampipe.dev/config/v1
kind: BeampipeConfig
environment: production
database:
  url: null
  max_connections: 20
api:
  bind_addr: 0.0.0.0:8080
auth:
  jwt_secret: null
worker:
  instance_name: null
  pool: default
  concurrency: 4
  heartbeat_interval_seconds: 10
  lock_seconds: 60
  scheduler_enabled: true
  capabilities:
    - casda-discovery
    - manifest-generation
    - daliuge-translation
    - daliuge-deployment
    - slurm-remote
    - output-verification
integrations:
  use_real_backends: true
  casda_tap_url: null
  vizier_tap_url: null
  tm_url: null
  dim_url: null
metrics:
  server_enabled: true
telemetry:
  log_json: true
"#
}

fn development_env() -> &'static str {
    r#"# Copy to .env and replace the development-only values before sharing the environment.
DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
BEAMPIPE_JWT_SECRET=replace-with-at-least-32-random-characters
BEAMPIPE_CONFIG=beampipe.yaml
"#
}

fn production_env() -> &'static str {
    r#"# Production values must be supplied by your secret manager or deployment environment.
DATABASE_URL=<secret-reference-or-runtime-value>
BEAMPIPE_JWT_SECRET=<secret-reference-or-runtime-value>
BEAMPIPE_CONFIG=beampipe.yaml
BEAMPIPE_CASDA_TAP_URL=<casda-tap-url>
BEAMPIPE_TM_URL=<daliuge-translator-url>
BEAMPIPE_DIM_URL=<daliuge-manager-url>
SLURM_SSH_PRIVATE_KEY_PATH=<runtime-mounted-private-key>
SLURM_SSH_KNOWN_HOSTS=<runtime-mounted-known-hosts>
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_does_not_replace_owned_files_without_force() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("beampipe.yaml"), "owned").unwrap();
        let error = run(InitOptions {
            directory: dir.path().into(),
            force: false,
            production: false,
        })
        .unwrap_err();
        assert!(error.to_string().contains("--force"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("beampipe.yaml")).unwrap(),
            "owned"
        );
    }

    #[test]
    fn production_template_contains_no_generated_secret() {
        assert!(production_config().contains("jwt_secret: null"));
        assert!(!production_config().contains("change-me"));
    }
}

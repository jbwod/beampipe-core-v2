use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const OPERATOR_COMPOSE: &str = include_str!("../../../deploy/operator/docker-compose.yml");
const OPERATOR_ENV_EXAMPLE: &str = include_str!("../../../deploy/operator/.env.example");
const SAMPLE_PROJECT: &str = include_str!("../../../config/wallaby_hires.v2.yaml");

#[derive(Debug, Default, Clone)]
pub struct MaterializeReport {
    pub created: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
    pub replaced: Vec<PathBuf>,
}

pub fn materialize(root: &Path, force: bool) -> Result<MaterializeReport> {
    let mut report = MaterializeReport::default();
    std::fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;

    write_file(
        &root.join("docker-compose.yml"),
        OPERATOR_COMPOSE,
        force,
        &mut report,
    )?;
    write_file(
        &root.join(".env.example"),
        OPERATOR_ENV_EXAMPLE,
        force,
        &mut report,
    )?;

    let config_dir = root.join("config");
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("create {}", config_dir.display()))?;
    write_file(
        &config_dir.join("wallaby_hires.v2.yaml"),
        SAMPLE_PROJECT,
        force,
        &mut report,
    )?;

    let credentials_dir = root.join("credentials/ssh");
    std::fs::create_dir_all(&credentials_dir)
        .with_context(|| format!("create {}", credentials_dir.display()))?;
    write_file(&credentials_dir.join(".gitkeep"), "", force, &mut report)?;

    Ok(report)
}

fn write_file(
    path: &Path,
    contents: &str,
    force: bool,
    report: &mut MaterializeReport,
) -> Result<()> {
    if path.exists() && !force {
        report.skipped.push(path.to_path_buf());
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let existed = path.exists();
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    if existed {
        report.replaced.push(path.to_path_buf());
    } else {
        report.created.push(path.to_path_buf());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use beampipe_project::ProjectConfig;

    #[test]
    fn materialize_writes_pull_only_compose_and_sample_project() {
        let dir = tempfile::tempdir().unwrap();
        let report = materialize(dir.path(), false).unwrap();
        assert!(report
            .created
            .iter()
            .any(|path| path.ends_with("docker-compose.yml")));

        let compose = std::fs::read_to_string(dir.path().join("docker-compose.yml")).unwrap();
        assert!(!compose.contains("build:"));
        assert!(!compose.contains("Dockerfile"));
        assert!(compose.contains("ghcr.io/jbwod/beampipe-core-v2"));
        assert!(dir.path().join("credentials/ssh/.gitkeep").exists());

        let project = std::fs::read(dir.path().join("config/wallaby_hires.v2.yaml")).unwrap();
        let config = ProjectConfig::from_slice(&project).unwrap();
        assert!(config.validate_report().valid);
        assert_eq!(config.metadata.id, "wallaby_hires");

        std::fs::write(dir.path().join("docker-compose.yml"), "owned\n").unwrap();
        let second = materialize(dir.path(), false).unwrap();
        assert!(second
            .skipped
            .iter()
            .any(|path| path.ends_with("docker-compose.yml")));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("docker-compose.yml")).unwrap(),
            "owned\n"
        );
    }
}

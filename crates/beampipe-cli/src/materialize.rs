use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

// Keep these copies inside the crate so `COPY crates` is enough for Docker
// builds. `embedded_bundle_tracks_repo_sources` fails CI if they drift.
const OPERATOR_COMPOSE: &str = include_str!("../embedded/docker-compose.yml");
const OPERATOR_ENV_EXAMPLE: &str = include_str!("../embedded/env.example");
const SAMPLE_PROJECT: &str = include_str!("../embedded/wallaby_hires.v2.yaml");
const SAMPLE_REST_PROFILE: &str = include_str!("../embedded/deployment_profile.dlg-dim.json");
const SAMPLE_SLURM_PROFILE: &str = include_str!("../embedded/deployment_profile.slurm-remote.json");
const BUNDLE_MANIFEST: &str = ".beampipe-operator-bundle.json";

#[derive(Debug, Default, Clone)]
pub struct MaterializeReport {
    pub created: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
    pub replaced: Vec<PathBuf>,
    pub bundle_current: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BundleManifest {
    schema_version: u32,
    bundle_version: String,
    files: BTreeMap<String, String>,
}

pub fn materialize(root: &Path, force: bool) -> Result<MaterializeReport> {
    let mut report = MaterializeReport {
        bundle_current: true,
        ..MaterializeReport::default()
    };
    fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
    let previous = read_manifest(root);
    let mut next = BundleManifest {
        schema_version: 1,
        bundle_version: env!("CARGO_PKG_VERSION").into(),
        files: BTreeMap::new(),
    };

    for (relative, contents) in [
        ("docker-compose.yml", OPERATOR_COMPOSE),
        (".env.example", OPERATOR_ENV_EXAMPLE),
        ("config/wallaby_hires.v2.yaml", SAMPLE_PROJECT),
        (
            "config/deployment_profile.dlg-dim.json",
            SAMPLE_REST_PROFILE,
        ),
        (
            "config/deployment_profile.slurm-remote.json",
            SAMPLE_SLURM_PROFILE,
        ),
        ("credentials/ssh/.gitkeep", ""),
    ] {
        materialize_file(
            root,
            relative,
            contents,
            force,
            previous.as_ref(),
            &mut next,
            &mut report,
        )?;
    }
    write_manifest(root, &next)?;
    Ok(report)
}

fn materialize_file(
    root: &Path,
    relative: &str,
    contents: &str,
    force: bool,
    previous: Option<&BundleManifest>,
    next: &mut BundleManifest,
    report: &mut MaterializeReport,
) -> Result<()> {
    let path = root.join(relative);
    let desired_hash = sha256(contents.as_bytes());
    let actual_hash = fs::read(&path).ok().map(|bytes| sha256(&bytes));
    let previous_hash = previous.and_then(|manifest| manifest.files.get(relative));
    let managed = force
        || actual_hash.is_none()
        || actual_hash.as_deref() == Some(desired_hash.as_str())
        || previous_hash.is_some_and(|hash| actual_hash.as_deref() == Some(hash.as_str()));

    if managed {
        if actual_hash.as_deref() == Some(desired_hash.as_str()) {
            report.skipped.push(path.clone());
        } else {
            let existed = path.exists();
            atomic_write(&path, contents.as_bytes())?;
            if existed {
                report.replaced.push(path.clone());
            } else {
                report.created.push(path.clone());
            }
        }
    } else {
        report.skipped.push(path.clone());
        report.bundle_current = false;
    }
    next.files.insert(relative.into(), desired_hash);
    Ok(())
}

fn read_manifest(root: &Path) -> Option<BundleManifest> {
    let bytes = fs::read(root.join(BUNDLE_MANIFEST)).ok()?;
    let manifest: BundleManifest = serde_json::from_slice(&bytes).ok()?;
    (manifest.schema_version == 1).then_some(manifest)
}

fn write_manifest(root: &Path, manifest: &BundleManifest) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    atomic_write(&root.join(BUNDLE_MANIFEST), &bytes)
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("managed file has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bundle"),
        Uuid::new_v4().simple()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use beampipe_project::ProjectConfig;

    #[test]
    fn materialize_writes_pull_only_compose_and_preserves_operator_edits() {
        let dir = tempfile::tempdir().unwrap();
        let report = materialize(dir.path(), false).unwrap();
        assert!(report.bundle_current);
        assert!(report
            .created
            .iter()
            .any(|path| path.ends_with("docker-compose.yml")));
        assert!(dir.path().join(BUNDLE_MANIFEST).is_file());

        let compose = fs::read_to_string(dir.path().join("docker-compose.yml")).unwrap();
        assert!(!compose.contains("build:"));
        assert!(compose.contains("ghcr.io/jbwod/beampipe-core-v2"));

        let project = fs::read(dir.path().join("config/wallaby_hires.v2.yaml")).unwrap();
        let config = ProjectConfig::from_slice(&project).unwrap();
        assert!(config.validate_report().valid);

        fs::write(dir.path().join("docker-compose.yml"), "operator-owned\n").unwrap();
        let second = materialize(dir.path(), false).unwrap();
        assert!(!second.bundle_current);
        assert_eq!(
            fs::read_to_string(dir.path().join("docker-compose.yml")).unwrap(),
            "operator-owned\n"
        );
    }

    #[test]
    fn embedded_bundle_tracks_repo_sources() {
        assert_eq!(
            OPERATOR_COMPOSE,
            include_str!("../../../deploy/operator/docker-compose.yml")
        );
        assert_eq!(
            OPERATOR_ENV_EXAMPLE,
            include_str!("../../../deploy/operator/.env.example")
        );
        assert_eq!(
            SAMPLE_PROJECT,
            include_str!("../../../config/wallaby_hires.v2.yaml")
        );
        assert_eq!(
            SAMPLE_REST_PROFILE,
            include_str!("../../../config/deployment_profile.dlg-dim.json")
        );
        assert_eq!(
            SAMPLE_SLURM_PROFILE,
            include_str!("../../../config/deployment_profile.slurm-remote.json")
        );
    }

    #[test]
    fn materialize_updates_an_unmodified_managed_file() {
        let dir = tempfile::tempdir().unwrap();
        materialize(dir.path(), false).unwrap();
        let mut manifest = read_manifest(dir.path()).unwrap();
        manifest.files.insert(
            "docker-compose.yml".into(),
            sha256(b"old managed compose\n"),
        );
        write_manifest(dir.path(), &manifest).unwrap();
        fs::write(
            dir.path().join("docker-compose.yml"),
            "old managed compose\n",
        )
        .unwrap();

        let report = materialize(dir.path(), false).unwrap();
        assert!(report.bundle_current);
        assert!(report
            .replaced
            .iter()
            .any(|path| path.ends_with("docker-compose.yml")));
    }
}

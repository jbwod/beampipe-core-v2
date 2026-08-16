use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const INSTALLATION_STATE_FILE: &str = "installation.json";
pub const INSTALLATION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    Docker,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationState {
    pub schema_version: u32,
    pub beampipe_version: String,
    pub runtime: RuntimeMode,
    pub database_mode: String,
    pub home: PathBuf,
    pub environment_file: PathBuf,
    pub config_file: PathBuf,
    pub credential_root: PathBuf,
    pub operator_bundle_version: String,
    pub compose_project: String,
}

#[derive(Debug, Clone)]
pub struct InstallationContext {
    pub home: PathBuf,
    pub environment_file: PathBuf,
    pub config_file: PathBuf,
    pub credential_root: PathBuf,
    pub state: Option<InstallationState>,
}

impl InstallationContext {
    pub fn resolve(explicit_home: Option<&Path>) -> Result<Self> {
        let home = resolve_home(explicit_home)?;
        Self::from_home(home)
    }

    pub fn from_home(home: PathBuf) -> Result<Self> {
        let state_path = home.join(INSTALLATION_STATE_FILE);
        let state = if state_path.is_file() {
            let bytes =
                fs::read(&state_path).with_context(|| format!("read {}", state_path.display()))?;
            let state: InstallationState = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", state_path.display()))?;
            validate_state(&home, &state)?;
            Some(state)
        } else {
            None
        };
        let environment_file = state
            .as_ref()
            .map(|state| state.environment_file.clone())
            .unwrap_or_else(|| home.join(".env"));
        let config_file = state
            .as_ref()
            .map(|state| state.config_file.clone())
            .unwrap_or_else(|| home.join("beampipe.yaml"));
        let credential_root = state
            .as_ref()
            .map(|state| state.credential_root.clone())
            .map(Ok)
            .unwrap_or_else(|| resolve_credential_root(&home, None))?;
        Ok(Self {
            home,
            environment_file,
            config_file,
            credential_root,
            state,
        })
    }

    pub fn exists(&self) -> bool {
        self.state.is_some()
            || self.environment_file.is_file()
            || self.config_file.is_file()
            || self.home.join("docker-compose.yml").is_file()
    }

    pub fn activate(&self) -> Result<()> {
        std::env::set_var("BEAMPIPE_HOME", &self.home);
        if self.environment_file.is_file() {
            dotenvy::from_path(&self.environment_file)
                .with_context(|| format!("load {}", self.environment_file.display()))?;
        }
        if std::env::var_os("BEAMPIPE_CONFIG").is_none() && self.config_file.is_file() {
            std::env::set_var("BEAMPIPE_CONFIG", &self.config_file);
        }
        if std::env::var("BEAMPIPE_SSH_CREDENTIALS_DIR")
            .ok()
            .is_none_or(|value| value.trim().is_empty())
        {
            std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", &self.credential_root);
        }
        Ok(())
    }
}

pub fn activate_if_selected(explicit_home: Option<&Path>) -> Result<Option<InstallationContext>> {
    let explicitly_selected = explicit_home.is_some() || non_empty_env("BEAMPIPE_HOME").is_some();
    let context = InstallationContext::resolve(explicit_home)?;
    if explicitly_selected || context.exists() {
        context.activate()?;
        Ok(Some(context))
    } else {
        Ok(None)
    }
}

pub fn resolve_home(explicit_home: Option<&Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("current directory")?;
    let selected = explicit_home
        .map(Path::to_path_buf)
        .or_else(|| non_empty_env("BEAMPIPE_HOME").map(PathBuf::from))
        .or_else(|| non_empty_env("HOME").map(|home| PathBuf::from(home).join("beampipe")))
        .ok_or_else(|| {
            anyhow::anyhow!("cannot resolve Beampipe home; set --home or BEAMPIPE_HOME")
        })?;
    absolute_path(&cwd, &selected)
}

pub fn resolve_credential_root(home: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return absolute_path(home, path);
    }
    if let Some(path) = non_empty_env("BEAMPIPE_SSH_CREDENTIALS_DIR") {
        return absolute_path(home, Path::new(&path));
    }

    let canonical = home.join("credentials/ssh");
    let legacy_install = home.join("deploy/ssh/credentials");
    let legacy_user = non_empty_env("HOME")
        .map(|user_home| PathBuf::from(user_home).join(".config/beampipe/credentials"));
    let mut populated = vec![canonical.clone(), legacy_install];
    if let Some(path) = legacy_user {
        if !populated.contains(&path) {
            populated.push(path);
        }
    }
    populated.retain(|path| credential_tree_has_material(path));
    match populated.as_slice() {
        [] => Ok(canonical),
        [path] => Ok(path.clone()),
        paths => bail!(
            "multiple non-empty SSH credential roots found: {}. Select one with --credentials-dir or BEAMPIPE_SSH_CREDENTIALS_DIR; Beampipe will not merge private keys automatically",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn write_state(home: &Path, state: &InstallationState) -> Result<()> {
    validate_state(home, state)?;
    fs::create_dir_all(home).with_context(|| format!("create {}", home.display()))?;
    let destination = home.join(INSTALLATION_STATE_FILE);
    let temporary = home.join(format!(
        ".{INSTALLATION_STATE_FILE}.tmp-{}",
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(state)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, &destination)
        .with_context(|| format!("replace {}", destination.display()))?;
    Ok(())
}

pub fn compose_project_name(home: &Path) -> String {
    let raw = home
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("beampipe");
    let normalized = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = normalized.trim_matches(['-', '_']);
    if trimmed.is_empty() {
        "beampipe".into()
    } else {
        trimmed.into()
    }
}

fn validate_state(home: &Path, state: &InstallationState) -> Result<()> {
    if state.schema_version != INSTALLATION_SCHEMA_VERSION {
        bail!(
            "unsupported installation state schema {}; expected {}",
            state.schema_version,
            INSTALLATION_SCHEMA_VERSION
        );
    }
    if state.home != home {
        bail!(
            "installation state home {} does not match selected home {}",
            state.home.display(),
            home.display()
        );
    }
    for (name, path) in [
        ("environment_file", &state.environment_file),
        ("config_file", &state.config_file),
        ("credential_root", &state.credential_root),
    ] {
        if !path.is_absolute() {
            bail!("installation state {name} must be an absolute path");
        }
    }
    Ok(())
}

fn absolute_path(cwd: &Path, path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    if absolute.exists() {
        absolute
            .canonicalize()
            .with_context(|| format!("resolve {}", absolute.display()))
    } else {
        Ok(absolute)
    }
}

fn credential_tree_has_material(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if entry.file_name() == ".gitkeep" {
            return false;
        }
        if path.is_dir() {
            credential_tree_has_material(&path)
        } else {
            path.is_file()
        }
    })
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(home: &Path, credential_root: &Path) -> InstallationState {
        InstallationState {
            schema_version: INSTALLATION_SCHEMA_VERSION,
            beampipe_version: "0.1.0".into(),
            runtime: RuntimeMode::Docker,
            database_mode: "compose".into(),
            home: home.to_path_buf(),
            environment_file: home.join(".env"),
            config_file: home.join("beampipe.yaml"),
            credential_root: credential_root.to_path_buf(),
            operator_bundle_version: "0.1.0".into(),
            compose_project: "beampipe".into(),
        }
    }

    #[test]
    fn explicit_home_is_absolute_and_independent_of_compose_files() {
        let cwd = tempfile::tempdir().unwrap();
        fs::write(cwd.path().join("docker-compose.yml"), "services: {}\n").unwrap();
        let selected = tempfile::tempdir().unwrap();
        assert_eq!(
            absolute_path(cwd.path(), selected.path()).unwrap(),
            selected.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn state_round_trip_preserves_absolute_paths() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let expected = state(&home, &home.join("credentials/ssh"));
        write_state(&home, &expected).unwrap();
        let context = InstallationContext::from_home(home).unwrap();
        assert_eq!(context.state, Some(expected));
    }

    #[test]
    fn legacy_credential_root_is_selected_only_when_populated() {
        let home = tempfile::tempdir().unwrap();
        let legacy = home.path().join("deploy/ssh/credentials/setonix");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("private_key"), "test-key").unwrap();
        assert_eq!(
            resolve_credential_root(home.path(), None).unwrap(),
            home.path().join("deploy/ssh/credentials")
        );
    }

    #[test]
    fn multiple_populated_credential_roots_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        for root in [
            home.path().join("credentials/ssh/a"),
            home.path().join("deploy/ssh/credentials/b"),
        ] {
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("private_key"), "test-key").unwrap();
        }
        let error = resolve_credential_root(home.path(), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("multiple non-empty SSH credential roots"));
    }
}

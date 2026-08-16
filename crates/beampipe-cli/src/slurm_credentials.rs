//! Operator tool for per-slot Slurm SSH credential files.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONTAINER_UID: &str = "10001";

#[derive(Debug, Clone)]
pub struct InitOptions {
    pub slot: String,
    pub dir: Option<PathBuf>,
    pub host: String,
    pub user: Option<String>,
    pub passphrase_file: Option<PathBuf>,
    pub no_passphrase: bool,
    pub copy_id: bool,
    pub acl: bool,
    pub force: bool,
    pub yes: bool,
    pub skip_keyscan: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            slot: "setonix".into(),
            dir: None,
            host: "setonix.pawsey.org.au".into(),
            user: None,
            passphrase_file: None,
            no_passphrase: false,
            copy_id: false,
            acl: false,
            force: false,
            yes: false,
            skip_keyscan: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InitResult {
    pub root: PathBuf,
    pub slot: String,
    pub private_key: PathBuf,
    pub public_key: PathBuf,
    pub passphrase: Option<PathBuf>,
    pub known_hosts: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileStatus {
    pub path: PathBuf,
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SlotStatus {
    pub slot: String,
    pub root: PathBuf,
    pub private_key: FileStatus,
    pub public_key: FileStatus,
    pub passphrase: FileStatus,
    pub known_hosts: FileStatus,
}

pub fn default_credentials_root() -> PathBuf {
    if let Ok(path) = std::env::var("BEAMPIPE_SSH_CREDENTIALS_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let deploy = PathBuf::from("deploy/ssh/credentials");
    if deploy.is_dir() {
        return deploy;
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join(".config/beampipe/credentials");
        }
    }
    deploy
}

pub fn resolve_root(dir: Option<&Path>) -> PathBuf {
    dir.map(Path::to_path_buf)
        .unwrap_or_else(default_credentials_root)
}

fn validate_slot(slot: &str) -> Result<()> {
    beampipe_profiles::validate_ssh_credential_name(slot)
        .map_err(|error| anyhow::anyhow!("{error}"))
}

pub fn init(opts: InitOptions) -> Result<InitResult> {
    validate_slot(&opts.slot)?;
    if opts.yes && !opts.no_passphrase && opts.passphrase_file.is_none() {
        bail!("--yes requires --no-passphrase or --passphrase-file (do not pass the passphrase on the command line)");
    }

    let root = resolve_root(opts.dir.as_deref());
    let slot_dir = root.join(&opts.slot);
    fs::create_dir_all(&slot_dir)
        .with_context(|| format!("create {}", slot_dir.display()))?;
    set_dir_mode(&slot_dir, 0o700)?;

    let private_key = slot_dir.join("private_key");
    let public_key = slot_dir.join("private_key.pub");
    let passphrase_path = slot_dir.join("passphrase");
    if private_key.exists() && !opts.force {
        bail!(
            "{} already exists; pass --force to replace it",
            private_key.display()
        );
    }

    let passphrase = resolve_passphrase_input(&opts)?;
    generate_ed25519_key(&private_key, passphrase.as_deref())?;
    set_file_mode(&private_key, 0o600)?;

    let wrote_passphrase = if let Some(secret) = passphrase.as_deref() {
        write_secret_file(&passphrase_path, secret)?;
        true
    } else if passphrase_path.exists() && opts.force {
        let _ = fs::remove_file(&passphrase_path);
        false
    } else {
        false
    };

    let known_hosts = root.join("known_hosts");
    if !opts.skip_keyscan {
        write_known_hosts(&known_hosts, &opts.host)?;
    } else if !known_hosts.exists() {
        fs::write(&known_hosts, "").with_context(|| format!("write {}", known_hosts.display()))?;
    }

    if opts.acl {
        apply_container_acl(&private_key)?;
        if wrote_passphrase {
            apply_container_acl(&passphrase_path)?;
        }
    }
    if opts.copy_id {
        let user = opts.user.as_deref().unwrap_or("");
        if user.is_empty() {
            bail!("--copy-id requires --user");
        }
        copy_id(&public_key, user, &opts.host)?;
    }

    Ok(InitResult {
        root,
        slot: opts.slot,
        private_key,
        public_key,
        passphrase: wrote_passphrase.then_some(passphrase_path),
        known_hosts,
    })
}

pub fn list_slots(dir: Option<&Path>) -> Result<Vec<String>> {
    let root = resolve_root(dir);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = fs::read_dir(&root)
        .with_context(|| format!("read {}", root.display()))?
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| beampipe_profiles::validate_ssh_credential_name(name).is_ok())
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

pub fn show(slot: &str, dir: Option<&Path>) -> Result<SlotStatus> {
    validate_slot(slot)?;
    let root = resolve_root(dir);
    let slot_dir = root.join(slot);
    Ok(SlotStatus {
        slot: slot.to_string(),
        root: root.clone(),
        private_key: file_status(&slot_dir.join("private_key")),
        public_key: file_status(&slot_dir.join("private_key.pub")),
        passphrase: file_status(&slot_dir.join("passphrase")),
        known_hosts: file_status(&slot_dir.join("known_hosts")).or_else_present(&root.join("known_hosts")),
    })
}

pub fn check(slot: &str, dir: Option<&Path>) -> Result<SlotStatus> {
    let status = show(slot, dir)?;
    if !status.private_key.present {
        bail!(
            "private_key missing at {}",
            status.private_key.path.display()
        );
    }
    let previous = std::env::var("BEAMPIPE_SSH_CREDENTIALS_DIR").ok();
    std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", &status.root);
    let result = (|| {
        let creds = beampipe_orchestration::SlurmSshCredentials::resolve_for(Some(slot))
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        creds
            .load_private_key()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        Ok::<_, anyhow::Error>(())
    })();
    match previous {
        Some(value) => std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", value),
        None => std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR"),
    }
    result?;
    Ok(status)
}

pub fn print_init_next_steps(result: &InitResult) {
    println!("Created SSH credential slot '{}'.", result.slot);
    println!("  private_key  {}", result.private_key.display());
    if let Some(path) = &result.passphrase {
        println!("  passphrase   {}", path.display());
    }
    println!("  known_hosts  {}", result.known_hosts.display());
    println!("\nNext:");
    println!("  set deployment.ssh_credential to \"{}\"", result.slot);
    println!(
        "  beampipe slurm credentials check --slot {}",
        result.slot
    );
    println!("  beampipe slurm ping --profile slurm-remote");
    println!(
        "  Container uid is 10001; re-run with --acl or `setfacl -m u:10001:r` on private_key and passphrase so Compose can read them."
    );
}

fn resolve_passphrase_input(opts: &InitOptions) -> Result<Option<String>> {
    if opts.no_passphrase {
        return Ok(None);
    }
    if let Some(path) = &opts.passphrase_file {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read passphrase file {}", path.display()))?;
        let secret = raw.trim_end_matches(['\r', '\n']).to_string();
        if secret.is_empty() {
            bail!("passphrase file {} is empty", path.display());
        }
        return Ok(Some(secret));
    }
    if opts.yes {
        bail!("--yes requires --no-passphrase or --passphrase-file");
    }
    if !stdin_is_tty() {
        bail!("passphrase prompt requires a TTY; use --passphrase-file or --no-passphrase");
    }
    let first = rpassword::prompt_password("SSH key passphrase (empty for none): ")?;
    if first.is_empty() {
        return Ok(None);
    }
    let second = rpassword::prompt_password("Confirm passphrase: ")?;
    if first != second {
        bail!("passphrases do not match");
    }
    Ok(Some(first))
}

fn generate_ed25519_key(path: &Path, passphrase: Option<&str>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("pub"));
    }
    let status = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-f",
            path.to_str().context("private key path is not UTF-8")?,
            "-N",
            passphrase.unwrap_or(""),
            "-C",
            &format!("beampipe-{}@{}", path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("slurm"), hostname()),
            "-q",
        ])
        .status()
        .context("run ssh-keygen (install OpenSSH client)")?;
    if !status.success() {
        bail!("ssh-keygen failed with status {status}");
    }
    Ok(())
}

fn write_known_hosts(path: &Path, host: &str) -> Result<()> {
    let output = Command::new("ssh-keyscan")
        .args(["-t", "ed25519", "-T", "8", host])
        .output()
        .context("run ssh-keyscan (install OpenSSH client)")?;
    if !output.status.success() || output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("ssh-keyscan failed for {host}: {stderr}");
    }
    let scanned = String::from_utf8(output.stdout).context("ssh-keyscan stdout")?;
    let mut existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    } else {
        String::new()
    };
    for line in scanned.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !existing.lines().any(|present| present.trim() == line) {
            if !existing.is_empty() && !existing.ends_with('\n') {
                existing.push('\n');
            }
            existing.push_str(line);
            existing.push('\n');
        }
    }
    fs::write(path, existing).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn copy_id(public_key: &Path, user: &str, host: &str) -> Result<()> {
    let status = Command::new("ssh-copy-id")
        .args(["-i", public_key.to_str().context("public key path")?, &format!("{user}@{host}")])
        .status()
        .context("run ssh-copy-id")?;
    if !status.success() {
        bail!("ssh-copy-id failed with status {status}");
    }
    Ok(())
}

fn apply_container_acl(path: &Path) -> Result<()> {
    let status = Command::new("setfacl")
        .args(["-m", &format!("u:{CONTAINER_UID}:r"), path.to_str().context("acl path")?])
        .status();
    match status {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => bail!("setfacl failed with status {code}"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            eprintln!("setfacl not installed; skip ACL for {}", path.display());
            Ok(())
        }
        Err(error) => Err(error).context("run setfacl"),
    }
}

fn write_secret_file(path: &Path, secret: &str) -> Result<()> {
    fs::write(path, format!("{secret}\n")).with_context(|| format!("write {}", path.display()))?;
    set_file_mode(path, 0o600)?;
    Ok(())
}

fn file_status(path: &Path) -> FileStatus {
    FileStatus {
        path: path.to_path_buf(),
        present: path.is_file(),
        mode: unix_mode(path),
    }
}

impl FileStatus {
    fn or_else_present(self, fallback: &Path) -> Self {
        if self.present {
            self
        } else {
            file_status(fallback)
        }
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "beampipe".into())
}

fn stdin_is_tty() -> bool {
    std::io::IsTerminal::is_terminal(&io::stdin())
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_dir_mode(path: &Path, mode: u32) -> Result<()> {
    set_file_mode(path, mode)
}

#[cfg(not(unix))]
fn set_dir_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn unix_mode(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|meta| format!("{:04o}", meta.permissions().mode() & 0o777))
}

#[cfg(not(unix))]
fn unix_mode(_path: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_opts(dir: &Path, slot: &str) -> InitOptions {
        InitOptions {
            slot: slot.into(),
            dir: Some(dir.to_path_buf()),
            skip_keyscan: true,
            no_passphrase: true,
            yes: true,
            ..InitOptions::default()
        }
    }

    #[test]
    fn rejects_unsafe_slot_names() {
        let dir = tempfile::tempdir().unwrap();
        let err = init(InitOptions {
            slot: "../etc".into(),
            dir: Some(dir.path().to_path_buf()),
            yes: true,
            no_passphrase: true,
            skip_keyscan: true,
            ..InitOptions::default()
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("ssh_credential") || err.contains("ASCII"));
    }

    #[test]
    fn refuses_to_overwrite_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        init(temp_opts(dir.path(), "setonix")).unwrap();
        let err = init(temp_opts(dir.path(), "setonix"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn copies_passphrase_file_without_echoing_secret() {
        let dir = tempfile::tempdir().unwrap();
        let pass_in = dir.path().join("in.pass");
        fs::write(&pass_in, "super-secret-pass\n").unwrap();
        let result = init(InitOptions {
            slot: "setonix".into(),
            dir: Some(dir.path().to_path_buf()),
            passphrase_file: Some(pass_in),
            yes: true,
            skip_keyscan: true,
            ..InitOptions::default()
        })
        .unwrap();
        let written = fs::read_to_string(result.passphrase.as_ref().unwrap()).unwrap();
        assert_eq!(written.trim_end(), "super-secret-pass");
        let shown = serde_json::to_string(&show("setonix", Some(dir.path())).unwrap()).unwrap();
        assert!(!shown.contains("super-secret-pass"));
        assert!(shown.contains("\"present\":true"));
    }

    #[test]
    fn show_reports_missing_files_without_creating_them() {
        let dir = tempfile::tempdir().unwrap();
        let status = show("setonix", Some(dir.path())).unwrap();
        assert!(!status.private_key.present);
        assert!(!status.passphrase.present);
        assert_eq!(status.slot, "setonix");
    }
}

//! Operator tool for per-slot Slurm SSH credential files.

use crate::{
    installation::{InstallationContext, RuntimeMode},
    runtime,
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;
use zeroize::Zeroizing;

const CONTAINER_UID: &str = "10001";

#[derive(Debug, Clone)]
pub struct InitOptions {
    pub slot: String,
    pub dir: Option<PathBuf>,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub passphrase_file: Option<PathBuf>,
    pub no_passphrase: bool,
    pub copy_id: bool,
    pub acl: bool,
    pub force: bool,
    pub yes: bool,
    pub skip_keyscan: bool,
    pub accept_host_key: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            slot: String::new(),
            dir: None,
            host: String::new(),
            port: 22,
            user: None,
            passphrase_file: None,
            no_passphrase: false,
            copy_id: false,
            acl: false,
            force: false,
            yes: false,
            skip_keyscan: false,
            accept_host_key: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub slot: String,
    pub dir: Option<PathBuf>,
    pub private_key: PathBuf,
    pub public_key: Option<PathBuf>,
    pub known_hosts: Option<PathBuf>,
    pub passphrase_file: Option<PathBuf>,
    pub host: Option<String>,
    pub port: u16,
    pub acl: bool,
    pub force: bool,
    pub accept_host_key: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitResult {
    pub root: PathBuf,
    pub slot: String,
    pub private_key: PathBuf,
    pub public_key: PathBuf,
    pub passphrase: Option<PathBuf>,
    pub known_hosts: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub port: u16,
    pub copied_id: bool,
    pub generated: bool,
}

#[derive(Debug, Clone)]
pub struct CopyIdOptions {
    pub slot: String,
    pub dir: Option<PathBuf>,
    pub user: String,
    pub host: String,
    pub port: u16,
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

#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub root: PathBuf,
    pub slot: Option<String>,
    pub configured_slots: Vec<String>,
    pub bind_source_matches: bool,
    pub scheduler_readable: Option<bool>,
    pub worker_readable: Option<bool>,
    pub message: String,
}

pub fn default_credentials_root() -> PathBuf {
    if let Ok(path) = std::env::var("BEAMPIPE_SSH_CREDENTIALS_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("BEAMPIPE_HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join("credentials/ssh");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join("beampipe/credentials/ssh");
        }
    }
    PathBuf::from("/run/beampipe/ssh")
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
    if !opts.skip_keyscan && opts.host.trim().is_empty() {
        bail!("--host is required unless --skip-keyscan");
    }
    if opts.yes && !opts.no_passphrase && opts.passphrase_file.is_none() {
        bail!("--yes requires --no-passphrase or --passphrase-file (do not pass the passphrase on the command line)");
    }
    if opts.copy_id {
        require_copy_id_user(opts.user.as_deref())?;
        require_copy_id_host(&opts.host)?;
    }

    let root = resolve_root(opts.dir.as_deref());
    let slot_dir = root.join(&opts.slot);
    fs::create_dir_all(&slot_dir).with_context(|| format!("create {}", slot_dir.display()))?;
    set_dir_mode(&root, 0o700)?;
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
    generate_ed25519_key(
        &private_key,
        passphrase.as_deref().map(String::as_str),
        opts.force,
    )?;
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
        acquire_known_hosts(
            &known_hosts,
            &opts.host,
            opts.port,
            opts.yes,
            opts.accept_host_key,
        )?;
    } else if !known_hosts.exists() {
        fs::write(&known_hosts, "").with_context(|| format!("write {}", known_hosts.display()))?;
    }

    if opts.acl {
        apply_container_acl_tree(&root, &slot_dir, &private_key)?;
        if wrote_passphrase {
            apply_container_acl(&passphrase_path)?;
        }
    }
    let (copied_id, copy_user) = maybe_copy_id(&opts, &public_key)?;

    Ok(InitResult {
        root,
        slot: opts.slot,
        private_key,
        public_key,
        passphrase: wrote_passphrase.then_some(passphrase_path),
        known_hosts,
        host: nonempty_opt(&opts.host),
        user: copy_user
            .or_else(|| opts.user.clone())
            .and_then(|value| nonempty_opt(&value)),
        port: opts.port,
        copied_id,
        generated: true,
    })
}

pub fn import(opts: ImportOptions) -> Result<InitResult> {
    validate_slot(&opts.slot)?;
    validate_import_source(&opts.private_key, "private key")?;
    let root = resolve_root(opts.dir.as_deref());
    let slot_dir = root.join(&opts.slot);
    fs::create_dir_all(&slot_dir).with_context(|| format!("create {}", slot_dir.display()))?;
    set_dir_mode(&root, 0o700)?;
    set_dir_mode(&slot_dir, 0o700)?;

    let private_key = slot_dir.join("private_key");
    let public_key = slot_dir.join("private_key.pub");
    let slot_known_hosts = slot_dir.join("known_hosts");
    if private_key.exists() && !opts.force {
        bail!(
            "{} already exists; pass --force to replace the managed copy",
            private_key.display()
        );
    }
    atomic_copy(&opts.private_key, &private_key, 0o600)?;

    let inferred_public = inferred_public_key(&opts.private_key);
    let public_source = opts
        .public_key
        .or_else(|| inferred_public.is_file().then_some(inferred_public));
    if let Some(source) = public_source {
        validate_import_source(&source, "public key")?;
        atomic_copy(&source, &public_key, 0o644)?;
    }

    if let Some(source) = opts.known_hosts {
        validate_import_source(&source, "known_hosts")?;
        atomic_copy(&source, &slot_known_hosts, 0o600)?;
    } else if let Some(host) = opts.host.as_deref() {
        acquire_known_hosts(
            &slot_known_hosts,
            host,
            opts.port,
            false,
            opts.accept_host_key,
        )?;
    } else if !slot_known_hosts.is_file() && !root.join("known_hosts").is_file() {
        bail!("known_hosts is required: pass --known-hosts or --host with --accept-host-key");
    }

    let passphrase_path = slot_dir.join("passphrase");
    if let Some(source) = opts.passphrase_file {
        validate_import_source(&source, "passphrase")?;
        atomic_copy(&source, &passphrase_path, 0o600)?;
    }

    if opts.acl {
        apply_container_acl_tree(&root, &slot_dir, &private_key)?;
        if passphrase_path.is_file() {
            apply_container_acl(&passphrase_path)?;
        }
    }

    Ok(InitResult {
        root,
        slot: opts.slot,
        private_key,
        public_key,
        passphrase: passphrase_path.is_file().then_some(passphrase_path),
        known_hosts: slot_known_hosts,
        host: opts.host.as_deref().and_then(nonempty_opt),
        user: None,
        port: opts.port,
        copied_id: false,
        generated: false,
    })
}

pub fn copy_id_for_slot(opts: CopyIdOptions) -> Result<PathBuf> {
    validate_slot(&opts.slot)?;
    if opts.user.trim().is_empty() {
        bail!("--copy-id requires --user");
    }
    if opts.host.trim().is_empty() {
        bail!("--copy-id requires --host");
    }
    let public_key = resolve_root(opts.dir.as_deref())
        .join(&opts.slot)
        .join("private_key.pub");
    if !public_key.is_file() {
        bail!(
            "public key missing at {}; run `beampipe slurm credentials init` or `import` first",
            public_key.display()
        );
    }
    copy_id(&public_key, &opts.user, &opts.host, opts.port)?;
    Ok(public_key)
}

pub fn sync(context: &InstallationContext, slot: Option<&str>) -> Result<SyncResult> {
    let configured_slots = list_slots(Some(&context.credential_root))?;
    if let Some(slot) = slot {
        check(slot, Some(&context.credential_root))?;
    }
    let configured_bind =
        env_file_value(&context.environment_file, "BEAMPIPE_SSH_CREDENTIALS_HOST")
            .map(PathBuf::from);
    let bind_source_matches = configured_bind.as_deref() == Some(context.credential_root.as_path());
    if !bind_source_matches {
        bail!(
            "Compose SSH bind source does not match installation credential root {}; rerun `beampipe setup`",
            context.credential_root.display()
        );
    }

    let docker_runtime = context
        .state
        .as_ref()
        .is_some_and(|state| state.runtime == RuntimeMode::Docker);
    let (scheduler_readable, worker_readable, message) = match (slot, docker_runtime) {
        (Some(slot), true) => {
            let path = format!("/run/beampipe/ssh/{slot}/private_key");
            let scheduler = container_can_read(context, "scheduler", &path)?;
            let worker = container_can_read(context, "worker", &path)?;
            let message = if scheduler == Some(true) && worker == Some(true) {
                "credential slot is visible to the running Docker services".into()
            } else if scheduler.is_none() || worker.is_none() {
                "credential files are configured; start the Docker runtime to verify container readability".into()
            } else {
                bail!(
                    "credential slot is mounted but unreadable; run `beampipe slurm credentials import --acl ...` or fix uid 10001 ACLs"
                );
            };
            (scheduler, worker, message)
        }
        (Some(_), false) => (
            None,
            None,
            "host runtime resolves this credential slot directly from the canonical credential root"
                .into(),
        ),
        (None, _) => (
            None,
            None,
            "credential bind source matches the active installation".into(),
        ),
    };

    Ok(SyncResult {
        root: context.credential_root.clone(),
        slot: slot.map(str::to_string),
        configured_slots,
        bind_source_matches,
        scheduler_readable,
        worker_readable,
        message,
    })
}

pub fn remove(slot: &str, dir: Option<&Path>, confirmed: bool) -> Result<()> {
    validate_slot(slot)?;
    if !confirmed {
        bail!(
            "removing a credential slot requires --yes after deployment profiles have been reassigned"
        );
    }
    let slot_dir = resolve_root(dir).join(slot);
    let metadata = fs::symlink_metadata(&slot_dir)
        .with_context(|| format!("inspect {}", slot_dir.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "credential slot is not a managed directory: {}",
            slot_dir.display()
        );
    }
    fs::remove_dir_all(&slot_dir).with_context(|| format!("remove {}", slot_dir.display()))?;
    Ok(())
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
        known_hosts: file_status(&slot_dir.join("known_hosts"))
            .or_else_present(&root.join("known_hosts")),
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
    print!("{}", format_credential_next_steps(result));
}

pub fn format_credential_next_steps(result: &InitResult) -> String {
    let mut out = String::new();
    if result.generated {
        out.push_str(&format!("Created SSH credential slot '{}'.\n", result.slot));
    } else {
        out.push_str(&format!("Imported SSH credential slot '{}'.\n", result.slot));
    }
    out.push_str(&format!("  private_key  {}\n", result.private_key.display()));
    out.push_str(&format!("  public_key   {}\n", result.public_key.display()));
    if let Some(path) = &result.passphrase {
        out.push_str(&format!("  passphrase   {}\n", path.display()));
    }
    out.push_str(&format!("  known_hosts  {}\n", result.known_hosts.display()));

    if result.public_key.is_file() {
        if let Ok(contents) = fs::read_to_string(&result.public_key) {
            let line = contents.trim();
            if !line.is_empty() {
                out.push_str("\nPublic key:\n");
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    let host = result.host.as_deref().unwrap_or("login.example.org");
    let user = result.user.as_deref().unwrap_or("USER");
    if result.copied_id {
        out.push_str("\nPublic key installed on the login node via ssh-copy-id.\n");
    } else if result.generated {
        out.push_str(
            "\nThis slot cannot log in until the public key is in ~/.ssh/authorized_keys on the login node.\n",
        );
        out.push_str(
            "Beampipe generated this key; do not also run ssh-keygen for the same slot.\n",
        );
        out.push_str(
            "Beampipe does not use ssh-agent; workers unlock private_key plus an optional passphrase file.\n",
        );
        out.push_str("\nIf password SSH still works:\n");
        out.push_str(&format!("  {}\n", copy_id_example(result, user, host)));
        out.push_str("\nOr append manually (use >> so existing keys are kept):\n");
        out.push_str(&format!(
            "  cat {} | ssh {user}@{host} \"mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys\"\n",
            result.public_key.display()
        ));
        out.push_str(
            "\nSome sites require a portal or helpdesk key-registration process instead of ssh-copy-id.\n",
        );
    } else {
        out.push_str(
            "\nIf this public key is already in the login node's authorized_keys, skip the upload.\n",
        );
        out.push_str(
            "If not, install it the same way as after init (copy-id, append, or facility process).\n",
        );
        out.push_str("Do not run init for this slot unless you intend to replace the key.\n");
        out.push_str(&format!("  {}\n", copy_id_example(result, user, host)));
    }

    out.push_str("\nThen:\n");
    out.push_str(&format!(
        "  set deployment.ssh_credential to \"{}\"\n",
        result.slot
    ));
    out.push_str(&format!(
        "  beampipe slurm credentials check --slot {}\n",
        result.slot
    ));
    out.push_str(&format!(
        "  beampipe slurm credentials sync --slot {}\n",
        result.slot
    ));
    out.push_str("  beampipe slurm ping --profile PROFILE\n");
    out
}

fn copy_id_example(result: &InitResult, user: &str, host: &str) -> String {
    let mut cmd = format!(
        "beampipe slurm credentials copy-id --slot {} --user {user} --host {host}",
        result.slot
    );
    if result.port != 22 {
        cmd.push_str(&format!(" --port {}", result.port));
    }
    cmd
}

fn nonempty_opt(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn resolve_passphrase_input(opts: &InitOptions) -> Result<Option<Zeroizing<String>>> {
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
        return Ok(Some(Zeroizing::new(secret)));
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
    Ok(Some(Zeroizing::new(first)))
}

struct SystemRandom;

impl ssh_key::rand_core::TryRng for SystemRandom {
    type Error = std::convert::Infallible;

    fn try_next_u32(&mut self) -> std::result::Result<u32, Self::Error> {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes).expect("operating-system random source unavailable");
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> std::result::Result<u64, Self::Error> {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes).expect("operating-system random source unavailable");
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> std::result::Result<(), Self::Error> {
        getrandom::fill(destination).expect("operating-system random source unavailable");
        Ok(())
    }
}

impl ssh_key::rand_core::TryCryptoRng for SystemRandom {}

fn generate_ed25519_key(path: &Path, passphrase: Option<&str>, force: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() && !force {
        bail!("{} already exists", path.display());
    }
    let mut rng = SystemRandom;
    let mut private =
        PrivateKey::random(&mut rng, Algorithm::Ed25519).context("generate Ed25519 private key")?;
    private.set_comment(format!(
        "beampipe-{}@{}",
        path.parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("slurm"),
        hostname()
    ));
    let public = private.public_key().to_openssh()?;
    let private = if let Some(passphrase) = passphrase {
        private
            .encrypt(&mut rng, passphrase)
            .context("encrypt SSH private key")?
    } else {
        private
    };
    let encoded = private.to_openssh(LineEnding::LF)?;
    atomic_write(path, encoded.as_bytes(), 0o600)?;
    atomic_write(&inferred_public_key(path), public.as_bytes(), 0o644)?;
    Ok(())
}

fn acquire_known_hosts(
    path: &Path,
    host: &str,
    port: u16,
    non_interactive: bool,
    accept_host_key: bool,
) -> Result<()> {
    if non_interactive && !accept_host_key {
        bail!(
            "non-interactive ssh-keyscan requires --accept-host-key; importing a verified --known-hosts file is preferred"
        );
    }
    let port = port.to_string();
    let output = Command::new("ssh-keyscan")
        .args(["-p", port.as_str(), "-t", "ed25519", "-T", "8", host])
        .output()
        .context("run ssh-keyscan (install OpenSSH client)")?;
    if !output.status.success() || output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("ssh-keyscan failed for {host}: {stderr}");
    }
    let scanned = String::from_utf8(output.stdout).context("ssh-keyscan stdout")?;
    print_host_key_fingerprints(&scanned)?;
    if !accept_host_key && !prompt_yes_no("Trust these SSH host keys?", false)? {
        bail!("SSH host key was not accepted");
    }
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
    atomic_write(path, existing.as_bytes(), 0o600)?;
    Ok(())
}

fn maybe_copy_id(opts: &InitOptions, public_key: &Path) -> Result<(bool, Option<String>)> {
    if opts.copy_id {
        let user = require_copy_id_user(opts.user.as_deref())?;
        require_copy_id_host(&opts.host)?;
        copy_id(public_key, &user, &opts.host, opts.port)?;
        return Ok((true, Some(user)));
    }
    if opts.yes || !stdin_is_tty() || opts.host.trim().is_empty() {
        return Ok((false, None));
    }
    let prompt = format!(
        "Install this public key on {} now with ssh-copy-id (password SSH must still work)?",
        opts.host
    );
    if !prompt_yes_no(&prompt, false)? {
        return Ok((false, None));
    }
    let user = if let Some(user) = opts.user.as_deref().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }) {
        user
    } else {
        prompt_remote_user()?
    };
    copy_id(public_key, &user, &opts.host, opts.port)?;
    Ok((true, Some(user)))
}

fn require_copy_id_user(user: Option<&str>) -> Result<String> {
    match user.map(str::trim).filter(|value| !value.is_empty()) {
        Some(user) => Ok(user.to_string()),
        None => bail!("--copy-id requires --user"),
    }
}

fn require_copy_id_host(host: &str) -> Result<()> {
    if host.trim().is_empty() {
        bail!("--copy-id requires --host");
    }
    Ok(())
}

fn prompt_remote_user() -> Result<String> {
    print!("Remote SSH user: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let user = value.trim().to_string();
    if user.is_empty() {
        bail!("--copy-id requires --user");
    }
    Ok(user)
}

fn copy_id(public_key: &Path, user: &str, host: &str, port: u16) -> Result<()> {
    let port = port.to_string();
    let status = Command::new("ssh-copy-id")
        .args([
            "-i",
            public_key.to_str().context("public key path")?,
            "-p",
            port.as_str(),
            &format!("{user}@{host}"),
        ])
        .status()
        .context("run ssh-copy-id")?;
    if !status.success() {
        bail!("ssh-copy-id failed with status {status}");
    }
    Ok(())
}

fn apply_container_acl_tree(root: &Path, slot_dir: &Path, private_key: &Path) -> Result<()> {
    apply_container_traverse_acl(root)?;
    apply_container_traverse_acl(slot_dir)?;
    apply_container_acl(private_key)
}

fn apply_container_traverse_acl(path: &Path) -> Result<()> {
    run_setfacl(path, &format!("u:{CONTAINER_UID}:rx"))
}

fn apply_container_acl(path: &Path) -> Result<()> {
    run_setfacl(path, &format!("u:{CONTAINER_UID}:r"))
}

fn run_setfacl(path: &Path, entry: &str) -> Result<()> {
    let status = Command::new("setfacl")
        .args(["-m", entry, path.to_str().context("acl path")?])
        .status();
    match status {
        Ok(code) if code.success() => Ok(()),
        Ok(code) => bail!("setfacl failed with status {code}"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            bail!(
                "setfacl is required to grant container uid {CONTAINER_UID} access to {}; install the ACL tools package or omit --acl on Docker Desktop",
                path.display()
            )
        }
        Err(error) => Err(error).context("run setfacl"),
    }
}

fn write_secret_file(path: &Path, secret: &str) -> Result<()> {
    atomic_write(path, format!("{secret}\n").as_bytes(), 0o600)
}

fn atomic_copy(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let mut file = fs::File::open(source).with_context(|| format!("read {}", source.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    atomic_write(destination, &bytes, mode)
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .context("managed credential path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("credential"),
        Uuid::new_v4().simple()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;
    set_file_mode(&temporary, mode)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

fn validate_import_source(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("{label} source must not be a symlink: {}", path.display());
    }
    if !metadata.is_file() {
        bail!("{label} source must be a regular file: {}", path.display());
    }
    Ok(())
}

fn inferred_public_key(private_key: &Path) -> PathBuf {
    PathBuf::from(format!("{}.pub", private_key.display()))
}

fn print_host_key_fingerprints(known_hosts: &str) -> Result<()> {
    let path =
        std::env::temp_dir().join(format!("beampipe-known-hosts-{}", Uuid::new_v4().simple()));
    fs::write(&path, known_hosts)?;
    let output = Command::new("ssh-keygen")
        .args([
            "-lf",
            path.to_str().context("fingerprint path")?,
            "-E",
            "sha256",
        ])
        .output()
        .context("run ssh-keygen to display host-key fingerprints")?;
    let _ = fs::remove_file(&path);
    if !output.status.success() {
        bail!("could not fingerprint scanned SSH host keys");
    }
    println!("Scanned SSH host-key fingerprints:");
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn prompt_yes_no(label: &str, default_yes: bool) -> Result<bool> {
    if !stdin_is_tty() {
        bail!("{label} requires a TTY; pass --accept-host-key only after verifying the host key");
    }
    let hint = if default_yes { "Y/n" } else { "y/N" };
    print!("{label} [{hint}]: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim();
    if value.is_empty() {
        return Ok(default_yes);
    }
    Ok(value.eq_ignore_ascii_case("y") || value.eq_ignore_ascii_case("yes"))
}

fn env_file_value(path: &Path, key: &str) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let prefix = format!("{key}=");
    contents.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn container_can_read(
    context: &InstallationContext,
    service: &str,
    path: &str,
) -> Result<Option<bool>> {
    let output = runtime::compose_command(context)?
        .args(["ps", "--status", "running", "--services"])
        .output()
        .context("inspect running Compose services")?;
    if !output.status.success() {
        return Ok(None);
    }
    let services = String::from_utf8_lossy(&output.stdout);
    if !services.lines().any(|line| line.trim() == service) {
        return Ok(None);
    }
    let status = runtime::compose_command(context)?
        .args(["exec", "-T", service, "test", "-r", path])
        .status()
        .context("check credential readability in container")?;
    Ok(Some(status.success()))
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

    #[test]
    fn init_requires_host_unless_skip_keyscan() {
        let dir = tempfile::tempdir().unwrap();
        let err = init(InitOptions {
            slot: "hpc".into(),
            dir: Some(dir.path().to_path_buf()),
            host: String::new(),
            skip_keyscan: false,
            no_passphrase: true,
            yes: true,
            ..InitOptions::default()
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("--host is required"));
    }

    #[test]
    fn copy_id_requires_user() {
        let err = copy_id_for_slot(CopyIdOptions {
            slot: "hpc".into(),
            dir: None,
            user: String::new(),
            host: "login.example.org".into(),
            port: 22,
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("--user"));
    }

    #[test]
    fn copy_id_requires_host() {
        let err = copy_id_for_slot(CopyIdOptions {
            slot: "hpc".into(),
            dir: None,
            user: "alice".into(),
            host: String::new(),
            port: 22,
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("--host"));
    }

    #[test]
    fn init_copy_id_requires_user() {
        let dir = tempfile::tempdir().unwrap();
        let err = init(InitOptions {
            slot: "hpc".into(),
            dir: Some(dir.path().to_path_buf()),
            skip_keyscan: true,
            no_passphrase: true,
            yes: true,
            copy_id: true,
            host: "login.example.org".into(),
            ..InitOptions::default()
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("--user"));
    }

    fn sample_result(generated: bool) -> InitResult {
        InitResult {
            root: PathBuf::from("/tmp/creds"),
            slot: "hpc".into(),
            private_key: PathBuf::from("/tmp/creds/hpc/private_key"),
            public_key: PathBuf::from("/tmp/creds/hpc/private_key.pub"),
            passphrase: None,
            known_hosts: PathBuf::from("/tmp/creds/known_hosts"),
            host: Some("login.example.org".into()),
            user: Some("alice".into()),
            port: 22,
            copied_id: false,
            generated,
        }
    }

    #[test]
    fn next_steps_mention_public_key_and_authorized_keys() {
        let text = format_credential_next_steps(&sample_result(true));
        assert!(text.contains("public_key"));
        assert!(text.contains("authorized_keys"));
        assert!(text.contains("copy-id"));
        assert!(text.contains("do not also run ssh-keygen"));
        assert!(text.contains(">>"));
        assert!(text.contains("--slot hpc --user alice --host login.example.org"));
    }

    #[test]
    fn import_next_steps_say_skip_upload_when_already_authorized() {
        let text = format_credential_next_steps(&sample_result(false));
        assert!(text.contains("Imported SSH credential slot"));
        assert!(text.contains("already"));
        assert!(text.contains("authorized_keys"));
        assert!(text.contains("Do not run init"));
    }

    #[test]
    fn generated_slot_records_public_key_path() {
        let dir = tempfile::tempdir().unwrap();
        let result = init(temp_opts(dir.path(), "hpc")).unwrap();
        assert!(result.generated);
        assert!(!result.copied_id);
        assert!(result.public_key.is_file());
        let text = format_credential_next_steps(&result);
        assert!(text.contains("ssh-ed25519"));
        assert!(text.contains("authorized_keys"));
    }
}

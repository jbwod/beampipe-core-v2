//! Resolve Slurm SSH private keys and known-hosts policy from environment.

use crate::OrchestrationError;
use beampipe_profiles::ProfileValidationError;
use beampipe_security::{
    allow_inline_secrets_override, bool_env, is_runtime_production, runtime_env_name,
};
use russh::keys::{decode_secret_key, load_secret_key, PrivateKey};
use serde::Serialize;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Presence of files in a named SSH credential slot. Never includes key material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotPresence {
    pub name: String,
    pub private_key: bool,
    pub public_key: bool,
    pub passphrase: bool,
    pub known_hosts: bool,
}

/// Resolved SSH material for Slurm login nodes (process-wide from env).
#[derive(Clone)]
pub struct SlurmSshCredentials {
    pub key_source: SlurmKeySource,
    pub known_hosts_path: Option<String>,
    pub strict_known_hosts: bool,
    /// Named credential slot from `deployment.ssh_credential`, if any.
    pub slot: Option<String>,
}

#[derive(Clone)]
pub enum SlurmKeySource {
    /// PEM loaded from `SLURM_SSH_PRIVATE_KEY` (never logged).
    Pem(Zeroizing<Vec<u8>>),
    /// Path from `SLURM_SSH_PRIVATE_KEY_PATH`, `SLURM_SSH_PRIVATE_KEY_FILE`, or dev fallback.
    Path(PathBuf),
    /// Dev-only fallback from `~/.ssh` when explicitly enabled.
    DevHome(PathBuf),
}

impl fmt::Debug for SlurmSshCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("SlurmSshCredentials");
        debug.field("key_source", &self.key_source);
        if is_production_env() {
            debug.field(
                "known_hosts_path",
                &self.known_hosts_path.as_ref().map(|_| "[REDACTED_PATH]"),
            );
        } else {
            debug.field("known_hosts_path", &self.known_hosts_path);
        }
        debug
            .field("strict_known_hosts", &self.strict_known_hosts)
            .field("slot", &self.slot)
            .finish()
    }
}

impl fmt::Debug for SlurmKeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlurmKeySource::Pem(_) => f
                .debug_struct("Pem")
                .field("source_kind", &"env")
                .field("value", &"[REDACTED]")
                .finish(),
            SlurmKeySource::Path(path) => {
                let mut debug = f.debug_struct("Path");
                if is_production_env() {
                    debug.field("path", &"[REDACTED_PATH]");
                } else {
                    debug.field("path", path);
                }
                debug.finish()
            }
            SlurmKeySource::DevHome(path) => {
                let mut debug = f.debug_struct("DevHome");
                if is_production_env() {
                    debug.field("path", &"[REDACTED_PATH]");
                } else {
                    debug.field("path", path);
                }
                debug.finish()
            }
        }
    }
}

impl SlurmKeySource {
    pub fn source_kind(&self) -> &'static str {
        match self {
            SlurmKeySource::Pem(_) => "env",
            SlurmKeySource::Path(_) => "file",
            SlurmKeySource::DevHome(_) => "dev_home",
        }
    }
}

pub fn beampipe_env() -> String {
    runtime_env_name()
}

pub fn is_production_env() -> bool {
    is_runtime_production()
}

fn parse_bool_env(name: &str) -> Option<bool> {
    bool_env(name)
}

fn allow_insecure_ssh_host_keys() -> bool {
    parse_bool_env("BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS").unwrap_or(false)
}

/// Default strict host-key policy: true in production; dev can opt in/out.
pub fn strict_known_hosts_default() -> bool {
    if let Some(v) = parse_bool_env("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS") {
        return v;
    }
    is_production_env()
}

fn first_non_empty(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(v) = std::env::var(name) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn env_suffix(slot: &str) -> String {
    slot.chars()
        .map(|character| {
            if character == '-' || character == '.' {
                '_'
            } else {
                character
            }
        })
        .collect::<String>()
        .to_ascii_uppercase()
}

fn first_slotted(bases: &[&str], suffix: &str) -> Option<String> {
    let names = bases
        .iter()
        .map(|base| format!("{base}_{suffix}"))
        .collect::<Vec<_>>();
    first_non_empty(&names.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Operator credentials root used by both the native binary and containers.
/// Docker should mount the same tree at this path and set the env var.
pub fn ssh_credentials_dir() -> Option<PathBuf> {
    if let Some(path) = first_non_empty(&["BEAMPIPE_SSH_CREDENTIALS_DIR"]) {
        return Some(PathBuf::from(path));
    }
    let runtime = PathBuf::from("/run/beampipe/ssh");
    if runtime.is_dir() {
        return Some(runtime);
    }
    std::env::var("HOME")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(|home| PathBuf::from(home).join(".config/beampipe/credentials"))
}

fn slot_dir(slot: &str) -> Option<PathBuf> {
    ssh_credentials_dir().map(|root| root.join(slot))
}

fn slot_key_path(slot: &str) -> Option<PathBuf> {
    let dir = slot_dir(slot)?;
    for name in ["private_key", "slurm_key"] {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn slot_known_hosts_path(slot: &str) -> Option<String> {
    if let Some(path) = slot_dir(slot).map(|dir| dir.join("known_hosts")) {
        if path.is_file() {
            return Some(path.to_string_lossy().into_owned());
        }
    }
    if let Some(path) = ssh_credentials_dir().map(|root| root.join("known_hosts")) {
        if path.is_file() {
            return Some(path.to_string_lossy().into_owned());
        }
    }
    None
}

/// Immediate valid subdirectory names under the credentials root.
pub fn list_credential_slots() -> Vec<String> {
    let Some(root) = ssh_credentials_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| beampipe_profiles::validate_ssh_credential_name(name).is_ok())
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// File presence for every listed credential slot. Empty or missing root yields `[]`.
pub fn list_credential_slot_presence() -> Vec<SlotPresence> {
    list_credential_slots()
        .into_iter()
        .filter_map(|name| inspect_credential_slot(&name).ok())
        .collect()
}

/// Presence of files for `name`. Invalid names fail; a missing directory is all-false.
pub fn inspect_credential_slot(name: &str) -> Result<SlotPresence, ProfileValidationError> {
    beampipe_profiles::validate_ssh_credential_name(name)?;
    let Some(root) = ssh_credentials_dir() else {
        return Ok(empty_slot_presence(name));
    };
    let slot_dir = root.join(name);
    Ok(SlotPresence {
        name: name.to_string(),
        private_key: slot_dir.join("private_key").is_file(),
        public_key: slot_dir.join("private_key.pub").is_file(),
        passphrase: slot_dir.join("passphrase").is_file(),
        known_hosts: slot_dir.join("known_hosts").is_file() || root.join("known_hosts").is_file(),
    })
}

fn empty_slot_presence(name: &str) -> SlotPresence {
    SlotPresence {
        name: name.to_string(),
        private_key: false,
        public_key: false,
        passphrase: false,
        known_hosts: false,
    }
}

pub fn has_global_ssh_key_config() -> bool {
    first_non_empty(&[
        "SLURM_SSH_PRIVATE_KEY",
        "SLURM_SSH_PRIVATE_KEY_PATH",
        "SLURM_SSH_PRIVATE_KEY_FILE",
    ])
    .is_some()
        || parse_bool_env("BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK").unwrap_or(false)
}

fn resolve_known_hosts_path(slot: Option<&str>) -> Option<String> {
    if let Some(name) = slot {
        let suffix = env_suffix(name);
        if let Some(path) = first_slotted(
            &["SLURM_SSH_KNOWN_HOSTS", "SLURM_SSH_KNOWN_HOSTS_SOURCE"],
            &suffix,
        ) {
            return Some(path);
        }
        if let Some(path) = slot_known_hosts_path(name) {
            return Some(path);
        }
    }
    first_non_empty(&["SLURM_SSH_KNOWN_HOSTS", "SLURM_SSH_KNOWN_HOSTS_SOURCE"])
}

fn read_passphrase_file(path: &str) -> Result<Option<Zeroizing<String>>, OrchestrationError> {
    check_private_key_permissions(Path::new(path))?;
    let mut buf = String::new();
    std::fs::File::open(path)
        .and_err_path(path)?
        .read_to_string(&mut buf)
        .map_err(|e| OrchestrationError::Backend(format!("read passphrase file: {e}")))?;
    let trimmed = buf.trim_end_matches(['\r', '\n']).to_string();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(Zeroizing::new(trimmed))
    })
}

fn resolve_passphrase(slot: Option<&str>) -> Result<Option<Zeroizing<String>>, OrchestrationError> {
    if let Some(name) = slot {
        let suffix = env_suffix(name);
        if let Some(path) = first_slotted(
            &[
                "SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE",
                "SLURM_SSH_PRIVATE_KEY_PASSCODE_FILE",
            ],
            &suffix,
        ) {
            return read_passphrase_file(&path);
        }
        if let Some(inline) = first_slotted(
            &[
                "SLURM_SSH_PRIVATE_KEY_PASSPHRASE",
                "SLURM_SSH_PRIVATE_KEY_PASSCODE",
            ],
            &suffix,
        ) {
            return Ok(Some(Zeroizing::new(inline)));
        }
        if let Some(dir) = slot_dir(name) {
            for filename in ["passphrase", "passcode"] {
                let path = dir.join(filename);
                if path.is_file() {
                    return read_passphrase_file(&path.to_string_lossy());
                }
            }
        }
        return Ok(None);
    }
    if let Some(path) = first_non_empty(&[
        "SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE",
        "SLURM_SSH_PRIVATE_KEY_PASSCODE_FILE",
    ]) {
        return read_passphrase_file(&path);
    }
    Ok(first_non_empty(&[
        "SLURM_SSH_PRIVATE_KEY_PASSPHRASE",
        "SLURM_SSH_PRIVATE_KEY_PASSCODE",
        "SSH_KEY_PASSPHRASE",
    ])
    .map(Zeroizing::new))
}

trait PathErr {
    fn and_err_path(self, path: &str) -> Result<std::fs::File, OrchestrationError>;
}

impl PathErr for Result<std::fs::File, std::io::Error> {
    fn and_err_path(self, path: &str) -> Result<std::fs::File, OrchestrationError> {
        self.map_err(|e| OrchestrationError::Backend(format!("open {path}: {e}")))
    }
}

#[cfg(unix)]
fn check_private_key_permissions(path: &Path) -> Result<(), OrchestrationError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let link_meta = std::fs::symlink_metadata(path)
        .map_err(|e| OrchestrationError::Backend(format!("stat SSH key: {e}")))?;
    if link_meta.file_type().is_symlink() && is_production_env() {
        return Err(OrchestrationError::Backend(
            "SSH private key path must not be a symlink in production".into(),
        ));
    }
    let meta = std::fs::metadata(path)
        .map_err(|e| OrchestrationError::Backend(format!("stat SSH key target: {e}")))?;
    if !meta.file_type().is_file() {
        return Err(OrchestrationError::Backend(
            "SSH private key path must be a regular file".into(),
        ));
    }
    let mode = meta.permissions().mode() & 0o777;
    if private_key_mode_too_open(path, mode) {
        return Err(OrchestrationError::Backend(format!(
            "SSH private key {} permissions {mode:o} are too open (expected 0600 or stricter)",
            path.display()
        )));
    }
    if is_production_env() {
        let owner = meta.uid();
        let current = unsafe { libc::geteuid() };
        if owner != 0 && owner != current {
            return Err(OrchestrationError::Backend(
                "SSH private key must be owned by the Beampipe process user or root in production"
                    .into(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn private_key_mode_too_open(path: &Path, mode: u32) -> bool {
    if mode & 0o007 != 0 {
        return true;
    }
    if mode & 0o070 == 0 {
        return false;
    }
    linux_posix_acl_access(path).is_none_or(|acl| posix_acl_grants_group_or_other(&acl))
}

#[cfg(not(unix))]
fn check_private_key_permissions(_path: &Path) -> Result<(), OrchestrationError> {
    Ok(())
}

const POSIX_ACL_XATTR_VERSION: u32 = 2;
const ACL_GROUP_OBJ: u16 = 0x04;
const ACL_GROUP: u16 = 0x08;
const ACL_OTHER: u16 = 0x20;
const ACL_READ: u16 = 0x4;
const ACL_WRITE: u16 = 0x2;

fn posix_acl_grants_group_or_other(acl: &[u8]) -> bool {
    if acl.len() < 4 || acl.len() % 8 != 4 {
        return true;
    }
    let version = u32::from_le_bytes(acl[0..4].try_into().unwrap_or([0; 4]));
    if version != POSIX_ACL_XATTR_VERSION {
        return true;
    }
    acl[4..].chunks_exact(8).any(|chunk| {
        let tag = u16::from_le_bytes(chunk[0..2].try_into().unwrap_or([0; 2]));
        let perm = u16::from_le_bytes(chunk[2..4].try_into().unwrap_or([0; 2]));
        matches!(tag, ACL_GROUP_OBJ | ACL_GROUP | ACL_OTHER) && perm & (ACL_READ | ACL_WRITE) != 0
    })
}

#[cfg(target_os = "linux")]
fn linux_posix_acl_access(path: &Path) -> Option<Vec<u8>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let name = CString::new("system.posix_acl_access").ok()?;
    let size = unsafe { libc::lgetxattr(c_path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return None;
    }
    let mut buf = vec![0_u8; size as usize];
    let read = unsafe {
        libc::lgetxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if read <= 0 {
        return None;
    }
    buf.truncate(read as usize);
    Some(buf)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn linux_posix_acl_access(_path: &Path) -> Option<Vec<u8>> {
    None
}

fn map_key_load_error(
    path: &Path,
    passphrase_set: bool,
    err: &dyn std::fmt::Display,
) -> OrchestrationError {
    let msg = err.to_string();
    let hint = if !passphrase_set
        && (msg.contains("decrypt")
            || msg.contains("passphrase")
            || msg.contains("password")
            || msg.contains("incorrect"))
    {
        " — set a 0600 passphrase file next to the key, or SLURM_SSH_PRIVATE_KEY_PASSPHRASE / *_FILE"
    } else {
        ""
    };
    if is_production_env() {
        OrchestrationError::Backend(format!("load SSH key: {msg}{hint}"))
    } else {
        OrchestrationError::Backend(format!("load SSH key {}: {msg}{hint}", path.display()))
    }
}

fn normalize_slot(slot: Option<&str>) -> Result<Option<String>, OrchestrationError> {
    let Some(raw) = slot.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    beampipe_profiles::validate_ssh_credential_name(raw)
        .map_err(|error| OrchestrationError::Backend(error.to_string()))?;
    Ok(Some(raw.to_string()))
}

fn reject_inline_pem() -> Result<(), OrchestrationError> {
    if is_production_env() && !allow_inline_secrets_override() {
        return Err(OrchestrationError::Backend(
            "SLURM_SSH_PRIVATE_KEY inline PEM is not allowed in production; use SLURM_SSH_PRIVATE_KEY_PATH or SLURM_SSH_PRIVATE_KEY_FILE, or set BEAMPIPE_ALLOW_INLINE_SECRETS=true"
                .into(),
        ));
    }
    Ok(())
}

fn resolve_key_source(slot: Option<&str>) -> Result<SlurmKeySource, OrchestrationError> {
    if let Some(name) = slot {
        let suffix = env_suffix(name);
        if let Some(pem) = first_slotted(&["SLURM_SSH_PRIVATE_KEY"], &suffix) {
            reject_inline_pem()?;
            return Ok(SlurmKeySource::Pem(Zeroizing::new(pem.into_bytes())));
        }
        if let Some(path) = first_slotted(
            &["SLURM_SSH_PRIVATE_KEY_PATH", "SLURM_SSH_PRIVATE_KEY_FILE"],
            &suffix,
        ) {
            let pb = PathBuf::from(&path);
            check_private_key_permissions(&pb)?;
            return Ok(SlurmKeySource::Path(pb));
        }
        if let Some(path) = slot_key_path(name) {
            check_private_key_permissions(&path)?;
            return Ok(SlurmKeySource::Path(path));
        }
        let looked = ssh_credentials_dir()
            .map(|root| root.join(name).join("private_key"))
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| format!("<credentials>/{name}/private_key"));
        return Err(OrchestrationError::Backend(format!(
            "no Slurm SSH private key for credential '{name}': place private_key or slurm_key at {looked}, or set SLURM_SSH_PRIVATE_KEY_PATH_{suffix}"
        )));
    }

    if let Some(pem) = first_non_empty(&["SLURM_SSH_PRIVATE_KEY"]) {
        reject_inline_pem()?;
        return Ok(SlurmKeySource::Pem(Zeroizing::new(pem.into_bytes())));
    }
    if let Some(path) =
        first_non_empty(&["SLURM_SSH_PRIVATE_KEY_PATH", "SLURM_SSH_PRIVATE_KEY_FILE"])
    {
        let pb = PathBuf::from(&path);
        check_private_key_permissions(&pb)?;
        return Ok(SlurmKeySource::Path(pb));
    }
    if let Some(path) = home_ssh_fallback() {
        check_private_key_permissions(&path)?;
        return Ok(SlurmKeySource::DevHome(path));
    }
    Err(OrchestrationError::Backend(
        "no Slurm SSH private key: set SLURM_SSH_PRIVATE_KEY, SLURM_SSH_PRIVATE_KEY_PATH, or SLURM_SSH_PRIVATE_KEY_FILE (or BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK=true for ~/.ssh)"
            .into(),
    ))
}

fn home_ssh_fallback() -> Option<PathBuf> {
    if !parse_bool_env("BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK").unwrap_or(false) {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    for name in ["id_ed25519", "id_rsa"] {
        let p = PathBuf::from(&home).join(".ssh").join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

impl SlurmSshCredentials {
    pub fn resolve() -> Result<Self, OrchestrationError> {
        Self::resolve_for(None)
    }

    pub fn resolve_for(slot: Option<&str>) -> Result<Self, OrchestrationError> {
        let slot = normalize_slot(slot)?;
        let strict_known_hosts = strict_known_hosts_default();
        let known_hosts_path = resolve_known_hosts_path(slot.as_deref());

        if is_production_env() && !strict_known_hosts && !allow_insecure_ssh_host_keys() {
            return Err(OrchestrationError::Backend(
                "BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS=false is not allowed in production without BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS=true"
                    .into(),
            ));
        }

        if is_production_env()
            && parse_bool_env("BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK").unwrap_or(false)
        {
            return Err(OrchestrationError::Backend(
                "BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK is not allowed in production".into(),
            ));
        }

        if strict_known_hosts {
            let kh = known_hosts_path.as_deref().unwrap_or("");
            if kh.is_empty() || kh.eq_ignore_ascii_case("none") {
                return Err(OrchestrationError::Backend(
                    if slot.is_some() {
                        "known_hosts required for this SSH credential: add known_hosts next to the key, or set SLURM_SSH_KNOWN_HOSTS_<SLOT> / SLURM_SSH_KNOWN_HOSTS"
                    } else {
                        "known_hosts required: set SLURM_SSH_KNOWN_HOSTS or SLURM_SSH_KNOWN_HOSTS_SOURCE when strict host verification is enabled"
                    }
                    .into(),
                ));
            }
            crate::slurm_ssh::load_known_host_keys(kh)?;
        }

        let key_source = resolve_key_source(slot.as_deref())?;

        Ok(Self {
            key_source,
            known_hosts_path,
            strict_known_hosts,
            slot,
        })
    }

    pub fn load_private_key(&self) -> Result<PrivateKey, OrchestrationError> {
        let passphrase = resolve_passphrase(self.slot.as_deref())?;
        match &self.key_source {
            SlurmKeySource::Pem(bytes) => {
                let pem = std::str::from_utf8(bytes).map_err(|e| {
                    OrchestrationError::Backend(format!("SLURM_SSH_PRIVATE_KEY invalid UTF-8: {e}"))
                })?;
                decode_secret_key(pem, passphrase.as_ref().map(|s| s.as_str())).map_err(|e| {
                    let path = Path::new("SLURM_SSH_PRIVATE_KEY");
                    map_key_load_error(path, passphrase.is_some(), &e)
                })
            }
            SlurmKeySource::Path(path) | SlurmKeySource::DevHome(path) => {
                load_secret_key(path, passphrase.as_ref().map(|s| s.as_str()))
                    .map_err(|e| map_key_load_error(path, passphrase.is_some(), &e))
            }
        }
    }

    /// Whether Slurm SSH credentials can be resolved (for health checks).
    pub fn try_resolve_ok() -> bool {
        if Self::resolve().is_ok() {
            return true;
        }
        list_credential_slots()
            .iter()
            .any(|slot| Self::resolve_for(Some(slot)).is_ok())
    }
}

/// Build OpenSSH-style `-i` / `UserKnownHostsFile=` args for transitional CLI wrappers.
pub fn ssh_option_args_from_credentials(
    creds: &SlurmSshCredentials,
) -> Result<Vec<String>, OrchestrationError> {
    let mut args = Vec::new();
    match &creds.key_source {
        SlurmKeySource::Path(path) | SlurmKeySource::DevHome(path) => {
            args.push("-i".into());
            args.push(path.display().to_string());
        }
        SlurmKeySource::Pem(_) => {
            return Err(OrchestrationError::Backend(
                "inline PEM credentials cannot be converted to OpenSSH -i arguments".into(),
            ));
        }
    }
    if let Some(path) = creds.known_hosts_path.as_ref() {
        if !path.eq_ignore_ascii_case("none") {
            args.push("-o".into());
            args.push(format!("UserKnownHostsFile={path}"));
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn strict_default_true_in_production_env() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("BEAMPIPE_ENV", " Production ");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        assert!(strict_known_hosts_default());
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn bool_env_normalizes_mixed_case_and_whitespace() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", " TrUe ");
        assert!(strict_known_hosts_default());
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", " OFF ");
        assert!(!strict_known_hosts_default());
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
    }

    #[test]
    fn path_precedence_over_file_alias() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path_key = dir.path().join("path_key");
        let file_key = dir.path().join("file_key");
        std::fs::write(&path_key, "path").unwrap();
        std::fs::write(&file_key, "file").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for p in [&path_key, &file_key] {
                let mut perms = std::fs::metadata(p).unwrap().permissions();
                perms.set_mode(0o600);
                std::fs::set_permissions(p, perms).unwrap();
            }
        }
        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_PATH", &path_key);
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_FILE", &file_key);
        let creds = SlurmSshCredentials::resolve().unwrap();
        match creds.key_source {
            SlurmKeySource::Path(p) => assert_eq!(p, path_key),
            _ => panic!("expected path key"),
        }
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn rejects_loose_key_permissions() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_test");
        std::fs::write(&key, "not-a-real-key").unwrap();
        let mut perms = std::fs::metadata(&key).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&key, perms).unwrap();
        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_FILE", &key);
        assert!(SlurmSshCredentials::resolve().is_err());
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    fn acl_entry(tag: u16, perm: u16, id: u32) -> [u8; 8] {
        let mut entry = [0_u8; 8];
        entry[0..2].copy_from_slice(&tag.to_le_bytes());
        entry[2..4].copy_from_slice(&perm.to_le_bytes());
        entry[4..8].copy_from_slice(&id.to_le_bytes());
        entry
    }

    fn acl_blob(entries: &[[u8; 8]]) -> Vec<u8> {
        let mut blob = Vec::from(2_u32.to_le_bytes());
        for entry in entries {
            blob.extend_from_slice(entry);
        }
        blob
    }

    #[test]
    fn container_named_user_acl_is_not_group_or_other_access() {
        let undefined = u32::MAX;
        let acl = acl_blob(&[
            acl_entry(0x01, 0x6, undefined),
            acl_entry(0x02, 0x4, 10001),
            acl_entry(0x04, 0x0, undefined),
            acl_entry(0x10, 0x4, undefined),
            acl_entry(0x20, 0x0, undefined),
        ]);
        assert!(!posix_acl_grants_group_or_other(&acl));
    }

    #[test]
    fn owning_group_read_acl_is_too_open() {
        let undefined = u32::MAX;
        let acl = acl_blob(&[
            acl_entry(0x01, 0x6, undefined),
            acl_entry(0x04, 0x4, undefined),
            acl_entry(0x10, 0x4, undefined),
            acl_entry(0x20, 0x0, undefined),
        ]);
        assert!(posix_acl_grants_group_or_other(&acl));
    }

    #[test]
    fn rejects_group_readable_key_without_acl() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_test");
        std::fs::write(&key, "not-a-real-key").unwrap();
        let mut perms = std::fs::metadata(&key).unwrap().permissions();
        perms.set_mode(0o640);
        std::fs::set_permissions(&key, perms).unwrap();
        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_FILE", &key);
        let err = SlurmSshCredentials::resolve().unwrap_err().to_string();
        assert!(err.contains("too open"), "{err}");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn accepts_setfacl_named_user_when_stat_shows_group_bits() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_test");
        std::fs::write(&key, "not-a-real-key").unwrap();
        let mut perms = std::fs::metadata(&key).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&key, perms).unwrap();
        let applied = std::process::Command::new("setfacl")
            .args(["-m", "u:10001:r"])
            .arg(&key)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !applied {
            return;
        }
        let mode = std::fs::metadata(&key).unwrap().permissions().mode() & 0o777;
        assert!(
            !private_key_mode_too_open(&key, mode),
            "mode {mode:o} should be accepted as an ACL mask"
        );
        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_FILE", &key);
        SlurmSshCredentials::resolve().expect("named-user ACL key should resolve");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn production_rejects_inline_private_key_without_escape_hatch() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("BEAMPIPE_ENV", "production");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::set_var("BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS", "true");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY", "not-a-real-key");
        std::env::remove_var("BEAMPIPE_ALLOW_INLINE_SECRETS");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK");
        let err = SlurmSshCredentials::resolve().unwrap_err().to_string();
        assert!(err.contains("inline PEM is not allowed"));
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::remove_var("BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn production_rejects_non_strict_known_hosts_without_escape_hatch() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("BEAMPIPE_ENV", "production");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::remove_var("BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS");
        let err = SlurmSshCredentials::resolve().unwrap_err().to_string();
        assert!(err.contains("STRICT_KNOWN_HOSTS=false is not allowed"));
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn production_rejects_home_fallback() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("BEAMPIPE_ENV", "production");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::set_var("BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS", "true");
        std::env::set_var("BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK", "true");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        let err = SlurmSshCredentials::resolve().unwrap_err().to_string();
        assert!(err.contains("not allowed in production"));
        std::env::remove_var("BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK");
        std::env::remove_var("BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn passphrase_file_preserves_spaces_and_trims_newlines() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pass_path = dir.path().join("passphrase");
        std::fs::write(&pass_path, "  secret with spaces  \n").unwrap();
        let mut perms = std::fs::metadata(&pass_path).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&pass_path, perms).unwrap();
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE", &pass_path);
        let passphrase = resolve_passphrase(None).unwrap().unwrap();
        assert_eq!(passphrase.as_str(), "  secret with spaces  ");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE");
    }

    #[test]
    fn debug_redacts_inline_private_key() {
        let creds = SlurmSshCredentials {
            key_source: SlurmKeySource::Pem(Zeroizing::new(b"PRIVATE KEY MATERIAL".to_vec())),
            known_hosts_path: Some("/tmp/known_hosts".into()),
            strict_known_hosts: true,
            slot: None,
        };
        let rendered = format!("{creds:?}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("PRIVATE KEY MATERIAL"));
    }

    #[test]
    fn loads_passphrase_protected_ed25519_key() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("id_encrypted");
        let pass = "test-passcode-123";
        let status = std::process::Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                key_path.to_str().unwrap(),
                "-N",
                pass,
                "-q",
            ])
            .status()
            .expect("ssh-keygen");
        assert!(
            status.success(),
            "ssh-keygen failed (install OpenSSH client)"
        );

        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_FILE", &key_path);
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSCODE");

        let creds = SlurmSshCredentials::resolve().unwrap();
        assert!(
            creds.load_private_key().is_err(),
            "encrypted key must fail without passphrase"
        );

        std::env::set_var("SLURM_SSH_PRIVATE_KEY_PASSCODE", pass);
        creds.load_private_key().expect("passcode env unlocks key");

        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSCODE");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE", pass);
        creds
            .load_private_key()
            .expect("passphrase env unlocks key");

        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    fn write_mode600(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn named_slot_uses_directory_key_and_ignores_global_key() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let slot = dir.path().join("setonix-pawsey0411");
        std::fs::create_dir(&slot).unwrap();
        let slot_key = slot.join("private_key");
        let global_key = dir.path().join("global_key");
        write_mode600(&slot_key, "slot-key");
        write_mode600(&global_key, "global-key");

        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", dir.path());
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_FILE", &global_key);
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH");

        let creds = SlurmSshCredentials::resolve_for(Some("setonix-pawsey0411")).unwrap();
        match creds.key_source {
            SlurmKeySource::Path(path) => assert_eq!(path, slot_key),
            other => panic!("expected slot path, got {other:?}"),
        }
        assert_eq!(creds.slot.as_deref(), Some("setonix-pawsey0411"));

        std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn named_slot_does_not_fall_back_to_global_key() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let global_key = dir.path().join("global_key");
        write_mode600(&global_key, "global-key");
        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", dir.path());
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_FILE", &global_key);
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH");

        let err = SlurmSshCredentials::resolve_for(Some("missing-slot"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing-slot"));
        assert!(!err.contains("SLURM_SSH_PRIVATE_KEY_FILE"));

        std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn slotted_env_path_overrides_directory() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let slot = dir.path().join("setonix");
        std::fs::create_dir(&slot).unwrap();
        let dir_key = slot.join("private_key");
        let env_key = dir.path().join("env_key");
        write_mode600(&dir_key, "dir-key");
        write_mode600(&env_key, "env-key");

        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", dir.path());
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_PATH_SETONIX", &env_key);
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");

        let creds = SlurmSshCredentials::resolve_for(Some("setonix")).unwrap();
        match creds.key_source {
            SlurmKeySource::Path(path) => assert_eq!(path, env_key),
            other => panic!("expected env path, got {other:?}"),
        }

        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH_SETONIX");
        std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn named_slot_unlocks_passphrase_protected_key_from_slot_file() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let slot = dir.path().join("setonix");
        std::fs::create_dir(&slot).unwrap();
        let key_path = slot.join("private_key");
        let pass = "slot-passcode-123";
        let status = std::process::Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                key_path.to_str().unwrap(),
                "-N",
                pass,
                "-q",
            ])
            .status()
            .expect("ssh-keygen");
        assert!(status.success(), "ssh-keygen failed");
        let mut key_perms = std::fs::metadata(&key_path).unwrap().permissions();
        key_perms.set_mode(0o600);
        std::fs::set_permissions(&key_path, key_perms).unwrap();
        write_mode600(&slot.join("passphrase"), pass);

        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "false");
        std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", dir.path());
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PATH");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_FILE");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE");

        let creds = SlurmSshCredentials::resolve_for(Some("setonix")).unwrap();
        creds
            .load_private_key()
            .expect("slot passphrase file unlocks the key");

        std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }

    #[test]
    fn inspect_reports_presence_without_serializing_key_material() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let slot = dir.path().join("hpc");
        std::fs::create_dir(&slot).unwrap();
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nnot-a-real-key\n-----END OPENSSH PRIVATE KEY-----\n";
        write_mode600(&slot.join("private_key"), pem);
        std::fs::write(slot.join("private_key.pub"), "ssh-ed25519 AAAA demo").unwrap();
        write_mode600(&slot.join("passphrase"), "super-secret-passphrase");
        std::fs::write(
            slot.join("known_hosts"),
            "login.example.org ssh-ed25519 AAAA",
        )
        .unwrap();
        std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", dir.path());

        let listed = list_credential_slot_presence();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "hpc");
        assert!(listed[0].private_key);
        assert!(listed[0].public_key);
        assert!(listed[0].passphrase);
        assert!(listed[0].known_hosts);

        let json = serde_json::to_string(&listed[0]).unwrap();
        assert!(!json.contains("BEGIN"));
        assert!(!json.contains("super-secret-passphrase"));
        assert!(!json.contains("not-a-real-key"));
        assert!(json.contains("\"private_key\":true"));

        std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR");
    }

    #[test]
    fn inspect_missing_slot_is_all_false_and_empty_root_lists_nothing() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", dir.path());

        assert!(list_credential_slots().is_empty());
        assert!(list_credential_slot_presence().is_empty());
        let missing = inspect_credential_slot("hpc").unwrap();
        assert_eq!(
            missing,
            SlotPresence {
                name: "hpc".into(),
                private_key: false,
                public_key: false,
                passphrase: false,
                known_hosts: false,
            }
        );

        std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR");
    }

    #[test]
    fn inspect_rejects_unsafe_slot_names() {
        assert!(inspect_credential_slot("../etc").is_err());
        assert!(inspect_credential_slot("hpc/../root").is_err());
        assert!(inspect_credential_slot("").is_err());
    }

    #[test]
    fn inspect_known_hosts_falls_back_to_root_file() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let slot = dir.path().join("hpc");
        std::fs::create_dir(&slot).unwrap();
        write_mode600(&slot.join("private_key"), "slot-key");
        std::fs::write(
            dir.path().join("known_hosts"),
            "login.example.org ssh-ed25519 AAAA",
        )
        .unwrap();
        std::env::set_var("BEAMPIPE_SSH_CREDENTIALS_DIR", dir.path());

        let presence = inspect_credential_slot("hpc").unwrap();
        assert!(presence.private_key);
        assert!(!presence.public_key);
        assert!(presence.known_hosts);

        std::env::remove_var("BEAMPIPE_SSH_CREDENTIALS_DIR");
    }
}

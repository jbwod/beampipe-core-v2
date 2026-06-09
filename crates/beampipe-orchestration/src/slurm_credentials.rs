//! Resolve Slurm SSH private keys and known-hosts policy from environment.

use crate::OrchestrationError;
use beampipe_security::{
    allow_inline_secrets_override, bool_env, is_process_production, process_env_name,
};
use russh::keys::{decode_secret_key, load_secret_key, PrivateKey};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Resolved SSH material for Slurm login nodes (process-wide from env).
#[derive(Clone)]
pub struct SlurmSshCredentials {
    pub key_source: SlurmKeySource,
    pub known_hosts_path: Option<String>,
    pub strict_known_hosts: bool,
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
    process_env_name()
}

pub fn is_production_env() -> bool {
    is_process_production()
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

fn resolve_known_hosts_path() -> Option<String> {
    first_non_empty(&["SLURM_SSH_KNOWN_HOSTS", "SLURM_SSH_KNOWN_HOSTS_SOURCE"])
}

fn resolve_passphrase() -> Result<Option<Zeroizing<String>>, OrchestrationError> {
    // Passphrase / passcode (operator wording) — file takes precedence over inline env.
    if let Some(path) = first_non_empty(&[
        "SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE",
        "SLURM_SSH_PRIVATE_KEY_PASSCODE_FILE",
    ]) {
        let mut buf = String::new();
        std::fs::File::open(&path)
            .and_err_path(&path)?
            .read_to_string(&mut buf)
            .map_err(|e| OrchestrationError::Backend(format!("read passphrase file: {e}")))?;
        let t = buf.trim_end_matches(['\r', '\n']).to_string();
        return Ok(if t.is_empty() {
            None
        } else {
            Some(Zeroizing::new(t))
        });
    }
    Ok(first_non_empty(&[
        "SLURM_SSH_PRIVATE_KEY_PASSPHRASE",
        "SLURM_SSH_PRIVATE_KEY_PASSCODE",
        // Legacy Python / manual_ssh parity
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
    if mode & 0o077 != 0 {
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

#[cfg(not(unix))]
fn check_private_key_permissions(_path: &Path) -> Result<(), OrchestrationError> {
    Ok(())
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
        " — set SLURM_SSH_PRIVATE_KEY_PASSPHRASE or SLURM_SSH_PRIVATE_KEY_PASSCODE (or *_FILE)"
    } else {
        ""
    };
    if is_production_env() {
        OrchestrationError::Backend(format!("load SSH key: {msg}{hint}"))
    } else {
        OrchestrationError::Backend(format!("load SSH key {}: {msg}{hint}", path.display()))
    }
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
        let strict_known_hosts = strict_known_hosts_default();
        let known_hosts_path = resolve_known_hosts_path();

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
                    "known_hosts required: set SLURM_SSH_KNOWN_HOSTS or SLURM_SSH_KNOWN_HOSTS_SOURCE when strict host verification is enabled"
                        .into(),
                ));
            }
            crate::slurm_ssh::load_known_host_keys(kh)?;
        }

        let key_source = if let Some(pem) = first_non_empty(&["SLURM_SSH_PRIVATE_KEY"]) {
            if is_production_env() && !allow_inline_secrets_override() {
                return Err(OrchestrationError::Backend(
                    "SLURM_SSH_PRIVATE_KEY inline PEM is not allowed in production; use SLURM_SSH_PRIVATE_KEY_PATH or SLURM_SSH_PRIVATE_KEY_FILE, or set BEAMPIPE_ALLOW_INLINE_SECRETS=true"
                        .into(),
                ));
            }
            SlurmKeySource::Pem(Zeroizing::new(pem.into_bytes()))
        } else if let Some(path) =
            first_non_empty(&["SLURM_SSH_PRIVATE_KEY_PATH", "SLURM_SSH_PRIVATE_KEY_FILE"])
        {
            let pb = PathBuf::from(&path);
            check_private_key_permissions(&pb)?;
            SlurmKeySource::Path(pb)
        } else if let Some(path) = home_ssh_fallback() {
            check_private_key_permissions(&path)?;
            SlurmKeySource::DevHome(path)
        } else {
            return Err(OrchestrationError::Backend(
                "no Slurm SSH private key: set SLURM_SSH_PRIVATE_KEY, SLURM_SSH_PRIVATE_KEY_PATH, or SLURM_SSH_PRIVATE_KEY_FILE (or BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK=true for ~/.ssh)"
                    .into(),
            ));
        };

        Ok(Self {
            key_source,
            known_hosts_path,
            strict_known_hosts,
        })
    }

    pub fn load_private_key(&self) -> Result<PrivateKey, OrchestrationError> {
        let passphrase = resolve_passphrase()?;
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
        Self::resolve().is_ok()
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
        std::env::set_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE", &pass_path);
        let passphrase = resolve_passphrase().unwrap().unwrap();
        assert_eq!(passphrase.as_str(), "  secret with spaces  ");
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE");
    }

    #[test]
    fn debug_redacts_inline_private_key() {
        let creds = SlurmSshCredentials {
            key_source: SlurmKeySource::Pem(Zeroizing::new(b"PRIVATE KEY MATERIAL".to_vec())),
            known_hosts_path: Some("/tmp/known_hosts".into()),
            strict_known_hosts: true,
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
}

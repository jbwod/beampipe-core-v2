//! Persistent `russh` sessions to Slurm login nodes for batched polling and deploy.

use crate::slurm_batch::{
    chunk_job_ids, merge_squeue_sacct_batch, parse_sacct_batch, parse_squeue_batch,
    SlurmJobPollResult,
};
use crate::slurm_credentials::SlurmSshCredentials;
use crate::OrchestrationError;
use beampipe_profiles::SlurmRemoteDeploymentConfig;
use russh::client;
use russh::keys::PrivateKeyWithHashAlg;
use russh::ChannelMsg;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::BufRead;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const SQUEUE_FORMAT: &str = "%i|%T|%R";
const SACCT_FORMAT: &str = "JobID,State,ExitCode";

/// Hashable SSH target for session pooling (credentials are process-wide from env).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SlurmTarget {
    pub login_node: String,
    pub ssh_port: u16,
    pub remote_user: String,
}

impl SlurmTarget {
    pub fn from_deployment(deployment: &SlurmRemoteDeploymentConfig, username: &str) -> Self {
        Self {
            login_node: deployment.login_node.clone(),
            ssh_port: deployment.ssh_port.max(1).min(u16::MAX as i32) as u16,
            remote_user: username.to_string(),
        }
    }

    pub fn advisory_lock_key(&self) -> i64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        (hasher.finish() & i64::MAX as u64) as i64
    }
}

struct SshClientHandler {
    trusted: Option<Vec<KnownHostEntry>>,
    strict_known_hosts: bool,
    target_host: String,
    target_port: u16,
}

impl SshClientHandler {
    fn from_credentials(
        creds: &SlurmSshCredentials,
        target: &SlurmTarget,
    ) -> Result<Self, OrchestrationError> {
        let trusted = if let Some(path) = creds.known_hosts_path.as_ref() {
            let path = path.trim();
            if path.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(load_known_host_entries(path)?)
            }
        } else {
            None
        };
        Ok(Self {
            trusted,
            strict_known_hosts: creds.strict_known_hosts,
            target_host: target.login_node.clone(),
            target_port: target.ssh_port,
        })
    }
}

#[derive(Debug, Clone)]
pub struct KnownHostEntry {
    patterns: Vec<String>,
    key: ssh_key::PublicKey,
}

impl KnownHostEntry {
    fn matches_target(&self, host: &str, port: u16) -> bool {
        self.patterns
            .iter()
            .any(|pattern| known_host_pattern_matches(pattern, host, port))
    }
}

pub fn load_known_host_entries(path: &str) -> Result<Vec<KnownHostEntry>, OrchestrationError> {
    let file = std::fs::File::open(path)
        .map_err(|e| OrchestrationError::Backend(format!("open known_hosts {path}: {e}")))?;
    let mut entries = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let host_field = parts.next();
        let Some(host_field) = host_field else {
            continue;
        };
        if host_field.starts_with("|1|") {
            return Err(OrchestrationError::Backend(
                "hashed known_hosts entries are not supported; provide plain host patterns for Slurm login nodes"
                    .into(),
            ));
        }
        let host_field = if host_field.starts_with('@') {
            let Some(next) = parts.next() else {
                continue;
            };
            if next.starts_with("|1|") {
                return Err(OrchestrationError::Backend(
                    "hashed known_hosts entries are not supported; provide plain host patterns for Slurm login nodes"
                        .into(),
                ));
            }
            next
        } else {
            host_field
        };
        let key_type = parts.next();
        let key_b64 = parts.next();
        let (Some(key_type), Some(key_b64)) = (key_type, key_b64) else {
            continue;
        };
        let line = format!("{key_type} {key_b64}");
        if let Ok(key) = line.parse::<ssh_key::PublicKey>() {
            let patterns = host_field
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            if !patterns.is_empty() {
                entries.push(KnownHostEntry { patterns, key });
            }
        }
    }
    if entries.is_empty() {
        return Err(OrchestrationError::Backend(format!(
            "no public keys parsed from known_hosts file {path}"
        )));
    }
    Ok(entries)
}

pub fn load_known_host_keys(path: &str) -> Result<Vec<ssh_key::PublicKey>, OrchestrationError> {
    Ok(load_known_host_entries(path)?
        .into_iter()
        .map(|entry| entry.key)
        .collect())
}

pub fn known_hosts_has_target(
    path: &str,
    host: &str,
    port: u16,
) -> Result<bool, OrchestrationError> {
    Ok(load_known_host_entries(path)?
        .iter()
        .any(|entry| entry.matches_target(host, port)))
}

fn known_host_pattern_matches(pattern: &str, host: &str, port: u16) -> bool {
    if pattern.starts_with('!') {
        return false;
    }
    if let Some((bracket_host, bracket_port)) = parse_bracket_host_port(pattern) {
        return bracket_port == port && wildcard_match(bracket_host, host);
    }
    port == 22 && wildcard_match(pattern, host)
}

fn parse_bracket_host_port(pattern: &str) -> Option<(&str, u16)> {
    let rest = pattern.strip_prefix('[')?;
    let (host, port_part) = rest.split_once("]:")?;
    let port = port_part.parse().ok()?;
    Some((host, port))
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    fn inner(pattern: &[u8], value: &[u8]) -> bool {
        match pattern.split_first() {
            None => value.is_empty(),
            Some((&b'*', rest)) => {
                inner(rest, value) || (!value.is_empty() && inner(pattern, &value[1..]))
            }
            Some((&b'?', rest)) => !value.is_empty() && inner(rest, &value[1..]),
            Some((&p, rest)) => {
                !value.is_empty() && p.eq_ignore_ascii_case(&value[0]) && inner(rest, &value[1..])
            }
        }
    }
    inner(pattern.as_bytes(), value.as_bytes())
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        if let Some(trusted) = &self.trusted {
            return Ok(trusted.iter().any(|entry| {
                entry.key == *server_public_key
                    && entry.matches_target(&self.target_host, self.target_port)
            }));
        }
        if self.strict_known_hosts {
            return Ok(false);
        }
        // Non-strict dev only: allow unknown keys (discouraged; use known_hosts in production).
        Ok(!crate::slurm_credentials::is_production_env())
    }
}

/// One authenticated SSH session to a login node.
pub struct SlurmSshSession {
    handle: client::Handle<SshClientHandler>,
}

impl SlurmSshSession {
    pub async fn connect(target: &SlurmTarget) -> Result<Self, OrchestrationError> {
        let creds = SlurmSshCredentials::resolve()?;
        Self::connect_with_credentials(target, &creds).await
    }

    pub async fn connect_with_credentials(
        target: &SlurmTarget,
        creds: &SlurmSshCredentials,
    ) -> Result<Self, OrchestrationError> {
        let key_pair = creds.load_private_key()?;
        let handler = SshClientHandler::from_credentials(creds, target)?;
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(300)),
            ..Default::default()
        });
        let addr = (target.login_node.as_str(), target.ssh_port);
        let mut handle = client::connect(config, addr, handler).await.map_err(|e| {
            OrchestrationError::Backend(format!(
                "SSH connect {}@{}: {e}",
                target.remote_user, target.login_node
            ))
        })?;

        let rsa_hash = handle
            .best_supported_rsa_hash()
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH RSA hash: {e}")))?;
        let auth = handle
            .authenticate_publickey(
                &target.remote_user,
                PrivateKeyWithHashAlg::new(Arc::new(key_pair), rsa_hash.flatten()),
            )
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH auth: {e}")))?;
        if !auth.success() {
            return Err(OrchestrationError::Backend(format!(
                "SSH publickey auth failed for {}@{}",
                target.remote_user, target.login_node
            )));
        }
        Ok(Self { handle })
    }

    pub async fn run_command(&mut self, command: &str) -> Result<String, OrchestrationError> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH channel: {e}")))?;
        channel
            .exec(true, command)
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH exec: {e}")))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status: Option<u32> = None;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status: code } => exit_status = Some(code),
                _ => {}
            }
        }
        if let Some(code) = exit_status {
            if code != 0 {
                let out = String::from_utf8_lossy(&stdout);
                let err = String::from_utf8_lossy(&stderr);
                return Err(OrchestrationError::Backend(format!(
                    "remote command failed (exit={code}): {command:?}\nstdout: {out}\nstderr: {err}"
                )));
            }
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// Upload file content via remote `tee` (shell-escaped path).
    pub async fn upload_text(
        &mut self,
        remote_path: &str,
        content: &str,
    ) -> Result<(), OrchestrationError> {
        let escaped = shell_escape_single(remote_path);
        let cmd = format!("tee {escaped}");
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH channel: {e}")))?;
        channel
            .exec(false, cmd.as_str())
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH tee exec: {e}")))?;
        channel
            .data(content.as_bytes())
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH tee write: {e}")))?;
        channel
            .eof()
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH tee eof: {e}")))?;

        let mut exit_status: Option<u32> = None;
        while let Some(msg) = channel.wait().await {
            if let ChannelMsg::ExitStatus { exit_status: code } = msg {
                exit_status = Some(code);
            }
        }
        if exit_status != Some(0) {
            return Err(OrchestrationError::Backend(format!(
                "ssh tee failed for {remote_path:?} (exit={exit_status:?})"
            )));
        }
        Ok(())
    }

    pub async fn close(self) -> Result<(), OrchestrationError> {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await;
        Ok(())
    }
}

fn shell_escape_single(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        format!("'{s}'")
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

struct PooledEntry {
    session: SlurmSshSession,
    last_used: Instant,
}

/// Reuse `russh` sessions per login target with idle eviction.
pub struct SlurmSshPool {
    inner: Mutex<HashMap<SlurmTarget, PooledEntry>>,
    idle_seconds: u64,
}

impl SlurmSshPool {
    pub fn new_from_env() -> Self {
        let idle_seconds = std::env::var("BEAMPIPE_SLURM_SSH_IDLE_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        Self {
            inner: Mutex::new(HashMap::new()),
            idle_seconds,
        }
    }

    pub async fn query_slurm_states(
        &self,
        target: &SlurmTarget,
        job_ids: &[String],
    ) -> Result<HashMap<String, SlurmJobPollResult>, OrchestrationError> {
        let mut guard = self.inner.lock().await;
        self.evict_idle_locked(&mut guard).await;
        if !guard.contains_key(target) {
            let session = SlurmSshSession::connect(target).await?;
            guard.insert(
                target.clone(),
                PooledEntry {
                    session,
                    last_used: Instant::now(),
                },
            );
        }
        let entry = guard.get_mut(target).expect("session inserted above");
        entry.last_used = Instant::now();
        let result = query_slurm_states_batch(&mut entry.session, job_ids).await;
        if result.is_err() {
            if let Some(removed) = guard.remove(target) {
                let _ = removed.session.close().await;
            }
        }
        result
    }

    pub fn active_session_count(&self) -> usize {
        self.inner.try_lock().map(|g| g.len()).unwrap_or(0)
    }

    async fn evict_idle_locked(&self, guard: &mut HashMap<SlurmTarget, PooledEntry>) {
        let idle = Duration::from_secs(self.idle_seconds);
        let now = Instant::now();
        let stale: Vec<SlurmTarget> = guard
            .iter()
            .filter(|(_, e)| now.duration_since(e.last_used) > idle)
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale {
            if let Some(entry) = guard.remove(&key) {
                let _ = entry.session.close().await;
            }
        }
    }
}

pub async fn query_slurm_states_batch(
    session: &mut SlurmSshSession,
    job_ids: &[String],
) -> Result<HashMap<String, SlurmJobPollResult>, OrchestrationError> {
    if job_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut squeue_all = HashMap::new();
    let mut sacct_all = HashMap::new();
    for chunk in chunk_job_ids(job_ids) {
        let joined = chunk.join(",");
        let squeue_cmd = format!("squeue -h -j {joined} -o {SQUEUE_FORMAT} 2>/dev/null || true");
        let squeue_out = session.run_command(&squeue_cmd).await?;
        squeue_all.extend(parse_squeue_batch(&squeue_out));

        let missing: Vec<String> = chunk
            .iter()
            .filter(|id| !squeue_all.contains_key(*id))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let sacct_joined = missing.join(",");
            let sacct_cmd = format!(
                "sacct -j {sacct_joined} --format={SACCT_FORMAT} -P -n 2>/dev/null || true"
            );
            let sacct_out = session.run_command(&sacct_cmd).await?;
            sacct_all.extend(parse_sacct_batch(&sacct_out));
        }
    }
    Ok(merge_squeue_sacct_batch(job_ids, &squeue_all, &sacct_all))
}

#[cfg(test)]
mod tests {
    use super::{known_hosts_has_target, load_known_host_keys};

    fn generate_public_key(dir: &tempfile::TempDir) -> String {
        let key_path = dir.path().join("id_test");
        let status = std::process::Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                key_path.to_str().unwrap(),
                "-N",
                "",
                "-q",
            ])
            .status()
            .expect("ssh-keygen");
        assert!(status.success(), "ssh-keygen failed");
        std::fs::read_to_string(key_path.with_extension("pub")).unwrap()
    }

    #[test]
    fn load_known_host_keys_rejects_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::File::create(&path).unwrap();
        assert!(load_known_host_keys(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn known_hosts_match_target_host_and_default_port() {
        let dir = tempfile::tempdir().unwrap();
        let pubkey = generate_public_key(&dir);
        let key = pubkey.split_whitespace().collect::<Vec<_>>();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, format!("login-a.example {} {}\n", key[0], key[1])).unwrap();
        assert!(known_hosts_has_target(path.to_str().unwrap(), "login-a.example", 22).unwrap());
        assert!(!known_hosts_has_target(path.to_str().unwrap(), "login-b.example", 22).unwrap());
        assert!(!known_hosts_has_target(path.to_str().unwrap(), "login-a.example", 2222).unwrap());
    }

    #[test]
    fn known_hosts_match_bracketed_non_default_port() {
        let dir = tempfile::tempdir().unwrap();
        let pubkey = generate_public_key(&dir);
        let key = pubkey.split_whitespace().collect::<Vec<_>>();
        let path = dir.path().join("known_hosts");
        std::fs::write(
            &path,
            format!("[login-a.example]:2222 {} {}\n", key[0], key[1]),
        )
        .unwrap();
        assert!(known_hosts_has_target(path.to_str().unwrap(), "login-a.example", 2222).unwrap());
        assert!(!known_hosts_has_target(path.to_str().unwrap(), "login-a.example", 22).unwrap());
    }

    #[test]
    fn known_hosts_rejects_hashed_host_entries() {
        let dir = tempfile::tempdir().unwrap();
        let pubkey = generate_public_key(&dir);
        let key = pubkey.split_whitespace().collect::<Vec<_>>();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, format!("|1|salt|hash {} {}\n", key[0], key[1])).unwrap();
        let err = load_known_host_keys(path.to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("hashed known_hosts entries are not supported"));
    }

    #[test]
    fn strict_resolve_requires_known_hosts_path() {
        std::env::set_var("BEAMPIPE_ENV", "development");
        std::env::set_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS", "true");
        std::env::remove_var("SLURM_SSH_KNOWN_HOSTS");
        std::env::remove_var("SLURM_SSH_KNOWN_HOSTS_SOURCE");
        std::env::set_var("SLURM_SSH_PRIVATE_KEY", "not-valid-pem");
        assert!(crate::slurm_credentials::SlurmSshCredentials::resolve().is_err());
        std::env::remove_var("SLURM_SSH_PRIVATE_KEY");
        std::env::remove_var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS");
        std::env::remove_var("BEAMPIPE_ENV");
    }
}

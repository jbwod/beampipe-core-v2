//! Persistent `russh` sessions to Slurm login nodes for batched polling.

use crate::slurm_batch::{
    chunk_job_ids, merge_squeue_sacct_batch, parse_sacct_batch, parse_squeue_batch,
    SlurmJobPollResult,
};
use crate::slurm_deploy::ssh_option_args;
use crate::OrchestrationError;
use beampipe_profiles::SlurmRemoteDeploymentConfig;
use russh::client;
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use russh::ChannelMsg;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const SQUEUE_FORMAT: &str = "%i|%T|%R";
const SACCT_FORMAT: &str = "JobID,State,ExitCode";

/// Hashable SSH target for session pooling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlurmTarget {
    pub login_node: String,
    pub ssh_port: u16,
    pub remote_user: String,
    pub private_key_file: Option<String>,
    pub known_hosts_file: Option<String>,
}

impl Hash for SlurmTarget {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.login_node.hash(state);
        self.ssh_port.hash(state);
        self.remote_user.hash(state);
        self.private_key_file.hash(state);
        self.known_hosts_file.hash(state);
    }
}

impl SlurmTarget {
    pub fn from_deployment(deployment: &SlurmRemoteDeploymentConfig, username: &str) -> Self {
        let mut private_key_file = None;
        let mut known_hosts_file = None;
        let args = ssh_option_args();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-i" && i + 1 < args.len() {
                private_key_file = Some(args[i + 1].clone());
                i += 2;
                continue;
            }
            if let Some(rest) = args[i].strip_prefix("UserKnownHostsFile=") {
                known_hosts_file = Some(rest.to_string());
            }
            i += 1;
        }
        if private_key_file.is_none() {
            if let Ok(key) = std::env::var("SLURM_SSH_PRIVATE_KEY_FILE") {
                if !key.trim().is_empty() {
                    private_key_file = Some(key);
                }
            }
        }
        if known_hosts_file.is_none() {
            if let Ok(hosts) = std::env::var("SLURM_SSH_KNOWN_HOSTS_SOURCE") {
                if !hosts.trim().is_empty() {
                    known_hosts_file = Some(hosts);
                }
            }
        }
        Self {
            login_node: deployment.login_node.clone(),
            ssh_port: deployment.ssh_port.max(1).min(u16::MAX as i32) as u16,
            remote_user: username.to_string(),
            private_key_file,
            known_hosts_file,
        }
    }

    pub fn advisory_lock_key(&self) -> i64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        (hasher.finish() & i64::MAX as u64) as i64
    }
}

struct SshClientHandler {
    trusted: Option<Vec<ssh_key::PublicKey>>,
    strict_known_hosts: bool,
}

impl SshClientHandler {
    fn for_target(target: &SlurmTarget) -> Result<Self, OrchestrationError> {
        let strict_known_hosts = std::env::var("BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let trusted = if let Some(path) = target.known_hosts_file.as_ref() {
            let path = path.trim();
            if path.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(load_known_host_keys(path)?)
            }
        } else if strict_known_hosts {
            return Err(OrchestrationError::Backend(
                "SLURM_SSH_KNOWN_HOSTS_SOURCE required when BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS=true"
                    .into(),
            ));
        } else {
            None
        };
        Ok(Self {
            trusted,
            strict_known_hosts,
        })
    }
}

fn load_known_host_keys(path: &str) -> Result<Vec<ssh_key::PublicKey>, OrchestrationError> {
    let file = std::fs::File::open(path)
        .map_err(|e| OrchestrationError::Backend(format!("open known_hosts {path}: {e}")))?;
    let mut keys = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let _host = parts.next();
        let key_type = parts.next();
        let key_b64 = parts.next();
        let (Some(key_type), Some(key_b64)) = (key_type, key_b64) else {
            continue;
        };
        let line = format!("{key_type} {key_b64}");
        if let Ok(key) = line.parse::<ssh_key::PublicKey>() {
            keys.push(key);
        }
    }
    if keys.is_empty() {
        return Err(OrchestrationError::Backend(format!(
            "no public keys parsed from known_hosts file {path}"
        )));
    }
    Ok(keys)
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        if let Some(trusted) = &self.trusted {
            return Ok(trusted.iter().any(|k| k == server_public_key));
        }
        if self.strict_known_hosts {
            return Ok(false);
        }
        Ok(true)
    }
}

/// One authenticated SSH session to a login node.
pub struct SlurmSshSession {
    handle: client::Handle<SshClientHandler>,
}

impl SlurmSshSession {
    pub async fn connect(target: &SlurmTarget) -> Result<Self, OrchestrationError> {
        let key_path = resolve_private_key_path(target)?;
        let key_pair = load_secret_key(&key_path, None).map_err(|e| {
            OrchestrationError::Backend(format!("load SSH key {}: {e}", key_path.display()))
        })?;

        let handler = SshClientHandler::for_target(target)?;
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(300)),
            ..Default::default()
        });
        let addr = (target.login_node.as_str(), target.ssh_port);
        let mut handle = client::connect(config, addr, handler).await.map_err(|e| {
            OrchestrationError::Backend(format!("SSH connect {}: {e}", target.login_node))
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
        let mut exit_status: Option<u32> = None;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status: code } => exit_status = Some(code),
                _ => {}
            }
        }
        if let Some(code) = exit_status {
            if code != 0 {
                let out = String::from_utf8_lossy(&stdout);
                return Err(OrchestrationError::Backend(format!(
                    "remote command failed (exit={code}): {command:?}\n{out}"
                )));
            }
        }
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    pub async fn close(self) -> Result<(), OrchestrationError> {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await;
        Ok(())
    }
}

fn resolve_private_key_path(target: &SlurmTarget) -> Result<PathBuf, OrchestrationError> {
    if let Some(path) = target.private_key_file.as_ref() {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    for name in ["id_ed25519", "id_rsa"] {
        let p = PathBuf::from(&home).join(".ssh").join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(OrchestrationError::Backend(
        "no SSH private key: set SLURM_SSH_PRIVATE_KEY_FILE or place id_ed25519/id_rsa in ~/.ssh"
            .into(),
    ))
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

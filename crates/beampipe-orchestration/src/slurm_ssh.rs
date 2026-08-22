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

/// Hashable SSH target for session pooling. Credential slot is part of the key
/// so two profiles that share a login node but use different keys stay isolated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SlurmTarget {
    pub login_node: String,
    pub ssh_port: u16,
    pub remote_user: String,
    pub credential_slot: Option<String>,
}

impl SlurmTarget {
    pub fn from_deployment(deployment: &SlurmRemoteDeploymentConfig, username: &str) -> Self {
        Self {
            login_node: deployment.login_node.clone(),
            ssh_port: deployment.ssh_port.max(1).min(u16::MAX as i32) as u16,
            remote_user: username.to_string(),
            credential_slot: deployment
                .ssh_credential
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
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
        known_host_patterns_match(&self.patterns, host, port)
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
        if host_field.starts_with('@') {
            return Err(OrchestrationError::Backend(format!(
                "known_hosts marker {host_field} is not supported; revoked and certificate-authority entries cannot be used as direct Slurm host keys"
            )));
        }
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
    if let Some((bracket_host, bracket_port)) = parse_bracket_host_port(pattern) {
        return bracket_port == port && wildcard_match(bracket_host, host);
    }
    port == 22 && wildcard_match(pattern, host)
}

fn known_host_patterns_match(patterns: &[String], host: &str, port: u16) -> bool {
    let excluded = patterns.iter().any(|pattern| {
        pattern
            .strip_prefix('!')
            .is_some_and(|pattern| known_host_pattern_matches(pattern, host, port))
    });
    !excluded
        && patterns.iter().any(|pattern| {
            !pattern.starts_with('!') && known_host_pattern_matches(pattern, host, port)
        })
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
        let creds = SlurmSshCredentials::resolve_for(target.credential_slot.as_deref())?;
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
        self.run_command_inner(command, RemoteCommandKind::Ordinary)
            .await
    }

    /// Run the scheduler submission command while retaining the distinction
    /// between a definite command failure and a lost submission response.
    ///
    /// Opening the channel happens before dispatch and an explicit non-zero
    /// exit is a definite failure. Once the exec request may have reached the
    /// remote shell, losing its response or exit status leaves the scheduler
    /// outcome uncertain and must prevent an automatic resubmission.
    pub async fn run_submission_command(
        &mut self,
        command: &str,
    ) -> Result<String, OrchestrationError> {
        self.run_command_inner(command, RemoteCommandKind::Submission)
            .await
    }

    async fn run_command_inner(
        &mut self,
        command: &str,
        kind: RemoteCommandKind,
    ) -> Result<String, OrchestrationError> {
        let output = self.run_command_output_inner(command, kind).await?;
        command_stdout(command, output)
    }

    async fn run_command_output(
        &mut self,
        command: &str,
    ) -> Result<RemoteCommandOutput, OrchestrationError> {
        self.run_command_output_inner(command, RemoteCommandKind::Ordinary)
            .await
    }

    async fn run_command_output_inner(
        &mut self,
        command: &str,
        kind: RemoteCommandKind,
    ) -> Result<RemoteCommandOutput, OrchestrationError> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| OrchestrationError::Backend(format!("SSH channel: {e}")))?;
        channel
            .exec(true, command)
            .await
            .map_err(|error| {
                remote_command_transport_error(
                    kind,
                    format!("SSH exec response was not observed for {command:?}: {error}"),
                )
            })?;

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
        let Some(code) = exit_status else {
            return Err(remote_command_transport_error(
                kind,
                format!("remote command ended without an SSH exit status: {command:?}"),
            ));
        };
        Ok(RemoteCommandOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_status: code,
        })
    }

    /// Upload file content via remote `tee` (shell-escaped path).
    ///
    /// Submission artifacts can contain short-lived signed data URLs. Apply a
    /// restrictive umask in the same remote shell that creates the file so a
    /// permissive login-node default cannot expose them to group/other users.
    pub async fn upload_text(
        &mut self,
        remote_path: &str,
        content: &str,
    ) -> Result<(), OrchestrationError> {
        let cmd = upload_text_command(remote_path);
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

    /// Upload through a same-directory temporary file and atomically rename it.
    pub async fn upload_text_atomic(
        &mut self,
        remote_path: &str,
        content: &str,
    ) -> Result<(), OrchestrationError> {
        let temporary_path = format!("{remote_path}.tmp-{}", uuid::Uuid::now_v7().simple());
        self.upload_text(&temporary_path, content).await?;
        let result = self
            .run_command(&format!(
                "mv -- {} {}",
                shell_escape_single(&temporary_path),
                shell_escape_single(remote_path)
            ))
            .await;
        if result.is_err() {
            let _ = self
                .run_command(&format!(
                    "rm -f -- {}",
                    shell_escape_single(&temporary_path)
                ))
                .await;
        }
        result.map(|_| ())
    }

    pub async fn close(self) -> Result<(), OrchestrationError> {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteCommandKind {
    Ordinary,
    Submission,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteCommandOutput {
    stdout: String,
    stderr: String,
    exit_status: u32,
}

fn remote_command_transport_error(
    kind: RemoteCommandKind,
    detail: String,
) -> OrchestrationError {
    match kind {
        RemoteCommandKind::Ordinary => OrchestrationError::Backend(detail),
        RemoteCommandKind::Submission => OrchestrationError::SubmissionUncertain(detail),
    }
}

fn remote_command_exit_error(
    command: &str,
    code: u32,
    stdout: &str,
    stderr: &str,
) -> OrchestrationError {
    OrchestrationError::Backend(format!(
        "remote command failed (exit={code}): {command:?}\nstdout: {stdout}\nstderr: {stderr}"
    ))
}

fn command_stdout(
    command: &str,
    output: RemoteCommandOutput,
) -> Result<String, OrchestrationError> {
    if output.exit_status == 0 {
        return Ok(output.stdout);
    }
    Err(remote_command_exit_error(
        command,
        output.exit_status,
        &output.stdout,
        &output.stderr,
    ))
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

fn upload_text_command(remote_path: &str) -> String {
    format!("umask 077 && tee {}", shell_escape_single(remote_path))
}

struct PooledEntry {
    session: SlurmSshSession,
    last_used: Instant,
}

#[derive(Default)]
struct PooledTargetState {
    entry: Option<PooledEntry>,
}

/// Reuse `russh` sessions per login target with idle eviction.
pub struct SlurmSshPool {
    inner: Mutex<HashMap<SlurmTarget, Arc<Mutex<PooledTargetState>>>>,
    idle_seconds: u64,
}

impl SlurmSshPool {
    pub fn new_from_env() -> Self {
        let idle_seconds = std::env::var("BEAMPIPE_SLURM_SSH_IDLE_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        Self::with_idle_seconds(idle_seconds)
    }

    fn with_idle_seconds(idle_seconds: u64) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            idle_seconds,
        }
    }

    async fn target_state(&self, target: &SlurmTarget) -> Arc<Mutex<PooledTargetState>> {
        let mut targets = self.inner.lock().await;
        targets
            .entry(target.clone())
            .or_insert_with(|| Arc::new(Mutex::new(PooledTargetState::default())))
            .clone()
    }

    pub async fn query_slurm_states(
        &self,
        target: &SlurmTarget,
        job_ids: &[String],
    ) -> Result<HashMap<String, SlurmJobPollResult>, OrchestrationError> {
        let target_state = self.target_state(target).await;
        let mut state = target_state.lock().await;
        let idle = Duration::from_secs(self.idle_seconds);
        if state
            .entry
            .as_ref()
            .is_some_and(|entry| entry.last_used.elapsed() > idle)
        {
            if let Some(stale) = state.entry.take() {
                let _ = stale.session.close().await;
            }
        }
        if state.entry.is_none() {
            let session = SlurmSshSession::connect(target).await?;
            state.entry = Some(PooledEntry {
                session,
                last_used: Instant::now(),
            });
        }
        let entry = state.entry.as_mut().expect("session inserted above");
        entry.last_used = Instant::now();
        let result = query_slurm_states_batch(&mut entry.session, job_ids).await;
        if result.is_err() {
            if let Some(failed) = state.entry.take() {
                let _ = failed.session.close().await;
            }
        }
        result
    }

    pub fn active_session_count(&self) -> usize {
        self.inner
            .try_lock()
            .map(|targets| {
                targets
                    .values()
                    .filter(|target| {
                        target
                            .try_lock()
                            .map(|state| state.entry.is_some())
                            .unwrap_or(true)
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

fn squeue_query_command(job_ids: &str) -> String {
    format!("squeue -h -j {job_ids} -o {SQUEUE_FORMAT}")
}

fn sacct_query_command(job_ids: &str) -> String {
    format!("sacct -j {job_ids} --format={SACCT_FORMAT} -P -n")
}

fn is_missing_squeue_job_error(stderr: &str) -> bool {
    let mut lines = stderr.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(first) = lines.next() else {
        return false;
    };
    std::iter::once(first).chain(lines).all(|line| {
        line.to_ascii_lowercase()
            .ends_with("invalid job id specified")
    })
}

fn squeue_stdout(
    command: &str,
    output: RemoteCommandOutput,
) -> Result<String, OrchestrationError> {
    if output.exit_status == 0 || is_missing_squeue_job_error(&output.stderr) {
        return Ok(output.stdout);
    }
    Err(remote_command_exit_error(
        command,
        output.exit_status,
        &output.stdout,
        &output.stderr,
    ))
}

pub async fn query_slurm_states_batch(
    session: &mut SlurmSshSession,
    job_ids: &[String],
) -> Result<HashMap<String, SlurmJobPollResult>, OrchestrationError> {
    if job_ids.is_empty() {
        return Ok(HashMap::new());
    }
    for job_id in job_ids {
        validate_slurm_job_id(job_id)?;
    }
    let mut squeue_all = HashMap::new();
    let mut sacct_all = HashMap::new();
    for chunk in chunk_job_ids(job_ids) {
        let joined = chunk.join(",");
        let squeue_cmd = squeue_query_command(&joined);
        let squeue_out = squeue_stdout(
            &squeue_cmd,
            session.run_command_output(&squeue_cmd).await?,
        )?;
        squeue_all.extend(parse_squeue_batch(&squeue_out));

        let missing: Vec<String> = chunk
            .iter()
            .filter(|id| !squeue_all.contains_key(*id))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let sacct_joined = missing.join(",");
            let sacct_cmd = sacct_query_command(&sacct_joined);
            let sacct_out = session.run_command(&sacct_cmd).await?;
            sacct_all.extend(parse_sacct_batch(&sacct_out));
        }
    }
    Ok(merge_squeue_sacct_batch(job_ids, &squeue_all, &sacct_all))
}

pub fn validate_slurm_job_id(job_id: &str) -> Result<(), OrchestrationError> {
    if job_id.is_empty() || !job_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(OrchestrationError::Backend(
            "Slurm job ID must contain ASCII digits only".into(),
        ));
    }
    Ok(())
}

pub fn scancel_command(job_id: &str) -> Result<String, OrchestrationError> {
    validate_slurm_job_id(job_id)?;
    Ok(format!("scancel -- {job_id}"))
}

#[cfg(test)]
mod tests {
    use super::{
        command_stdout, is_missing_squeue_job_error,
        known_host_patterns_match, known_hosts_has_target, load_known_host_keys, scancel_command,
        remote_command_transport_error, sacct_query_command, squeue_query_command, squeue_stdout,
        upload_text_command, validate_slurm_job_id, RemoteCommandKind, RemoteCommandOutput,
        SlurmSshPool, SlurmTarget,
    };
    use crate::OrchestrationError;
    use std::sync::Arc;

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
    fn uploaded_submission_artifacts_are_created_private() {
        let command = upload_text_command("/scratch/session graph.pgt");
        assert!(command.starts_with("umask 077 && tee "));
        assert!(command.contains("'/scratch/session graph.pgt'"));
    }

    #[test]
    fn scheduler_commands_reject_untrusted_job_ids() {
        assert!(validate_slurm_job_id("123456").is_ok());
        assert_eq!(scancel_command("123456").unwrap(), "scancel -- 123456");
        for value in ["", "123_4", "123,456", "123; touch /tmp/bad"] {
            assert!(validate_slurm_job_id(value).is_err(), "accepted {value:?}");
            assert!(scancel_command(value).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn submission_transport_loss_is_uncertain() {
        assert!(matches!(
            remote_command_transport_error(
                RemoteCommandKind::Submission,
                "response lost after dispatch".into()
            ),
            OrchestrationError::SubmissionUncertain(_)
        ));
    }

    #[test]
    fn ordinary_transport_loss_is_backend() {
        assert!(matches!(
            remote_command_transport_error(
                RemoteCommandKind::Ordinary,
                "response lost after dispatch".into()
            ),
            OrchestrationError::Backend(_)
        ));
    }

    #[test]
    fn explicit_nonzero_submission_exit_is_deterministic() {
        assert!(matches!(
            command_stdout(
                "sbatch --parsable job.sh",
                RemoteCommandOutput {
                    stdout: String::new(),
                    stderr: "invalid account".into(),
                    exit_status: 1,
                }
            ),
            Err(OrchestrationError::Backend(_))
        ));
    }

    #[test]
    fn scheduler_poll_commands_do_not_mask_failures() {
        for command in [squeue_query_command("123"), sacct_query_command("123")] {
            assert!(!command.contains("2>/dev/null"));
            assert!(!command.contains("|| true"));
        }
    }

    #[test]
    fn only_the_exact_missing_squeue_diagnostic_falls_back() {
        assert!(is_missing_squeue_job_error(
            "slurm_load_jobs error: Invalid job id specified\n"
        ));
        let output = squeue_stdout(
            "squeue -j 123,456",
            RemoteCommandOutput {
                stdout: "456|RUNNING|None\n".into(),
                stderr: "slurm_load_jobs error: Invalid job id specified\n".into(),
                exit_status: 1,
            },
        )
        .unwrap();
        assert_eq!(output, "456|RUNNING|None\n");

        for stderr in [
            "permission denied",
            "Invalid job id specified\npermission denied",
            "",
        ] {
            assert!(matches!(
                squeue_stdout(
                    "squeue -j 123",
                    RemoteCommandOutput {
                        stdout: String::new(),
                        stderr: stderr.into(),
                        exit_status: 1,
                    }
                ),
                Err(OrchestrationError::Backend(_))
            ));
        }
    }

    #[tokio::test]
    async fn ssh_pool_uses_independent_locks_per_target() {
        let pool = SlurmSshPool::with_idle_seconds(300);
        let target_a = SlurmTarget {
            login_node: "login-a.example".into(),
            ssh_port: 22,
            remote_user: "operator".into(),
            credential_slot: None,
        };
        let target_b = SlurmTarget {
            login_node: "login-b.example".into(),
            ..target_a.clone()
        };

        let first_a = pool.target_state(&target_a).await;
        let second_a = pool.target_state(&target_a).await;
        let first_b = pool.target_state(&target_b).await;
        assert!(Arc::ptr_eq(&first_a, &second_a));
        assert!(!Arc::ptr_eq(&first_a, &first_b));
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
    fn known_hosts_rejects_revoked_markers_without_parsing_key_material() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(
            &path,
            "@revoked login-a.example ssh-ed25519 invalid-key-data\n",
        )
        .unwrap();
        let error = load_known_host_keys(path.to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("@revoked"));
        assert!(error.contains("not supported"));
    }

    #[test]
    fn negated_host_pattern_vetoes_a_positive_wildcard() {
        let patterns = vec!["*".into(), "!bad.example".into()];
        assert!(!known_host_patterns_match(&patterns, "bad.example", 22));
        assert!(known_host_patterns_match(&patterns, "good.example", 22));
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

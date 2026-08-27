//! Persistent `russh` sessions to Slurm login nodes for batched polling and deploy.

use crate::slurm_batch::{
    chunk_job_ids, merge_squeue_sacct_batch, parse_sacct_batch, parse_squeue_batch,
    SlurmJobPollResult,
};
use crate::slurm_credentials::SlurmSshCredentials;
use crate::slurm_sftp::{
    RemoteSftp, PRIVATE_DIRECTORY_MODE, PRIVATE_FILE_MODE, SUBMISSION_ARTIFACT_MODE,
};
use crate::OrchestrationError;
use beampipe_profiles::SlurmRemoteDeploymentConfig;
use russh::client;
use russh::keys::PrivateKeyWithHashAlg;
use russh::ChannelMsg;
use ssh_key::known_hosts::{HostPatterns, KnownHosts};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
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
    let input = std::fs::read_to_string(path)
        .map_err(|e| OrchestrationError::Backend(format!("open known_hosts {path}: {e}")))?;
    let mut entries = Vec::new();
    for entry in KnownHosts::new(&input) {
        let entry = entry.map_err(|error| {
            OrchestrationError::Backend(format!("parse known_hosts {path}: {error}"))
        })?;
        if let Some(marker) = entry.marker() {
            return Err(OrchestrationError::Backend(format!(
                "known_hosts marker {marker} is not supported; revoked and certificate-authority entries cannot be used as direct Slurm host keys"
            )));
        }
        let patterns = match entry.host_patterns() {
            HostPatterns::Patterns(patterns) => patterns.clone(),
            HostPatterns::HashedName { .. } => {
                return Err(OrchestrationError::Backend(
                    "hashed known_hosts entries are not supported; provide plain host patterns for Slurm login nodes"
                        .into(),
                ));
            }
        };
        if !patterns.is_empty() {
            entries.push(KnownHostEntry {
                patterns,
                key: entry.public_key().clone(),
            });
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

#[async_trait::async_trait]
trait AtomicSftpUploader {
    async fn ensure_private_directory(
        &mut self,
        path: &str,
        mode: u32,
    ) -> Result<(), OrchestrationError>;

    async fn upload_file_atomic(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError>;

    async fn upload_private_file_atomic(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError>;
}

#[async_trait::async_trait]
impl AtomicSftpUploader for RemoteSftp {
    async fn ensure_private_directory(
        &mut self,
        path: &str,
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        RemoteSftp::ensure_private_directory(self, path, mode).await
    }

    async fn upload_file_atomic(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        RemoteSftp::upload_file_atomic(self, final_path, temporary_path, content, mode).await
    }

    async fn upload_private_file_atomic(
        &mut self,
        final_path: &str,
        temporary_path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<(), OrchestrationError> {
        RemoteSftp::upload_private_file_atomic(
            self,
            final_path,
            temporary_path,
            content,
            mode,
        )
        .await
    }
}

fn ssh_client_config() -> client::Config {
    client::Config {
        // Submission artifacts can be tens of MiB. Keepalives detect a dead
        // peer while the persisted submission deadline bounds the complete
        // SFTP transfer and scheduler operation.
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    }
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
        let config = Arc::new(ssh_client_config());
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
        channel.exec(true, command).await.map_err(|error| {
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

    /// Upload text as an owner-only SFTP artifact.
    ///
    /// Retained for API compatibility; all artifact writes now use the same
    /// durable, atomic SFTP path as `upload_text_atomic`.
    pub async fn upload_text(
        &mut self,
        remote_path: &str,
        content: &str,
    ) -> Result<(), OrchestrationError> {
        self.upload_text_atomic(remote_path, content).await
    }

    /// Upload through an exclusive same-directory SFTP temporary file, sync it,
    /// verify its size and mode, and atomically rename it.
    pub async fn upload_text_atomic(
        &mut self,
        remote_path: &str,
        content: &str,
    ) -> Result<(), OrchestrationError> {
        validate_remote_path(remote_path, "submission artifact")?;
        let temporary_path = format!("{remote_path}.tmp-{}", uuid::Uuid::now_v7().simple());
        let mut sftp = self.open_sftp().await?;
        let result = upload_artifact_with(
            &mut sftp,
            remote_path,
            &temporary_path,
            content.as_bytes(),
        )
        .await;
        let close_result = sftp.shutdown().await;
        self.finish_atomic_upload(result, close_result, &temporary_path, remote_path)
            .await
    }

    /// Deliver one execution capability through SFTP. Secret bytes are sent
    /// only as binary channel data; remote commands contain paths and modes,
    /// never credential content.
    pub async fn upload_secret_atomic(
        &mut self,
        remote_directory: &str,
        remote_path: &str,
        content: &[u8],
    ) -> Result<(), OrchestrationError> {
        validate_remote_path(remote_directory, "publisher secret directory")?;
        validate_remote_path(remote_path, "publisher credential file")?;
        let expected_prefix = format!("{}/", remote_directory.trim_end_matches('/'));
        if !remote_path.starts_with(&expected_prefix) {
            return Err(OrchestrationError::Backend(
                "publisher credential file must be inside its private directory".into(),
            ));
        }
        let temporary_path = format!("{remote_path}.tmp-{}", uuid::Uuid::now_v7().simple());
        let mut sftp = self.open_sftp().await?;
        let result = upload_secret_with(
            &mut sftp,
            remote_directory,
            remote_path,
            &temporary_path,
            content,
        )
        .await;
        let close_result = sftp.shutdown().await;
        self.finish_atomic_upload(result, close_result, &temporary_path, remote_path)
            .await
    }

    /// Best-effort SFTP cleanup used only after a definitely rejected outer
    /// submission. An uncertain submission retains the credential for the
    /// possibly running allocation.
    pub async fn remove_file_sftp(
        &mut self,
        remote_path: &str,
    ) -> Result<(), OrchestrationError> {
        self.remove_file_sftp_inner(remote_path, "publisher credential file")
            .await
    }

    async fn open_sftp(&mut self) -> Result<RemoteSftp, OrchestrationError> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| OrchestrationError::Backend(format!("SSH SFTP channel: {error}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| {
                OrchestrationError::Backend(format!("SSH SFTP subsystem: {error}"))
            })?;
        RemoteSftp::connect(channel.into_stream()).await
    }

    async fn remove_file_sftp_inner(
        &mut self,
        remote_path: &str,
        label: &str,
    ) -> Result<(), OrchestrationError> {
        validate_remote_path(remote_path, label)?;
        let mut sftp = self.open_sftp().await?;
        let result = sftp.remove_file_if_present(remote_path).await;
        let close_result = sftp.shutdown().await;
        result.and(close_result)
    }

    async fn finish_atomic_upload(
        &mut self,
        result: Result<(), OrchestrationError>,
        close_result: Result<(), OrchestrationError>,
        temporary_path: &str,
        final_path: &str,
    ) -> Result<(), OrchestrationError> {
        match (result, close_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => {
                // The original subsystem may have died after the server
                // committed the atomic rename. The failed channel cannot
                // prove either pathname is absent, so retry both removals on
                // fresh SFTP channels before reporting a definite
                // pre-submission failure.
                for path in failed_atomic_upload_cleanup_paths(false, temporary_path, final_path) {
                    let _ = self.remove_file_sftp_inner(path, "failed upload artifact").await;
                }
                Err(error)
            }
            (Ok(()), Err(error)) => {
                // A failed subsystem shutdown occurs before sbatch dispatch.
                // The final LSTAT already confirmed the file, so open a new
                // SFTP channel and remove it before reporting failure.
                for path in failed_atomic_upload_cleanup_paths(true, temporary_path, final_path) {
                    let _ = self.remove_file_sftp_inner(path, "failed upload artifact").await;
                }
                Err(error)
            }
        }
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

fn remote_command_transport_error(kind: RemoteCommandKind, detail: String) -> OrchestrationError {
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

async fn upload_artifact_with<U: AtomicSftpUploader + Send>(
    uploader: &mut U,
    final_path: &str,
    temporary_path: &str,
    content: &[u8],
) -> Result<(), OrchestrationError> {
    uploader
        .upload_file_atomic(
            final_path,
            temporary_path,
            content,
            SUBMISSION_ARTIFACT_MODE,
        )
        .await
}

async fn upload_secret_with<U: AtomicSftpUploader + Send>(
    uploader: &mut U,
    remote_directory: &str,
    final_path: &str,
    temporary_path: &str,
    content: &[u8],
) -> Result<(), OrchestrationError> {
    uploader
        .ensure_private_directory(remote_directory, PRIVATE_DIRECTORY_MODE)
        .await?;
    uploader
        .upload_private_file_atomic(
            final_path,
            temporary_path,
            content,
            PRIVATE_FILE_MODE,
        )
        .await
}

fn validate_remote_path(path: &str, label: &str) -> Result<(), OrchestrationError> {
    if !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(OrchestrationError::Backend(format!(
            "{label} must be an absolute path without traversal components"
        )));
    }
    Ok(())
}

fn failed_atomic_upload_cleanup_paths<'a>(
    upload_completed: bool,
    temporary_path: &'a str,
    final_path: &'a str,
) -> Vec<&'a str> {
    if upload_completed {
        vec![final_path]
    } else {
        // A broken stream can hide a committed rename, so neither name can
        // be assumed absent after an incomplete upload.
        vec![temporary_path, final_path]
    }
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
    format!("squeue -h -j {job_ids} -o '{SQUEUE_FORMAT}'")
}

fn sacct_query_command(job_ids: &str) -> String {
    format!("sacct -j {job_ids} --format={SACCT_FORMAT} -P -n")
}

fn is_missing_squeue_job_error(stderr: &str) -> bool {
    let mut lines = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let Some(first) = lines.next() else {
        return false;
    };
    std::iter::once(first).chain(lines).all(|line| {
        line.to_ascii_lowercase()
            .ends_with("invalid job id specified")
    })
}

fn squeue_stdout(command: &str, output: RemoteCommandOutput) -> Result<String, OrchestrationError> {
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
        let squeue_out =
            squeue_stdout(&squeue_cmd, session.run_command_output(&squeue_cmd).await?)?;
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
        command_stdout, failed_atomic_upload_cleanup_paths, is_missing_squeue_job_error,
        known_host_patterns_match, known_hosts_has_target, load_known_host_keys,
        remote_command_transport_error, sacct_query_command, scancel_command,
        squeue_query_command, squeue_stdout, ssh_client_config, upload_artifact_with,
        upload_secret_with, validate_remote_path, validate_slurm_job_id, AtomicSftpUploader,
        RemoteCommandKind, RemoteCommandOutput, SlurmSshPool, SlurmTarget,
    };
    use crate::slurm_sftp::{
        PRIVATE_DIRECTORY_MODE, PRIVATE_FILE_MODE, SUBMISSION_ARTIFACT_MODE,
    };
    use crate::OrchestrationError;
    use std::sync::Arc;

    #[derive(Debug, PartialEq, Eq)]
    enum SftpCall {
        EnsurePrivateDirectory {
            path: String,
            mode: u32,
        },
        UploadArtifact {
            final_path: String,
            temporary_path: String,
            content: Vec<u8>,
            mode: u32,
        },
        UploadPrivate {
            final_path: String,
            temporary_path: String,
            content: Vec<u8>,
            mode: u32,
        },
    }

    #[derive(Default)]
    struct ScriptedSftp {
        calls: Vec<SftpCall>,
    }

    #[async_trait::async_trait]
    impl AtomicSftpUploader for ScriptedSftp {
        async fn ensure_private_directory(
            &mut self,
            path: &str,
            mode: u32,
        ) -> Result<(), OrchestrationError> {
            self.calls.push(SftpCall::EnsurePrivateDirectory {
                path: path.into(),
                mode,
            });
            Ok(())
        }

        async fn upload_file_atomic(
            &mut self,
            final_path: &str,
            temporary_path: &str,
            content: &[u8],
            mode: u32,
        ) -> Result<(), OrchestrationError> {
            self.calls.push(SftpCall::UploadArtifact {
                final_path: final_path.into(),
                temporary_path: temporary_path.into(),
                content: content.into(),
                mode,
            });
            Ok(())
        }

        async fn upload_private_file_atomic(
            &mut self,
            final_path: &str,
            temporary_path: &str,
            content: &[u8],
            mode: u32,
        ) -> Result<(), OrchestrationError> {
            self.calls.push(SftpCall::UploadPrivate {
                final_path: final_path.into(),
                temporary_path: temporary_path.into(),
                content: content.into(),
                mode,
            });
            Ok(())
        }
    }

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

    #[tokio::test]
    async fn submission_artifacts_use_owner_only_atomic_sftp() {
        let mut sftp = ScriptedSftp::default();
        upload_artifact_with(
            &mut sftp,
            "/scratch/session graph.pgt",
            "/scratch/session graph.pgt.tmp-test",
            b"graph\0bytes",
        )
        .await
        .unwrap();
        assert_eq!(
            sftp.calls,
            vec![SftpCall::UploadArtifact {
                final_path: "/scratch/session graph.pgt".into(),
                temporary_path: "/scratch/session graph.pgt.tmp-test".into(),
                content: b"graph\0bytes".to_vec(),
                mode: SUBMISSION_ARTIFACT_MODE,
            }]
        );
    }

    #[tokio::test]
    async fn publisher_secrets_retain_private_directory_and_file_policy() {
        let mut sftp = ScriptedSftp::default();
        upload_secret_with(
            &mut sftp,
            "/session/.beampipe-secrets",
            "/session/.beampipe-secrets/publisher.token",
            "/session/.beampipe-secrets/publisher.token.tmp-test",
            b"opaque-token",
        )
        .await
        .unwrap();
        assert_eq!(
            sftp.calls,
            vec![
                SftpCall::EnsurePrivateDirectory {
                    path: "/session/.beampipe-secrets".into(),
                    mode: PRIVATE_DIRECTORY_MODE,
                },
                SftpCall::UploadPrivate {
                    final_path: "/session/.beampipe-secrets/publisher.token".into(),
                    temporary_path: "/session/.beampipe-secrets/publisher.token.tmp-test".into(),
                    content: b"opaque-token".to_vec(),
                    mode: PRIVATE_FILE_MODE,
                },
            ]
        );
    }

    #[test]
    fn interrupted_atomic_upload_rechecks_both_names_on_fresh_channels() {
        let temporary = "/session/.beampipe-secrets/publisher.token.tmp-test";
        let final_path = "/session/.beampipe-secrets/publisher.token";
        assert_eq!(
            failed_atomic_upload_cleanup_paths(false, temporary, final_path),
            vec![temporary, final_path]
        );
        assert_eq!(
            failed_atomic_upload_cleanup_paths(true, temporary, final_path),
            vec![final_path]
        );
    }

    #[test]
    fn sftp_paths_must_be_absolute_and_traversal_free() {
        assert!(validate_remote_path("/scratch/session graph.pgt", "artifact").is_ok());
        for path in [
            "relative/file",
            "/scratch/../secret",
            "/scratch/./file",
            "/scratch/file\nname",
        ] {
            assert!(
                validate_remote_path(path, "artifact").is_err(),
                "accepted {path:?}"
            );
        }
    }

    #[test]
    fn large_silent_uploads_use_keepalives_not_an_inactivity_disconnect() {
        let config = ssh_client_config();
        assert_eq!(config.inactivity_timeout, None);
        assert_eq!(
            config.keepalive_interval,
            Some(std::time::Duration::from_secs(30))
        );
        assert_eq!(config.keepalive_max, 3);
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
        assert_eq!(
            squeue_query_command("123,456"),
            "squeue -h -j 123,456 -o '%i|%T|%R'"
        );
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
    fn known_hosts_rejects_revoked_markers() {
        let dir = tempfile::tempdir().unwrap();
        let pubkey = generate_public_key(&dir);
        let key = pubkey.split_whitespace().collect::<Vec<_>>();
        let path = dir.path().join("known_hosts");
        std::fs::write(
            &path,
            format!("@revoked login-a.example {} {}\n", key[0], key[1]),
        )
        .unwrap();
        let error = load_known_host_keys(path.to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("@revoked"));
        assert!(error.contains("not supported"));
    }

    #[test]
    fn malformed_known_hosts_entries_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, "login-a.example ssh-ed25519 not-base64\n").unwrap();
        let error = load_known_host_keys(path.to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("parse known_hosts"));
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

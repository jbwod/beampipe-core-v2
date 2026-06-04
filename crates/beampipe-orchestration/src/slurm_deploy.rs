use crate::OrchestrationError;
use beampipe_profiles::{DaliugeAlgo, SlurmRemoteDeploymentConfig};
use serde_json::Value;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const JOBSUB_CREATED_RE: &str = "Created job submission script";

pub struct SlurmSubmitParams {
    pub execution_id: String,
    pub session_id: String,
    pub pgt_json: Value,
    pub deployment: SlurmRemoteDeploymentConfig,
    pub username: String,
}

pub struct SlurmSubmitResult {
    pub slurm_job_id: String,
    pub session_dir: String,
    pub composite_scheduler_job_id: String,
}

pub fn render_generated_ini(
    deployment: &SlurmRemoteDeploymentConfig,
    username: &str,
    pgt_remote_path: &str,
    dlg_root: &str,
) -> String {
    let mut lines = vec![
        "[DEPLOYMENT]".into(),
        "remote = False".into(),
        "submit = False".into(),
        "[ENGINE]".into(),
        format!("NUM_NODES = {}", deployment.num_nodes),
        format!("NUM_ISLANDS = {}", deployment.num_islands),
        format!("MAX_THREADS = {}", deployment.max_threads),
        format!("VERBOSE_LEVEL = {}", deployment.verbose_level),
        "[GRAPH]".into(),
        format!("PHYSICAL_GRAPH = {pgt_remote_path}"),
        "[FACILITY]".into(),
        format!("USER = {username}"),
        format!("ACCOUNT = {}", deployment.account),
        format!("LOGIN_NODE = {}", deployment.login_node),
        format!("HOME_DIR = {}", deployment.home_dir),
        format!("DLG_ROOT = {dlg_root}"),
        format!("LOG_DIR = {}", deployment.log_dir),
        format!("EXEC_PREFIX = {}", deployment.exec_prefix),
    ];
    if let Some(modules) = deployment.modules.as_deref() {
        lines.push(format!("MODULES = {modules}"));
    }
    if let Some(venv) = deployment.venv.as_deref() {
        lines.push(format!("VENV = {venv}"));
    }
    if deployment.all_nics {
        lines.push("[ENGINE]".into());
        lines.push("ALL_NICS = True".into());
    }
    lines.join("\n")
}

pub fn env_prelude(deployment: &SlurmRemoteDeploymentConfig) -> String {
    let mut parts = vec!["set -euo pipefail".to_string()];
    if let Some(modules) = deployment.modules.as_deref() {
        parts.push("set +u".into());
        for line in modules.lines().map(str::trim).filter(|l| !l.is_empty()) {
            parts.push(line.to_string());
        }
        parts.push("set -u".into());
    }
    if let Some(venv) = deployment.venv.as_deref() {
        parts.push("set +u".into());
        parts.push(venv.trim().to_string());
        parts.push("set -u".into());
    }
    parts.join("\n")
}

pub fn create_dlg_job_argv(
    deployment: &SlurmRemoteDeploymentConfig,
    pgt_remote_path: &str,
    config_file_remote_path: &str,
    slurm_template_remote_path: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![
        "python3".into(),
        "-m".into(),
        "dlg.deploy.create_dlg_job".into(),
        "--action".into(),
        "submit".into(),
        "-f".into(),
        deployment.facility.clone(),
        "-P".into(),
        pgt_remote_path.to_string(),
        "--config_file".into(),
        config_file_remote_path.to_string(),
    ];
    if let Some(template) = slurm_template_remote_path {
        argv.push("--slurm_template".into());
        argv.push(template.to_string());
    }
    argv
}

pub fn parse_jobsub_path(stdout: &str) -> Result<String, OrchestrationError> {
    for line in stdout.lines() {
        if line.contains(JOBSUB_CREATED_RE) {
            if let Some(path) = line.split_whitespace().last() {
                return Ok(path.to_string());
            }
        }
    }
    Err(OrchestrationError::Backend(format!(
        "create_dlg_job did not print job submission script path; stdout={stdout:?}"
    )))
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-:".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub async fn submit_slurm_session(
    params: SlurmSubmitParams,
) -> Result<SlurmSubmitResult, OrchestrationError> {
    let SlurmSubmitParams {
        execution_id,
        session_id,
        mut pgt_json,
        deployment,
        username,
    } = params;
    let dlg_root = deployment.dlg_root.trim_end_matches('/').to_string();
    let staging_dir = format!("{dlg_root}/staging");
    let pgt_remote_path = format!("{staging_dir}/BeampipeExecution_{execution_id}.pgt.graph");
    let config_file_remote_path = format!("{staging_dir}/BeampipeExecution_{execution_id}.ini");
    let slurm_template_remote_path = deployment
        .slurm_template
        .as_ref()
        .filter(|t| !t.trim().is_empty())
        .map(|_| format!("{staging_dir}/BeampipeExecution_{execution_id}.slurm"));

    if let Value::Array(ref mut arr) = pgt_json {
        if !arr.is_empty() {
            arr[0] = Value::String(format!("{session_id}.pgt.graph"));
        }
    }

    let target = ssh_target(&deployment, &username);
    run_ssh(
        &target,
        deployment.ssh_port,
        &format!("mkdir -p {staging_dir}"),
    )
    .await?;
    put_text_via_ssh(
        &target,
        deployment.ssh_port,
        &pgt_remote_path,
        &serde_json::to_string(&pgt_json)
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?,
    )
    .await?;
    put_text_via_ssh(
        &target,
        deployment.ssh_port,
        &config_file_remote_path,
        &render_generated_ini(&deployment, &username, &pgt_remote_path, &dlg_root),
    )
    .await?;
    if let (Some(template_body), Some(template_path)) = (
        deployment.slurm_template.as_deref(),
        slurm_template_remote_path.as_deref(),
    ) {
        put_text_via_ssh(&target, deployment.ssh_port, template_path, template_body).await?;
    }

    let argv = create_dlg_job_argv(
        &deployment,
        &pgt_remote_path,
        &config_file_remote_path,
        slurm_template_remote_path.as_deref(),
    );
    let inner = format!(
        "{}\nexport DLG_ROOT={}\n{}",
        env_prelude(&deployment),
        shell_quote(&dlg_root),
        argv.iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let create_out = run_ssh(
        &target,
        deployment.ssh_port,
        &format!("bash -lc {}", shell_quote(&inner)),
    )
    .await?;
    let jobsub_path = parse_jobsub_path(&create_out)?;
    let sbatch_out = run_ssh(
        &target,
        deployment.ssh_port,
        &format!("sbatch --parsable {}", shell_quote(&jobsub_path)),
    )
    .await?;
    let slurm_job_id = sbatch_out
        .split(';')
        .next()
        .unwrap_or(&sbatch_out)
        .trim()
        .to_string();
    let session_dir = jobsub_path
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_else(|| format!("{dlg_root}/sessions/{session_id}"));
    let composite = beampipe_domain::slurm::compose_scheduler_job_id(
        &session_id,
        &slurm_job_id,
        Some(&session_dir),
    )
    .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
    Ok(SlurmSubmitResult {
        slurm_job_id,
        session_dir,
        composite_scheduler_job_id: composite,
    })
}

/// Preflight SSH to the Slurm login node before CASDA staging / TM translate.
pub async fn probe_slurm_login(
    deployment: &SlurmRemoteDeploymentConfig,
    username: &str,
) -> Result<(), String> {
    let target = ssh_target(deployment, username);
    run_ssh(&target, deployment.ssh_port, "echo ok")
        .await
        .map(|_| ())
        .map_err(|e| {
            format!(
                "Slurm login node {} ({target}) unreachable: {e}. Check VPN/SSH before submit.",
                deployment.login_node
            )
        })
}

fn ssh_target(deployment: &SlurmRemoteDeploymentConfig, username: &str) -> String {
    format!("{}@{}", username, deployment.login_node)
}

/// Extra `ssh` CLI args from `SLURM_SSH_PRIVATE_KEY_FILE` and `SLURM_SSH_KNOWN_HOSTS_SOURCE` (Python parity).
pub fn ssh_option_args() -> Vec<String> {
    let mut args = Vec::new();
    if let Ok(key) = std::env::var("SLURM_SSH_PRIVATE_KEY_FILE") {
        if !key.trim().is_empty() {
            args.push("-i".into());
            args.push(key);
        }
    }
    if let Ok(hosts) = std::env::var("SLURM_SSH_KNOWN_HOSTS_SOURCE") {
        if !hosts.trim().is_empty() {
            args.push("-o".into());
            args.push(format!("UserKnownHostsFile={hosts}"));
        }
    }
    args
}

fn append_ssh_options(cmd: &mut Command) {
    for arg in ssh_option_args() {
        cmd.arg(arg);
    }
}

async fn run_ssh(target: &str, port: i32, remote_cmd: &str) -> Result<String, OrchestrationError> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-p")
        .arg(port.to_string())
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new");
    append_ssh_options(&mut cmd);
    let output = cmd
        .arg(target)
        .arg(remote_cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| OrchestrationError::Backend(format!("ssh spawn failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(OrchestrationError::Backend(format!("ssh failed: {stderr}")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn put_text_via_ssh(
    target: &str,
    port: i32,
    remote_path: &str,
    content: &str,
) -> Result<(), OrchestrationError> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-p")
        .arg(port.to_string())
        .arg("-oBatchMode=yes")
        .arg("-oStrictHostKeyChecking=accept-new");
    append_ssh_options(&mut cmd);
    let mut child = cmd
        .arg(target)
        .arg(format!("tee {remote_path}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| OrchestrationError::Backend(format!("ssh tee spawn failed: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| OrchestrationError::Backend("ssh stdin unavailable".into()))?;
    stdin
        .write_all(content.as_bytes())
        .await
        .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(OrchestrationError::Backend(format!(
            "ssh tee failed: {stderr}"
        )));
    }
    Ok(())
}

pub fn resolve_remote_user(deployment: &SlurmRemoteDeploymentConfig) -> String {
    deployment
        .remote_user
        .clone()
        .or_else(|| std::env::var("SLURM_REMOTE_USER").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "root".into())
}

pub fn algo_str(algo: &DaliugeAlgo) -> &'static str {
    match algo {
        DaliugeAlgo::Metis => "metis",
        DaliugeAlgo::Mysarkar => "mysarkar",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jobsub_extracts_path() {
        let stdout = "Created job submission script /home/user/sessions/x/jobsub.sh\n";
        assert!(parse_jobsub_path(stdout).unwrap().ends_with("jobsub.sh"));
    }

    #[test]
    fn render_ini_contains_account() {
        let dep = SlurmRemoteDeploymentConfig {
            login_node: "login".into(),
            ssh_port: 22,
            remote_user: None,
            account: "myacct".into(),
            home_dir: "/home".into(),
            log_dir: "/log".into(),
            exec_prefix: "srun".into(),
            dlg_root: "/dlg".into(),
            venv: None,
            modules: None,
            facility: "setonix".into(),
            job_duration_minutes: 30,
            num_nodes: 1,
            num_islands: 1,
            verbose_level: 1,
            max_threads: 0,
            all_nics: false,
            zerorun: false,
            sleepncopy: false,
            check_with_session: false,
            verify_ssl: None,
            slurm_template: None,
        };
        let ini = render_generated_ini(&dep, "user", "/path.pgt", "/dlg");
        assert!(ini.contains("ACCOUNT = myacct"));
    }
}

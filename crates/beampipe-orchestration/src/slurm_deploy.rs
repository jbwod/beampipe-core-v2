use crate::scheduler::SchedulerResourceRequest;
use crate::slurm_ssh::{SlurmSshSession, SlurmTarget};
use crate::OrchestrationError;
use beampipe_profiles::{DaliugeAlgo, SlurmRemoteDeploymentConfig};
use serde_json::Value;

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
        format!("NUM_NODES = {}", deployment.effective_nodes()),
        format!("NUM_ISLANDS = {}", deployment.effective_islands()),
        format!("JOB_DUR = {}", deployment.effective_wall_time_minutes()),
        format!("MAX_THREADS = {}", deployment.max_threads),
        format!("VERBOSE_LEVEL = {}", deployment.verbose_level),
        format!(
            "ALL_NICS = {}",
            if deployment.all_nics { "True" } else { "False" }
        ),
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
    push_ini_value(&mut lines, "MODULES", deployment.modules.as_deref());
    push_ini_value(&mut lines, "VENV", deployment.venv.as_deref());
    lines.join("\n")
}

fn push_ini_value(lines: &mut Vec<String>, key: &str, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    let mut value_lines = value.lines();
    let first = value_lines.next().unwrap_or_default();
    lines.push(format!("{key} = {first}"));
    lines.extend(value_lines.map(|line| format!("    {line}")));
}

const SLURM_ACCOUNT_ENV: &str = "BEAMPIPE_SLURM_ACCOUNT";
const OPTIONAL_FORWARDED_ENVIRONMENT: [&str; 1] = ["BEAMPIPE_ASKAPSOFT_SIF"];

pub fn env_prelude(deployment: &SlurmRemoteDeploymentConfig) -> Result<String, OrchestrationError> {
    env_prelude_with(deployment, |name| std::env::var(name).ok())
}

fn env_prelude_with<F>(
    deployment: &SlurmRemoteDeploymentConfig,
    mut read_environment: F,
) -> Result<String, OrchestrationError>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut parts = vec![
        "set -euo pipefail".to_string(),
        format!(
            "export {SLURM_ACCOUNT_ENV}={}",
            shell_quote(&deployment.account)
        ),
    ];
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
    if let Some(setup) = deployment.environment_setup.as_deref() {
        for name in OPTIONAL_FORWARDED_ENVIRONMENT {
            if setup.contains(name) {
                let value = read_environment(name)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        OrchestrationError::Backend(format!(
                            "deployment.environment_setup requires non-empty {name}"
                        ))
                    })?;
                parts.push(format!("export {name}={}", shell_quote(&value)));
            }
        }
        for line in setup.lines().map(str::trim).filter(|line| !line.is_empty()) {
            parts.push(line.to_string());
        }
    }
    // The profile account is authoritative for both the outer DALiuGE
    // allocation and nested Slurm jobs. Re-assert it after operator setup so
    // an ambient process variable or setup command cannot make them drift.
    parts.push(format!(
        "export {SLURM_ACCOUNT_ENV}={}",
        shell_quote(&deployment.account)
    ));
    Ok(parts.join("\n"))
}

fn sbatch_command_with<F>(
    deployment: &SlurmRemoteDeploymentConfig,
    session_id: &str,
    jobsub_path: &str,
    read_environment: F,
) -> Result<String, OrchestrationError>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut exported = vec![SLURM_ACCOUNT_ENV];
    if deployment
        .environment_setup
        .as_deref()
        .is_some_and(|setup| setup.contains("BEAMPIPE_ASKAPSOFT_SIF"))
    {
        exported.push("BEAMPIPE_ASKAPSOFT_SIF");
    }
    let resources = SchedulerResourceRequest::from_slurm_profile(deployment);
    let mut argv = vec![
        "sbatch".to_string(),
        format!("--export={}", exported.join(",")),
        "--parsable".to_string(),
        format!("--job-name={session_id}"),
        format!("--account={}", resources.account),
        format!("--nodes={}", resources.nodes),
        format!(
            "--time={:02}:{:02}:00",
            resources.wall_time_minutes / 60,
            resources.wall_time_minutes % 60
        ),
    ];
    for (flag, value) in [
        ("partition", resources.partition.as_deref()),
        ("mem", resources.memory.as_deref()),
        ("constraint", resources.constraint.as_deref()),
        ("qos", resources.quality_of_service.as_deref()),
    ] {
        if let Some(value) = value {
            argv.push(format!("--{flag}={value}"));
        }
    }
    if let Some(tasks) = resources.tasks {
        argv.push(format!("--ntasks={tasks}"));
    }
    if let Some(cpus) = resources.cpus_per_task {
        argv.push(format!("--cpus-per-task={cpus}"));
    }
    argv.push(jobsub_path.to_string());
    let inner = format!(
        "{}\n{}",
        env_prelude_with(deployment, read_environment)?,
        argv.iter()
            .map(|argument| shell_quote(argument))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(format!("bash -lc {}", shell_quote(&inner)))
}

fn sbatch_command(
    deployment: &SlurmRemoteDeploymentConfig,
    session_id: &str,
    jobsub_path: &str,
) -> Result<String, OrchestrationError> {
    sbatch_command_with(deployment, session_id, jobsub_path, |name| {
        std::env::var(name).ok()
    })
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

    let target = SlurmTarget::from_deployment(&deployment, &username);
    let mut session = SlurmSshSession::connect(&target).await?;

    session
        .run_command(&format!("mkdir -p {staging_dir}"))
        .await?;
    session
        .upload_text_atomic(
            &pgt_remote_path,
            &serde_json::to_string(&pgt_json)
                .map_err(|e| OrchestrationError::Backend(e.to_string()))?,
        )
        .await?;
    session
        .upload_text_atomic(
            &config_file_remote_path,
            &render_generated_ini(&deployment, &username, &pgt_remote_path, &dlg_root),
        )
        .await?;
    if let (Some(template_body), Some(template_path)) = (
        deployment.slurm_template.as_deref(),
        slurm_template_remote_path.as_deref(),
    ) {
        session
            .upload_text_atomic(template_path, template_body)
            .await?;
    }

    let argv = create_dlg_job_argv(
        &deployment,
        &pgt_remote_path,
        &config_file_remote_path,
        slurm_template_remote_path.as_deref(),
    );
    let inner = format!(
        "{}\nexport DLG_ROOT={}\n{}",
        env_prelude(&deployment)?,
        shell_quote(&dlg_root),
        argv.iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let create_out = session
        .run_command(&format!("bash -lc {}", shell_quote(&inner)))
        .await?;
    let jobsub_path = parse_jobsub_path(&create_out)?;
    let sbatch_out = session
        .run_command(&sbatch_command(&deployment, &session_id, &jobsub_path)?)
        .await?;
    let _ = session.close().await;

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
    let target = SlurmTarget::from_deployment(deployment, username);
    let mut session = SlurmSshSession::connect(&target).await.map_err(|e| {
        format!(
            "Slurm login node {} ({}@{}) unreachable: {e}. Check VPN/SSH before submit.",
            deployment.login_node, username, deployment.login_node
        )
    })?;
    session.run_command("echo ok").await.map_err(|e| {
        format!(
            "Slurm login node {} ({}@{}) unreachable: {e}. Check VPN/SSH before submit.",
            deployment.login_node, username, deployment.login_node
        )
    })?;
    let _ = session.close().await;
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
        let mut dep = deployment();
        dep.resources.nodes = Some(3);
        dep.resources.wall_time_minutes = Some(125);
        dep.manager_topology.islands = Some(2);
        dep.all_nics = true;
        dep.modules = Some("module load singularity\nmodule load python".into());
        let ini = render_generated_ini(&dep, "user", "/path.pgt", "/dlg");
        assert!(ini.contains("ACCOUNT = myacct"));
        assert!(ini.contains("NUM_NODES = 3"));
        assert!(ini.contains("NUM_ISLANDS = 2"));
        assert!(ini.contains("JOB_DUR = 125"));
        assert_eq!(ini.matches("[ENGINE]").count(), 1);
        assert!(ini.contains("ALL_NICS = True"));
        assert!(ini.contains("MODULES = module load singularity\n    module load python"));
    }

    #[test]
    fn environment_setup_forwards_required_process_values_safely() {
        let mut dep = deployment();
        dep.environment_setup = Some(
            "export BEAMPIPE_SLURM_ACCOUNT=\"$BEAMPIPE_SLURM_ACCOUNT\"\n\
             export BEAMPIPE_ASKAPSOFT_SIF=\"$BEAMPIPE_ASKAPSOFT_SIF\""
                .into(),
        );
        let prelude = env_prelude_with(&dep, |name| match name {
            "BEAMPIPE_ASKAPSOFT_SIF" => Some("/images/askap soft's.sif".into()),
            _ => None,
        })
        .unwrap();
        assert!(prelude.contains("export BEAMPIPE_SLURM_ACCOUNT=myacct"));
        assert!(!prelude.contains("science-account"));
        assert!(prelude.contains("export BEAMPIPE_ASKAPSOFT_SIF='/images/askap soft'\\''s.sif'"));
        assert!(prelude.contains("export BEAMPIPE_SLURM_ACCOUNT=\"$BEAMPIPE_SLURM_ACCOUNT\""));
        assert!(prelude.ends_with("export BEAMPIPE_SLURM_ACCOUNT=myacct"));
    }

    #[test]
    fn environment_setup_rejects_missing_forwarded_values() {
        let mut dep = deployment();
        dep.environment_setup = Some("echo $BEAMPIPE_ASKAPSOFT_SIF".into());
        let error = env_prelude_with(&dep, |_| None).unwrap_err();
        assert!(error.to_string().contains("BEAMPIPE_ASKAPSOFT_SIF"));
    }

    #[test]
    fn sbatch_runs_with_exports_in_the_same_remote_shell() {
        let mut dep = deployment();
        dep.environment_setup = Some("test -n \"$BEAMPIPE_ASKAPSOFT_SIF\"".into());
        dep.resources.partition = Some("work".into());
        dep.resources.nodes = Some(2);
        dep.resources.tasks = Some(2);
        dep.resources.cpus_per_task = Some(4);
        dep.resources.memory = Some("12G".into());
        dep.resources.wall_time_minutes = Some(50);
        dep.resources.constraint = Some("cpu".into());
        dep.resources.quality_of_service = Some("normal".into());
        let command = sbatch_command_with(&dep, "session id", "/dlg/job sub.sh", |name| {
            (name == "BEAMPIPE_ASKAPSOFT_SIF").then(|| "/images/askap soft.sif".into())
        })
        .unwrap();

        assert!(command.starts_with("bash -lc "));
        assert!(command.contains("export BEAMPIPE_SLURM_ACCOUNT=myacct"));
        assert!(command.contains("export BEAMPIPE_ASKAPSOFT_SIF="));
        for expected in [
            "--export=BEAMPIPE_SLURM_ACCOUNT,BEAMPIPE_ASKAPSOFT_SIF",
            "--parsable",
            "--job-name=session id",
            "--account=myacct",
            "--partition=work",
            "--nodes=2",
            "--ntasks=2",
            "--cpus-per-task=4",
            "--mem=12G",
            "--time=00:50:00",
            "--constraint=cpu",
            "--qos=normal",
            "/dlg/job sub.sh",
        ] {
            assert!(
                command.contains(expected),
                "missing {expected:?} in {command}"
            );
        }
        assert!(
            command.find("export BEAMPIPE_ASKAPSOFT_SIF").unwrap()
                < command.find("sbatch").unwrap()
        );
    }

    fn deployment() -> SlurmRemoteDeploymentConfig {
        SlurmRemoteDeploymentConfig {
            login_node: "login".into(),
            ssh_port: 22,
            remote_user: None,
            ssh_credential: None,
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
            resources: Default::default(),
            manager_topology: Default::default(),
            container_runtime: None,
            environment_setup: None,
        }
    }
}

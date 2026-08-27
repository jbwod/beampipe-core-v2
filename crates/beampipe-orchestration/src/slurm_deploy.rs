use crate::scheduler::SchedulerResourceRequest;
use crate::slurm_ssh::{SlurmSshSession, SlurmTarget};
use crate::OrchestrationError;
use beampipe_profiles::{
    DaliugeAlgo, PublicationRuntimeConfig, SlurmRemoteDeploymentConfig,
    SlurmRuntimeContractConfig, SlurmRuntimeEnvironmentKind,
};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use url::Url;
use zeroize::Zeroizing;

const JOBSUB_CREATED_RE: &str = "Created job submission script";
const PYTHON_PATH_ENV: &str = "PYTHONPATH";
const OUTER_TERMINATION_NOTICE_SECONDS: i32 = 120;
const PUBLISHER_COMMAND: &str = "beampipe-publish";
const BEAMPIPE_APPS_PYTHON_MODULE: &str = "beampipe_pallette.apps";
const INGEST_APP_CLASS: &str = "BeampipeIngestApp";
const PUBLISHER_APP_CLASS: &str = "BeampipePublishApp";
const PUBLISHER_EXECUTION_ID_ENV: &str = "BEAMPIPE_EXECUTION_ID";
const PUBLISHER_EXECUTION_ATTEMPT_ENV: &str = "BEAMPIPE_EXECUTION_ATTEMPT";
const PUBLISHER_CORE_URL_ENV: &str = "BEAMPIPE_CORE_URL";
const PUBLISHER_DESTINATION_URI_ENV: &str = "BEAMPIPE_OUTPUT_DESTINATION_URI";
const PUBLISHER_OUTPUT_ROOT_ENV: &str = "BEAMPIPE_OUTPUT_ROOT";
const PUBLISHER_TOKEN_FILE_ENV: &str = "BEAMPIPE_PUBLISHER_TOKEN_FILE";
const PUBLISHER_SECRETS_DIRECTORY: &str = ".beampipe-secrets";
const PUBLISHER_TOKEN_FILENAME: &str = "publisher.token";
const DALIUGE_FAILED_SESSION_SITECUSTOMIZE: &str = r#"# Beampipe compatibility shim for DALiuGE deploy.common.
from dlg.deploy import common as _beampipe_common
from dlg.manager.session import SessionStates as _beampipe_session_states

_beampipe_original_is_end_state = _beampipe_common._is_end_state


def _beampipe_is_end_state(state):
    return (
        state == _beampipe_session_states.FAILED
        or _beampipe_original_is_end_state(state)
    )


_beampipe_common._is_end_state = _beampipe_is_end_state
"#;

pub struct SlurmSubmitParams {
    pub execution_id: String,
    pub session_id: String,
    pub pgt_json: Value,
    pub deployment: SlurmRemoteDeploymentConfig,
    pub username: String,
    pub publisher_credential: Option<PublisherRuntimeCredential>,
}

struct PublisherToken(Zeroizing<Vec<u8>>);

/// Execution-scoped capability delivered outside every persisted graph and
/// manifest. Debug output is intentionally redacted and clones share one
/// zeroizing allocation rather than duplicating plaintext bytes.
#[derive(Clone)]
pub struct PublisherRuntimeCredential {
    execution_attempt: i32,
    token: Arc<PublisherToken>,
}

impl std::fmt::Debug for PublisherRuntimeCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublisherRuntimeCredential")
            .field("execution_attempt", &self.execution_attempt)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

impl PublisherRuntimeCredential {
    pub fn new(
        execution_attempt: i32,
        token: impl Into<Vec<u8>>,
    ) -> Result<Self, OrchestrationError> {
        if execution_attempt < 0 {
            return Err(OrchestrationError::Backend(
                "publisher execution attempt must be non-negative".into(),
            ));
        }
        let token = Zeroizing::new(token.into());
        if token.len() < 32
            || token.len() > 1024
            || !token
                .iter()
                .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
        {
            return Err(OrchestrationError::Backend(
                "publisher credential is not a valid opaque token".into(),
            ));
        }
        Ok(Self {
            execution_attempt,
            token: Arc::new(PublisherToken(token)),
        })
    }

    pub fn execution_attempt(&self) -> i32 {
        self.execution_attempt
    }

    fn token_bytes(&self) -> &[u8] {
        self.token.0.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPublisherRuntime {
    execution_id: String,
    execution_attempt: i32,
    core_url: String,
    durable_destination_uri: String,
    token_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlurmSubmitResult {
    pub slurm_job_id: String,
    pub session_dir: String,
    pub staging_root: Option<String>,
    pub composite_scheduler_job_id: String,
}

/// DALiuGE uses the first PGT array item as the physical-graph filename. Bind it
/// before Core hashes the immutable artifact and repeat this at the transport
/// boundary so the stored evidence and uploaded payload cannot drift.
pub fn bind_physical_graph_to_session(pgt_json: &mut Value, session_id: &str) {
    if let Value::Array(items) = pgt_json {
        if let Some(graph_name) = items.first_mut() {
            *graph_name = Value::String(format!("{session_id}.pgt.graph"));
        }
    }
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
    deployment
        .runtime_contract
        .validate()
        .map_err(|error| OrchestrationError::Backend(error.to_string()))?;
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
    for requirement in &deployment.runtime_contract.required_environment {
        let value = read_environment(&requirement.name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                OrchestrationError::Backend(format!(
                    "deployment.runtime_contract requires non-empty {}",
                    requirement.name
                ))
            })?;
        parts.push(format!(
            "export {}={}",
            requirement.name,
            shell_quote(&value)
        ));
    }
    if let Some(setup) = deployment.environment_setup.as_deref() {
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

fn slurm_preflight_script_with<F>(
    deployment: &SlurmRemoteDeploymentConfig,
    publication_required: bool,
    mut read_environment: F,
) -> Result<String, OrchestrationError>
where
    F: FnMut(&str) -> Option<String>,
{
    if publication_required {
        let publication = deployment.publication.as_ref().ok_or_else(|| {
            OrchestrationError::Backend(
                "required output publication has no pinned deployment publication contract"
                    .into(),
            )
        })?;
        resolve_publication_inputs_with(publication, |name| read_environment(name))?;
    }
    let mut lines = vec![env_prelude_with(deployment, |name| {
        read_environment(name)
    })?];
    let mut commands = vec![
        "sbatch", "squeue", "sacct", "scancel", "scontrol", "srun", "python3",
    ];
    if publication_required {
        commands.push(PUBLISHER_COMMAND);
    }
    for command in &deployment.runtime_contract.required_commands {
        if !commands.contains(&command.as_str()) {
            commands.push(command);
        }
    }
    for command in commands {
        lines.push(format!(
            "command -v {} >/dev/null 2>&1 || {{ echo {} >&2; exit 127; }}",
            shell_quote(command),
            shell_quote(&format!("missing required command: {command}")),
        ));
    }
    lines.push(format!(
        "test -d {root} && test -w {root} || {{ echo 'DLG_ROOT is not a writable directory' >&2; exit 73; }}",
        root = shell_quote(&deployment.dlg_root)
    ));
    let mut python_modules = vec!["dlg.deploy.create_dlg_job"];
    if publication_required {
        python_modules.push(BEAMPIPE_APPS_PYTHON_MODULE);
    }
    for module in &deployment.runtime_contract.required_python_modules {
        if !python_modules.contains(&module.as_str()) {
            python_modules.push(module);
        }
    }
    let module_list = serde_json::to_string(&python_modules)
        .map_err(|error| OrchestrationError::Backend(error.to_string()))?;
    let mut import_script =
        format!("import importlib; [importlib.import_module(name) for name in {module_list}]");
    let ingest_required = deployment
        .runtime_contract
        .required_python_modules
        .iter()
        .any(|module| module == BEAMPIPE_APPS_PYTHON_MODULE);
    if ingest_required {
        import_script.push_str(&format!(
            "; getattr(importlib.import_module({module}), {class_name})",
            module = serde_json::to_string(BEAMPIPE_APPS_PYTHON_MODULE)
                .map_err(|error| OrchestrationError::Backend(error.to_string()))?,
            class_name = serde_json::to_string(INGEST_APP_CLASS)
                .map_err(|error| OrchestrationError::Backend(error.to_string()))?,
        ));
    }
    if publication_required {
        import_script.push_str(&format!(
            "; getattr(importlib.import_module({module}), {class_name})",
            module = serde_json::to_string(BEAMPIPE_APPS_PYTHON_MODULE)
                .map_err(|error| OrchestrationError::Backend(error.to_string()))?,
            class_name = serde_json::to_string(PUBLISHER_APP_CLASS)
                .map_err(|error| OrchestrationError::Backend(error.to_string()))?,
        ));
    }
    lines.push(format!("python3 -c {}", shell_quote(&import_script)));
    for requirement in &deployment.runtime_contract.required_environment {
        if matches!(requirement.kind, SlurmRuntimeEnvironmentKind::ReadableFile) {
            lines.push(format!(
                "test -f \"${{{name}}}\" && test -r \"${{{name}}}\" || {{ echo {message} >&2; exit 66; }}",
                name = requirement.name,
                message = shell_quote(&format!(
                    "{} is not a readable regular file",
                    requirement.name
                )),
            ));
        }
    }
    Ok(lines.join("\n"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPublicationInputs {
    core_url: String,
    durable_destination_uri: String,
}

fn resolve_publication_inputs_with<F>(
    publication: &PublicationRuntimeConfig,
    mut read_environment: F,
) -> Result<ResolvedPublicationInputs, OrchestrationError>
where
    F: FnMut(&str) -> Option<String>,
{
    publication
        .validate()
        .map_err(|error| OrchestrationError::Backend(error.to_string()))?;
    let core_url = read_environment(&publication.core_url_environment)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            OrchestrationError::Backend(format!(
                "deployment publication requires non-empty {}",
                publication.core_url_environment
            ))
        })?;
    let durable_destination_uri =
        read_environment(&publication.durable_destination_uri_environment)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                OrchestrationError::Backend(format!(
                    "deployment publication requires non-empty {}",
                    publication.durable_destination_uri_environment
                ))
            })?;
    let core_url = validated_core_url(&core_url, publication.allow_insecure_core_http)?;
    let durable_destination_uri = validated_destination_uri(&durable_destination_uri)?;
    Ok(ResolvedPublicationInputs {
        core_url,
        durable_destination_uri,
    })
}

fn validated_core_url(
    raw: &str,
    allow_insecure_core_http: bool,
) -> Result<String, OrchestrationError> {
    let mut url = Url::parse(raw).map_err(|_| {
        OrchestrationError::Backend(
            "publisher Core URL must be an absolute HTTPS URL".into(),
        )
    })?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(OrchestrationError::Backend(
            "publisher Core URL must not contain credentials, a query, or a fragment".into(),
        ));
    }
    match url.scheme() {
        "https" => {}
        "http"
            if allow_insecure_core_http
                && !crate::slurm_credentials::is_production_env()
                && url.host_str().is_some_and(is_loopback_host) => {}
        _ => {
            return Err(OrchestrationError::Backend(
                "publisher Core URL must use HTTPS; loopback HTTP requires the profile development override"
                    .into(),
            ));
        }
    }
    if url.path() == "/" {
        url.set_path("");
    } else {
        let normalized = url.path().trim_end_matches('/').to_string();
        url.set_path(&normalized);
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn validated_destination_uri(raw: &str) -> Result<String, OrchestrationError> {
    let mut url = Url::parse(raw).map_err(|_| {
        OrchestrationError::Backend(
            "publisher destination must be an absolute file or s3 URI".into(),
        )
    })?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(OrchestrationError::Backend(
            "publisher destination must not contain credentials, a query, or a fragment".into(),
        ));
    }
    match url.scheme() {
        "s3" if url.host_str().is_some_and(|bucket| !bucket.is_empty()) => {}
        "file"
            if url
                .host_str()
                .is_none_or(|host| host.is_empty() || is_loopback_host(host))
                && url.path().starts_with('/') => {}
        _ => {
            return Err(OrchestrationError::Backend(
                "publisher destination must use s3://bucket/... or an absolute file:///... URI"
                    .into(),
            ));
        }
    }
    let normalized = url.path().trim_end_matches('/').to_string();
    if normalized.is_empty() && url.scheme() != "s3" {
        return Err(OrchestrationError::Backend(
            "publisher file destination must name a dedicated directory, not filesystem root"
                .into(),
        ));
    }
    url.set_path(&normalized);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn slurm_preflight_script(
    deployment: &SlurmRemoteDeploymentConfig,
    publication_required: bool,
) -> Result<String, OrchestrationError> {
    slurm_preflight_script_with(deployment, publication_required, |name| {
        std::env::var(name).ok()
    })
}

fn sbatch_command_with<F>(
    deployment: &SlurmRemoteDeploymentConfig,
    session_id: &str,
    jobsub_path: &str,
    staging_root: &str,
    cache_root: &str,
    python_path: &str,
    publisher: Option<&ResolvedPublisherRuntime>,
    read_environment: F,
) -> Result<String, OrchestrationError>
where
    F: FnMut(&str) -> Option<String>,
{
    deployment
        .runtime_contract
        .validate()
        .map_err(|error| OrchestrationError::Backend(error.to_string()))?;
    let staging_root = normalized_remote_absolute_path(staging_root, "run output root")?;
    let staging_root = staging_root.to_string_lossy();
    let cache_root = normalized_remote_absolute_path(cache_root, "shared staging root")?;
    let cache_root = cache_root.to_string_lossy();
    let contract = &deployment.runtime_contract;
    let mut exported = vec![SLURM_ACCOUNT_ENV.to_string()];
    for name in [
        contract.output_environment_variable.as_ref(),
        contract.shared_staging_environment_variable.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        exported.push(name.clone());
    }
    exported.push(PYTHON_PATH_ENV.to_string());
    if publisher.is_some() {
        exported.extend(
            [
                PUBLISHER_EXECUTION_ID_ENV,
                PUBLISHER_EXECUTION_ATTEMPT_ENV,
                PUBLISHER_CORE_URL_ENV,
                PUBLISHER_DESTINATION_URI_ENV,
                PUBLISHER_OUTPUT_ROOT_ENV,
                PUBLISHER_TOKEN_FILE_ENV,
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    for requirement in &contract.required_environment {
        exported.push(requirement.name.clone());
    }
    let mut seen = HashSet::new();
    exported.retain(|name| seen.insert(name.clone()));
    let resources = SchedulerResourceRequest::from_slurm_profile(deployment);
    let termination_notice_seconds = resources
        .wall_time_minutes
        .saturating_mul(60)
        .saturating_sub(1)
        .clamp(1, OUTER_TERMINATION_NOTICE_SECONDS);
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
        format!("--signal=TERM@{termination_notice_seconds}"),
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
    let mut inner = vec![
        env_prelude_with(deployment, read_environment)?,
        "umask 077".into(),
        format!(
            "mkdir -p -- {} {}",
            shell_quote(&staging_root),
            shell_quote(&cache_root)
        ),
    ];
    if let Some(name) = contract.output_environment_variable.as_deref() {
        inner.push(format!("export {name}={}", shell_quote(&staging_root)));
    }
    if let Some(name) = contract.shared_staging_environment_variable.as_deref() {
        inner.push(format!("export {name}={}", shell_quote(&cache_root)));
    }
    if let Some(publisher) = publisher {
        inner.extend([
            format!(
                "export {PUBLISHER_EXECUTION_ID_ENV}={}",
                shell_quote(&publisher.execution_id)
            ),
            format!(
                "export {PUBLISHER_EXECUTION_ATTEMPT_ENV}={}",
                publisher.execution_attempt
            ),
            format!(
                "export {PUBLISHER_CORE_URL_ENV}={}",
                shell_quote(&publisher.core_url)
            ),
            format!(
                "export {PUBLISHER_DESTINATION_URI_ENV}={}",
                shell_quote(&publisher.durable_destination_uri)
            ),
            format!(
                "export {PUBLISHER_OUTPUT_ROOT_ENV}={}",
                shell_quote(&staging_root)
            ),
            format!(
                "export {PUBLISHER_TOKEN_FILE_ENV}={}",
                shell_quote(&publisher.token_file)
            ),
        ]);
    }
    inner.push(format!(
        "if [ -n \"${{PYTHONPATH:-}}\" ]; then export {PYTHON_PATH_ENV}={}:\"$PYTHONPATH\"; else export {PYTHON_PATH_ENV}={}; fi",
        shell_quote(python_path),
        shell_quote(python_path),
    ));
    inner.push(
        argv.iter()
            .map(|argument| shell_quote(argument))
            .collect::<Vec<_>>()
            .join(" "),
    );
    let inner = inner.join("\n");
    Ok(format!("bash -lc {}", shell_quote(&inner)))
}

fn sbatch_command(
    deployment: &SlurmRemoteDeploymentConfig,
    session_id: &str,
    jobsub_path: &str,
    staging_root: &str,
    cache_root: &str,
    python_path: &str,
    publisher: Option<&ResolvedPublisherRuntime>,
) -> Result<String, OrchestrationError> {
    sbatch_command_with(
        deployment,
        session_id,
        jobsub_path,
        staging_root,
        cache_root,
        python_path,
        publisher,
        |name| std::env::var(name).ok(),
    )
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
        if let Some((_, path)) = line.split_once(JOBSUB_CREATED_RE) {
            let path = path.trim();
            if !path.is_empty() {
                return Ok(path.to_string());
            }
        }
    }
    Err(OrchestrationError::Backend(format!(
        "create_dlg_job did not print job submission script path; stdout={stdout:?}"
    )))
}

pub fn parse_sbatch_job_id(stdout: &str) -> Result<String, OrchestrationError> {
    let candidates: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let mut fields = line.split(';');
            let Some(job_id) = fields.next() else {
                return false;
            };
            if job_id.is_empty() || !job_id.bytes().all(|byte| byte.is_ascii_digit()) {
                return false;
            }
            match (fields.next(), fields.next()) {
                (None, None) => true,
                (Some(cluster), None) => {
                    !cluster.is_empty()
                        && cluster.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                        })
                }
                _ => false,
            }
        })
        .collect();
    match candidates.as_slice() {
        [candidate] => Ok(candidate
            .split_once(';')
            .map(|(job_id, _)| job_id)
            .unwrap_or(candidate)
            .to_string()),
        [] => Err(OrchestrationError::Backend(
            "sbatch --parsable returned no valid numeric job ID".into(),
        )),
        _ => Err(OrchestrationError::Backend(
            "sbatch --parsable returned multiple possible job IDs".into(),
        )),
    }
}

fn parse_dispatched_sbatch_job_id(stdout: &str) -> Result<String, OrchestrationError> {
    parse_sbatch_job_id(stdout).map_err(|error| {
        let non_empty_lines = stdout.lines().filter(|line| !line.trim().is_empty()).count();
        OrchestrationError::SubmissionUncertain(format!(
            "sbatch exited successfully, but its submission receipt was invalid: {error}; non_empty_lines={non_empty_lines}"
        ))
    })
}

fn normalized_remote_absolute_path(
    raw_path: &str,
    label: &str,
) -> Result<PathBuf, OrchestrationError> {
    if raw_path.chars().any(char::is_control) {
        return Err(OrchestrationError::Backend(format!(
            "{label} contains control characters"
        )));
    }
    if raw_path.contains(',') || raw_path.contains(':') {
        return Err(OrchestrationError::Backend(format!(
            "{label} contains a delimiter that is unsafe for container bind paths"
        )));
    }
    let path = Path::new(raw_path);
    if !path.is_absolute() {
        return Err(OrchestrationError::Backend(format!(
            "{label} must be an absolute path"
        )));
    }

    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(OrchestrationError::Backend(format!(
                    "{label} must not contain relative or traversal components"
                )));
            }
        }
    }
    Ok(normalized)
}

fn derive_session_paths(
    jobsub_path: &str,
    dlg_root: &str,
    contract: &SlurmRuntimeContractConfig,
) -> Result<(String, String, String), OrchestrationError> {
    contract
        .validate()
        .map_err(|error| OrchestrationError::Backend(error.to_string()))?;
    let jobsub_path = normalized_remote_absolute_path(jobsub_path, "job submission script path")?;
    let dlg_root = normalized_remote_absolute_path(dlg_root, "DLG_ROOT")?;
    if dlg_root == Path::new("/") {
        return Err(OrchestrationError::Backend(
            "DLG_ROOT must be a dedicated directory, not the remote filesystem root".into(),
        ));
    }
    if !jobsub_path.starts_with(&dlg_root) || jobsub_path == dlg_root {
        return Err(OrchestrationError::Backend(
            "job submission script path must be beneath DLG_ROOT".into(),
        ));
    }
    let session_dir = jobsub_path
        .parent()
        .filter(|path| *path != dlg_root)
        .ok_or_else(|| {
            OrchestrationError::Backend(
                "job submission script path must be inside a session directory beneath DLG_ROOT"
                    .into(),
            )
        })?;
    let staging_root = session_dir.join(&contract.output_subdirectory);
    let cache_root = dlg_root.join(&contract.shared_staging_subdirectory);
    Ok((
        session_dir.to_string_lossy().into_owned(),
        staging_root.to_string_lossy().into_owned(),
        cache_root.to_string_lossy().into_owned(),
    ))
}

fn publisher_secret_paths(session_dir: &str) -> Result<(String, String), OrchestrationError> {
    let session_dir = normalized_remote_absolute_path(session_dir, "DALiuGE session directory")?;
    let directory = session_dir.join(PUBLISHER_SECRETS_DIRECTORY);
    let token_file = directory.join(PUBLISHER_TOKEN_FILENAME);
    Ok((
        directory.to_string_lossy().into_owned(),
        token_file.to_string_lossy().into_owned(),
    ))
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
        publisher_credential,
    } = params;
    let publication_inputs = match publisher_credential.as_ref() {
        Some(_) => {
            let publication = deployment.publication.as_ref().ok_or_else(|| {
                OrchestrationError::Backend(
                    "required output publication has no pinned deployment publication contract"
                        .into(),
                )
            })?;
            Some(resolve_publication_inputs_with(publication, |name| {
                std::env::var(name).ok()
            })?)
        }
        None => None,
    };
    let dlg_root = deployment.dlg_root.trim_end_matches('/').to_string();
    let staging_dir = format!("{dlg_root}/staging");
    let pgt_remote_path = format!("{staging_dir}/BeampipeExecution_{execution_id}.pgt.graph");
    let config_file_remote_path = format!("{staging_dir}/BeampipeExecution_{execution_id}.ini");
    let slurm_template_remote_path = deployment
        .slurm_template
        .as_ref()
        .filter(|t| !t.trim().is_empty())
        .map(|_| format!("{staging_dir}/BeampipeExecution_{execution_id}.slurm"));

    bind_physical_graph_to_session(&mut pgt_json, &session_id);

    let target = SlurmTarget::from_deployment(&deployment, &username);
    let mut session = SlurmSshSession::connect(&target).await?;

    session
        .run_command(&format!("mkdir -p -- {}", shell_quote(&staging_dir)))
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
    let (session_dir, staging_root, cache_root) =
        derive_session_paths(&jobsub_path, &dlg_root, &deployment.runtime_contract)?;
    let python_shim_dir = format!("{session_dir}/.beampipe-python");
    session
        .run_command(&format!("mkdir -p -- {}", shell_quote(&python_shim_dir)))
        .await?;
    session
        .upload_text_atomic(
            &format!("{python_shim_dir}/sitecustomize.py"),
            DALIUGE_FAILED_SESSION_SITECUSTOMIZE,
        )
        .await?;
    let publisher_runtime = match (publisher_credential.as_ref(), publication_inputs) {
        (Some(credential), Some(inputs)) => {
            let (secrets_directory, token_file) = publisher_secret_paths(&session_dir)?;
            Some((
                ResolvedPublisherRuntime {
                    execution_id: execution_id.clone(),
                    execution_attempt: credential.execution_attempt(),
                    core_url: inputs.core_url,
                    durable_destination_uri: inputs.durable_destination_uri,
                    token_file,
                },
                secrets_directory,
            ))
        }
        (None, None) => None,
        _ => {
            return Err(OrchestrationError::Backend(
                "publisher runtime inputs were not resolved consistently".into(),
            ));
        }
    };
    let sbatch = sbatch_command(
        &deployment,
        &session_id,
        &jobsub_path,
        &staging_root,
        &cache_root,
        &python_shim_dir,
        publisher_runtime.as_ref().map(|(runtime, _)| runtime),
    )?;
    if let (Some(credential), Some((runtime, secrets_directory))) =
        (publisher_credential.as_ref(), publisher_runtime.as_ref())
    {
        session
            .upload_secret_atomic(
                secrets_directory,
                &runtime.token_file,
                credential.token_bytes(),
            )
            .await?;
    }
    let sbatch_out = match session.run_submission_command(&sbatch).await {
        Ok(output) => output,
        Err(error) => {
            if !matches!(error, OrchestrationError::SubmissionUncertain(_)) {
                if let Some((runtime, _)) = publisher_runtime.as_ref() {
                    let _ = session.remove_file_sftp(&runtime.token_file).await;
                }
            }
            let _ = session.close().await;
            return Err(error);
        }
    };
    let _ = session.close().await;

    let slurm_job_id = parse_dispatched_sbatch_job_id(&sbatch_out)?;
    let composite = beampipe_domain::slurm::compose_scheduler_job_id(
        &session_id,
        &slurm_job_id,
        Some(&session_dir),
    )
    .map_err(|error| {
        OrchestrationError::SubmissionUncertain(format!(
            "sbatch accepted job {slurm_job_id}, but its receipt could not be encoded: {error}"
        ))
    })?;
    Ok(SlurmSubmitResult {
        slurm_job_id,
        session_dir,
        staging_root: Some(staging_root),
        composite_scheduler_job_id: composite,
    })
}

/// Preflight SSH to the Slurm login node before translation and submission.
pub async fn probe_slurm_login(
    deployment: &SlurmRemoteDeploymentConfig,
    username: &str,
    publication_required: bool,
) -> Result<(), String> {
    // Resolve all local, non-secret publication inputs before opening an SSH
    // connection. A missing or insecure callback/destination is a local
    // configuration failure, not a remote probe.
    let preflight = slurm_preflight_script(deployment, publication_required)
        .map_err(|error| error.to_string())?;
    let target = SlurmTarget::from_deployment(deployment, username);
    let mut session = SlurmSshSession::connect(&target).await.map_err(|e| {
        format!(
            "Slurm login node {} ({}@{}) unreachable: {e}. Check VPN/SSH before submit.",
            deployment.login_node, username, deployment.login_node
        )
    })?;
    session
        .run_command(&format!("bash -lc {}", shell_quote(&preflight)))
        .await
        .map_err(|e| {
        format!(
            "Slurm runtime preflight failed on {} ({}@{}): {e}. Check the profile runtime contract and shared root before submit.",
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
    use beampipe_profiles::SlurmRuntimeEnvironmentRequirement;

    #[test]
    fn parse_jobsub_extracts_path() {
        let stdout = "Created job submission script /home/user/session root/x/job sub.sh\n";
        assert_eq!(
            parse_jobsub_path(stdout).unwrap(),
            "/home/user/session root/x/job sub.sh"
        );
    }

    #[test]
    fn sbatch_receipt_parser_accepts_one_id_after_a_banner() {
        assert_eq!(
            parse_sbatch_job_id("module environment ready\n123456;setonix\n").unwrap(),
            "123456"
        );
    }

    #[test]
    fn sbatch_receipt_parser_fails_closed_on_missing_ambiguous_or_unsafe_output() {
        for output in [
            "",
            "Submitted batch job 123456\n",
            "123456\n654321\n",
            "123456;setonix;extra\n",
            "123456;setonix && touch /tmp/bad\n",
        ] {
            assert!(parse_sbatch_job_id(output).is_err(), "accepted {output:?}");
        }
    }

    #[test]
    fn malformed_successful_sbatch_receipt_is_submission_uncertain() {
        for output in ["", "Submitted batch job 123456\n", "123456\n654321\n"] {
            assert!(matches!(
                parse_dispatched_sbatch_job_id(output),
                Err(OrchestrationError::SubmissionUncertain(_))
            ));
        }
        assert_eq!(
            parse_dispatched_sbatch_job_id("module ready\n123456;setonix\n").unwrap(),
            "123456"
        );
    }

    #[test]
    fn physical_graph_session_binding_is_deterministic_and_idempotent() {
        let mut physical_graph = serde_json::json!(["translator-name.pgt.graph", {"oid": "a"}]);

        bind_physical_graph_to_session(&mut physical_graph, "beampipe-session-1");
        let dispatched = physical_graph.clone();
        bind_physical_graph_to_session(&mut physical_graph, "beampipe-session-1");

        assert_eq!(physical_graph, dispatched);
        assert_eq!(physical_graph[0], "beampipe-session-1.pgt.graph");
    }

    #[test]
    fn session_paths_are_absolute_contained_and_space_safe() {
        let (session_dir, staging_root, cache_root) = derive_session_paths(
            "/scratch/project root/dlg/sessions/execution one/job sub.sh",
            "/scratch/project root/dlg",
            &SlurmRuntimeContractConfig::default(),
        )
        .unwrap();

        assert_eq!(
            session_dir,
            "/scratch/project root/dlg/sessions/execution one"
        );
        assert_eq!(
            staging_root,
            "/scratch/project root/dlg/sessions/execution one/outputs"
        );
        assert_eq!(cache_root, "/scratch/project root/dlg/shared_staging");
    }

    #[test]
    fn session_paths_reject_relative_traversal_control_and_outside_paths() {
        for jobsub_path in [
            "sessions/execution/jobsub.sh",
            "/dlg/sessions/../outside/jobsub.sh",
            "/dlg/sessions/execution\n/jobsub.sh",
            "/dlg/sessions/execution,other/jobsub.sh",
            "/dlg/sessions/execution:other/jobsub.sh",
            "/other/sessions/execution/jobsub.sh",
            "/dlg/jobsub.sh",
        ] {
            assert!(
                derive_session_paths(jobsub_path, "/dlg", &SlurmRuntimeContractConfig::default())
                    .is_err(),
                "accepted {jobsub_path:?}"
            );
        }
        assert!(derive_session_paths(
            "/sessions/execution/jobsub.sh",
            "/",
            &SlurmRuntimeContractConfig::default()
        )
        .is_err());
    }

    #[test]
    fn outer_sbatch_separates_run_outputs_from_the_shared_cache() {
        let mut dep = deployment();
        dep.dlg_root = "/dlg root".into();
        dep.runtime_contract = wallaby_runtime_contract();
        let (_, output_a, cache_a) = derive_session_paths(
            "/dlg root/sessions/execution-a/job sub.sh",
            &dep.dlg_root,
            &dep.runtime_contract,
        )
        .unwrap();
        let (_, output_b, cache_b) = derive_session_paths(
            "/dlg root/sessions/execution-b/job sub.sh",
            &dep.dlg_root,
            &dep.runtime_contract,
        )
        .unwrap();
        assert_ne!(output_a, output_b);
        assert_eq!(cache_a, cache_b);

        let command = sbatch_command_with(
            &dep,
            "execution-a",
            "/dlg root/sessions/execution-a/job sub.sh",
            &output_a,
            &cache_a,
            "/dlg root/sessions/execution-a/.beampipe-python",
            None,
            |name| (name == "BEAMPIPE_ASKAPSOFT_SIF").then(|| "/images/askap.sif".into()),
        )
        .unwrap();
        for expected in [
            "--export=BEAMPIPE_SLURM_ACCOUNT,WALLABY_HIRES_STAGING_ROOT,WALLABY_HIRES_CACHE_ROOT,PYTHONPATH,BEAMPIPE_ASKAPSOFT_SIF",
            "export BEAMPIPE_SLURM_ACCOUNT=myacct",
            "export BEAMPIPE_ASKAPSOFT_SIF=/images/askap.sif",
            "export WALLABY_HIRES_STAGING_ROOT=",
            "export WALLABY_HIRES_CACHE_ROOT=",
            "export PYTHONPATH=",
            "/dlg root/sessions/execution-a/.beampipe-python",
            "/dlg root/sessions/execution-a/wallaby_outputs",
            "/dlg root/wallaby_staging_data",
            "mkdir -p --",
        ] {
            assert!(
                command.contains(expected),
                "missing {expected:?} in {command}"
            );
        }
    }

    #[test]
    fn daliuge_failed_session_shim_marks_failed_terminal() {
        for expected in [
            "from dlg.deploy import common",
            "SessionStates",
            "state == _beampipe_session_states.FAILED",
            "_beampipe_common._is_end_state = _beampipe_is_end_state",
        ] {
            assert!(
                DALIUGE_FAILED_SESSION_SITECUSTOMIZE.contains(expected),
                "missing {expected:?}"
            );
        }
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
    fn runtime_contract_forwards_required_process_values_safely() {
        let mut dep = deployment();
        dep.runtime_contract.required_environment = vec![SlurmRuntimeEnvironmentRequirement {
            name: "BEAMPIPE_ASKAPSOFT_SIF".into(),
            kind: SlurmRuntimeEnvironmentKind::ReadableFile,
        }];
        dep.environment_setup =
            Some("export BEAMPIPE_SLURM_ACCOUNT=\"$BEAMPIPE_SLURM_ACCOUNT\"".into());
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
    fn runtime_contract_rejects_missing_required_values() {
        let mut dep = deployment();
        dep.runtime_contract.required_environment = vec![SlurmRuntimeEnvironmentRequirement {
            name: "BEAMPIPE_ASKAPSOFT_SIF".into(),
            kind: SlurmRuntimeEnvironmentKind::ReadableFile,
        }];
        let error = env_prelude_with(&dep, |_| None).unwrap_err();
        assert!(error.to_string().contains("BEAMPIPE_ASKAPSOFT_SIF"));
    }

    #[test]
    fn preflight_checks_the_exact_runtime_before_submission() {
        let mut dep = deployment();
        dep.dlg_root = "/scratch/project/user/dlg root".into();
        dep.runtime_contract = wallaby_runtime_contract();
        let script = slurm_preflight_script_with(&dep, false, |name| {
            (name == "BEAMPIPE_ASKAPSOFT_SIF").then(|| "/images/askapsoft.sif".into())
        })
        .unwrap();

        for expected in [
            "command -v sbatch",
            "command -v squeue",
            "command -v sacct",
            "command -v scancel",
            "command -v scontrol",
            "command -v srun",
            "command -v python3",
            "command -v wallaby_hires",
            "test -d '/scratch/project/user/dlg root'",
            "dlg.deploy.create_dlg_job",
            "wallaby_hires",
            "command -v singularity",
            "test -f \"${BEAMPIPE_ASKAPSOFT_SIF}\"",
        ] {
            assert!(
                script.contains(expected),
                "missing {expected:?} in {script}"
            );
        }
    }

    #[test]
    fn sbatch_runs_with_exports_in_the_same_remote_shell() {
        let mut dep = deployment();
        dep.runtime_contract = wallaby_runtime_contract();
        dep.resources.partition = Some("work".into());
        dep.resources.nodes = Some(2);
        dep.resources.tasks = Some(2);
        dep.resources.cpus_per_task = Some(4);
        dep.resources.memory = Some("12G".into());
        dep.resources.wall_time_minutes = Some(50);
        dep.resources.constraint = Some("cpu".into());
        dep.resources.quality_of_service = Some("normal".into());
        let command = sbatch_command_with(
            &dep,
            "session id",
            "/dlg/job sub.sh",
            "/dlg/wallaby_staging_data",
            "/dlg/shared-cache",
            "/dlg/.beampipe-python",
            None,
            |name| (name == "BEAMPIPE_ASKAPSOFT_SIF").then(|| "/images/askap soft.sif".into()),
        )
        .unwrap();

        assert!(command.starts_with("bash -lc "));
        assert!(command.contains("export BEAMPIPE_SLURM_ACCOUNT=myacct"));
        assert!(command.contains("export BEAMPIPE_ASKAPSOFT_SIF="));
        for expected in [
            "--export=BEAMPIPE_SLURM_ACCOUNT,WALLABY_HIRES_STAGING_ROOT,WALLABY_HIRES_CACHE_ROOT,PYTHONPATH,BEAMPIPE_ASKAPSOFT_SIF",
            "--parsable",
            "--job-name=session id",
            "--account=myacct",
            "--partition=work",
            "--nodes=2",
            "--ntasks=2",
            "--cpus-per-task=4",
            "--mem=12G",
            "--time=00:50:00",
            "--signal=TERM@120",
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

    #[test]
    fn short_outer_jobs_receive_a_bounded_termination_notice() {
        let mut dep = deployment();
        dep.resources.wall_time_minutes = Some(1);
        let command = sbatch_command_with(
            &dep,
            "session-id",
            "/dlg/jobsub.sh",
            "/dlg/wallaby_staging_data",
            "/dlg/shared-cache",
            "/dlg/.beampipe-python",
            None,
            |_| None,
        )
        .unwrap();

        assert!(command.contains("--time=00:01:00"));
        assert!(command.contains("--signal=TERM@59"));
    }

    #[test]
    fn generic_runtime_has_no_wallaby_requirements_or_names() {
        let dep = deployment();
        let script = slurm_preflight_script_with(&dep, false, |_| None).unwrap();
        assert!(!script.to_ascii_lowercase().contains("wallaby"));
        assert!(!script.to_ascii_lowercase().contains("askap"));

        let (_, output_root, shared_root) = derive_session_paths(
            "/dlg/sessions/execution-a/jobsub.sh",
            "/dlg",
            &dep.runtime_contract,
        )
        .unwrap();
        assert_eq!(output_root, "/dlg/sessions/execution-a/outputs");
        assert_eq!(shared_root, "/dlg/shared_staging");

        let command = sbatch_command_with(
            &dep,
            "execution-a",
            "/dlg/sessions/execution-a/jobsub.sh",
            &output_root,
            &shared_root,
            "/dlg/sessions/execution-a/.beampipe-python",
            None,
            |_| None,
        )
        .unwrap();
        assert!(!command.to_ascii_lowercase().contains("wallaby"));
        assert!(!command.to_ascii_lowercase().contains("askap"));
        assert!(command.contains("--export=BEAMPIPE_SLURM_ACCOUNT,PYTHONPATH"));
    }

    #[test]
    fn publication_runtime_is_resolved_and_validated_before_remote_use() {
        let publication = PublicationRuntimeConfig {
            core_url_environment: "SETONIX_BEAMPIPE_CORE_URL".into(),
            durable_destination_uri_environment: "SETONIX_BEAMPIPE_OUTPUT_DESTINATION".into(),
            allow_insecure_core_http: false,
            credential_ttl_minutes: 720,
        };
        let resolved = resolve_publication_inputs_with(&publication, |name| match name {
            "SETONIX_BEAMPIPE_CORE_URL" => Some("https://core.example.org/".into()),
            "SETONIX_BEAMPIPE_OUTPUT_DESTINATION" => {
                Some("s3://science-products/beampipe/".into())
            }
            _ => None,
        })
        .unwrap();
        assert_eq!(resolved.core_url, "https://core.example.org");
        assert_eq!(
            resolved.durable_destination_uri,
            "s3://science-products/beampipe"
        );

        for core_url in [
            "http://core.example.org",
            "https://user:password@core.example.org",
            "https://core.example.org?token=secret",
        ] {
            assert!(resolve_publication_inputs_with(&publication, |name| match name {
                "SETONIX_BEAMPIPE_CORE_URL" => Some(core_url.into()),
                "SETONIX_BEAMPIPE_OUTPUT_DESTINATION" => {
                    Some("file:///durable/beampipe".into())
                }
                _ => None,
            })
            .is_err());
        }
        assert!(resolve_publication_inputs_with(&publication, |_| None).is_err());
    }

    #[test]
    fn loopback_http_requires_an_explicit_development_override() {
        assert!(validated_core_url("http://127.0.0.1:18080", false).is_err());
        assert_eq!(
            validated_core_url("http://127.0.0.1:18080/", true).unwrap(),
            "http://127.0.0.1:18080"
        );
        assert!(validated_core_url("http://core.internal:18080", true).is_err());
    }

    #[test]
    fn publication_preflight_is_standalone_and_project_neutral() {
        let mut dep = deployment();
        dep.runtime_contract
            .required_python_modules
            .push("beampipe_pallette.apps".into());
        dep.publication = Some(PublicationRuntimeConfig {
            core_url_environment: "BEAMPIPE_CORE_URL".into(),
            durable_destination_uri_environment: "BEAMPIPE_OUTPUT_DESTINATION_URI".into(),
            allow_insecure_core_http: false,
            credential_ttl_minutes: 720,
        });
        let script = slurm_preflight_script_with(&dep, true, |name| match name {
            "BEAMPIPE_CORE_URL" => Some("https://core.example.org".into()),
            "BEAMPIPE_OUTPUT_DESTINATION_URI" => Some("file:///durable/beampipe".into()),
            _ => None,
        })
        .unwrap();
        assert!(script.contains("command -v beampipe-publish"));
        assert!(script.contains("beampipe_pallette.apps"));
        assert!(script.contains("BeampipeIngestApp"));
        assert!(script.contains("BeampipePublishApp"));
        assert!(!script.to_ascii_lowercase().contains("wallaby"));

        let opt_out = slurm_preflight_script_with(&dep, false, |_| None).unwrap();
        assert!(!opt_out.contains("beampipe-publish"));
        assert!(opt_out.contains("beampipe_pallette.apps"));
        assert!(opt_out.contains("BeampipeIngestApp"));
        assert!(!opt_out.contains("BeampipePublishApp"));
    }

    #[test]
    fn publisher_command_exports_paths_but_never_the_capability() {
        let token = "bpp_this-secret-must-never-enter-a-command-0123456789";
        let credential = PublisherRuntimeCredential::new(0, token.as_bytes().to_vec()).unwrap();
        let (secrets_directory, token_file) =
            publisher_secret_paths("/dlg/sessions/execution-a").unwrap();
        assert_eq!(
            secrets_directory,
            "/dlg/sessions/execution-a/.beampipe-secrets"
        );
        assert_eq!(
            token_file,
            "/dlg/sessions/execution-a/.beampipe-secrets/publisher.token"
        );
        let runtime = ResolvedPublisherRuntime {
            execution_id: "018f0000-0000-7000-8000-000000000001".into(),
            execution_attempt: credential.execution_attempt(),
            core_url: "https://core.example.org".into(),
            durable_destination_uri: "s3://science-products/beampipe".into(),
            token_file,
        };
        let dep = deployment();
        let command = sbatch_command_with(
            &dep,
            "execution-a",
            "/dlg/sessions/execution-a/jobsub.sh",
            "/dlg/sessions/execution-a/outputs",
            "/dlg/shared_staging",
            "/dlg/sessions/execution-a/.beampipe-python",
            Some(&runtime),
            |_| None,
        )
        .unwrap();
        for expected in [
            "BEAMPIPE_EXECUTION_ID",
            "BEAMPIPE_EXECUTION_ATTEMPT",
            "BEAMPIPE_CORE_URL",
            "BEAMPIPE_OUTPUT_DESTINATION_URI",
            "BEAMPIPE_OUTPUT_ROOT",
            "BEAMPIPE_PUBLISHER_TOKEN_FILE",
            "export BEAMPIPE_EXECUTION_ATTEMPT=0",
            "/dlg/sessions/execution-a/.beampipe-secrets/publisher.token",
            "https://core.example.org",
            "s3://science-products/beampipe",
        ] {
            assert!(
                command.contains(expected),
                "missing {expected:?} in {command}"
            );
        }
        let physical_graph = serde_json::json!(["execution-a.pgt.graph", {"oid": "drop"}]);
        let receipt = SlurmSubmitResult {
            slurm_job_id: "42".into(),
            session_dir: "/dlg/sessions/execution-a".into(),
            staging_root: Some("/dlg/sessions/execution-a/outputs".into()),
            composite_scheduler_job_id: "execution-a:42".into(),
        };
        let observable = format!(
            "{command}\n{physical_graph}\n{receipt:?}\n{credential:?}"
        );
        assert!(!observable.contains(token));
        assert!(observable.contains("[REDACTED]"));
    }

    fn wallaby_runtime_contract() -> SlurmRuntimeContractConfig {
        SlurmRuntimeContractConfig {
            required_commands: vec!["wallaby_hires".into(), "singularity".into()],
            required_python_modules: vec!["wallaby_hires".into()],
            required_environment: vec![SlurmRuntimeEnvironmentRequirement {
                name: "BEAMPIPE_ASKAPSOFT_SIF".into(),
                kind: SlurmRuntimeEnvironmentKind::ReadableFile,
            }],
            output_subdirectory: "wallaby_outputs".into(),
            shared_staging_subdirectory: "wallaby_staging_data".into(),
            output_environment_variable: Some("WALLABY_HIRES_STAGING_ROOT".into()),
            shared_staging_environment_variable: Some("WALLABY_HIRES_CACHE_ROOT".into()),
        }
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
            runtime_contract: Default::default(),
            publication: None,
        }
    }
}

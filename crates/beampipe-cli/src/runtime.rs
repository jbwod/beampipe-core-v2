use crate::installation::{InstallationContext, RuntimeMode};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

const STACK_SERVICES: &[&str] = &["api", "scheduler", "worker"];

#[derive(Debug, Serialize)]
pub struct RuntimeStatus {
    pub home: PathBuf,
    pub configured: bool,
    pub runtime: Option<RuntimeMode>,
    pub beampipe_version: Option<String>,
    pub operator_bundle_version: Option<String>,
    pub compose_project: Option<String>,
    pub environment_file: PathBuf,
    pub environment_present: bool,
    pub compose_file: PathBuf,
    pub compose_present: bool,
    pub credential_root: PathBuf,
    pub docker: Option<DockerStatus>,
}

#[derive(Debug, Serialize)]
pub struct DockerStatus {
    pub available: bool,
    pub services: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn start(context: &InstallationContext) -> Result<()> {
    require_runtime(context, RuntimeMode::Docker)?;
    compose_status(context, &["up", "-d", "--wait"], STACK_SERVICES)
}

pub fn stop(context: &InstallationContext) -> Result<()> {
    match runtime_mode(context)? {
        RuntimeMode::Docker => compose_status(context, &["stop"], &[]),
        RuntimeMode::Host => bail!(
            "host runtime is not daemonized by Beampipe; stop the foreground process or its service manager"
        ),
    }
}

pub fn restart(context: &InstallationContext) -> Result<()> {
    match runtime_mode(context)? {
        RuntimeMode::Docker => compose_status(
            context,
            &["up", "-d", "--wait", "--force-recreate"],
            STACK_SERVICES,
        ),
        RuntimeMode::Host => bail!(
            "host runtime is not daemonized by Beampipe; restart it with your service manager or stop it and run `beampipe start`"
        ),
    }
}

pub fn logs(
    context: &InstallationContext,
    service: Option<&str>,
    follow: bool,
    tail: usize,
) -> Result<()> {
    require_runtime(context, RuntimeMode::Docker)?;
    if let Some(service) = service {
        validate_service(service)?;
    }
    let tail = tail.to_string();
    let mut args = vec!["logs", "--tail", tail.as_str()];
    if follow {
        args.push("--follow");
    }
    if let Some(service) = service {
        args.push(service);
    }
    compose_status(context, &args, &[])
}

pub fn status(context: &InstallationContext) -> RuntimeStatus {
    let compose_file = context.home.join("docker-compose.yml");
    let state = context.state.as_ref();
    let docker = state
        .filter(|state| state.runtime == RuntimeMode::Docker)
        .map(|_| docker_status(context));
    RuntimeStatus {
        home: context.home.clone(),
        configured: state.is_some(),
        runtime: state.map(|state| state.runtime),
        beampipe_version: state.map(|state| state.beampipe_version.clone()),
        operator_bundle_version: state.map(|state| state.operator_bundle_version.clone()),
        compose_project: state.map(|state| state.compose_project.clone()),
        environment_file: context.environment_file.clone(),
        environment_present: context.environment_file.is_file(),
        compose_present: compose_file.is_file(),
        compose_file,
        credential_root: context.credential_root.clone(),
        docker,
    }
}

pub fn compose_command(context: &InstallationContext) -> Result<Command> {
    let state = context.state.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not a configured Beampipe installation; run `beampipe --home {} setup`",
            context.home.display(),
            context.home.display()
        )
    })?;
    let compose_file = context.home.join("docker-compose.yml");
    if !compose_file.is_file() {
        bail!(
            "operator Compose file missing at {}; rerun `beampipe setup` to restore managed files",
            compose_file.display()
        );
    }
    let mut command = Command::new("docker");
    command
        .arg("compose")
        .arg("--project-directory")
        .arg(&context.home)
        .arg("--env-file")
        .arg(&context.environment_file)
        .arg("--file")
        .arg(compose_file)
        .arg("--project-name")
        .arg(&state.compose_project);
    Ok(command)
}

fn compose_status(context: &InstallationContext, args: &[&str], services: &[&str]) -> Result<()> {
    require_docker()?;
    let mut command = compose_command(context)?;
    command.args(args).args(services);
    let status = command.status().context("run Docker Compose")?;
    if !status.success() {
        bail!("Docker Compose {} failed with {status}", args.join(" "));
    }
    Ok(())
}

fn docker_status(context: &InstallationContext) -> DockerStatus {
    let result = (|| -> Result<Output> {
        require_docker()?;
        compose_command(context)?
            .args(["ps", "--format", "json"])
            .output()
            .context("inspect Docker Compose services")
    })();
    match result {
        Ok(output) if output.status.success() => DockerStatus {
            available: true,
            services: parse_compose_ps(&output.stdout),
            error: None,
        },
        Ok(output) => DockerStatus {
            available: true,
            services: Value::Array(Vec::new()),
            error: Some(redacted_stderr(&output.stderr)),
        },
        Err(error) => DockerStatus {
            available: false,
            services: Value::Array(Vec::new()),
            error: Some(error.to_string()),
        },
    }
}

fn parse_compose_ps(stdout: &[u8]) -> Value {
    if stdout.is_empty() {
        return Value::Array(Vec::new());
    }
    if let Ok(value) = serde_json::from_slice(stdout) {
        return match value {
            Value::Object(_) => Value::Array(vec![value]),
            value => value,
        };
    }
    let values = String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect::<Vec<Value>>();
    Value::Array(values)
}

fn redacted_stderr(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr).trim().to_string();
    if message.is_empty() {
        "Docker Compose returned a non-zero status".into()
    } else {
        message
    }
}

fn require_docker() -> Result<()> {
    let output = Command::new("docker")
        .args(["compose", "version"])
        .output()
        .context("Docker Compose is unavailable; install Docker with the Compose plugin")?;
    if !output.status.success() {
        bail!("Docker Compose is unavailable or the Docker daemon is not running");
    }
    Ok(())
}

fn runtime_mode(context: &InstallationContext) -> Result<RuntimeMode> {
    context
        .state
        .as_ref()
        .map(|state| state.runtime)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "installation state missing at {}; run `beampipe setup` first",
                context.home.display()
            )
        })
}

fn require_runtime(context: &InstallationContext, expected: RuntimeMode) -> Result<()> {
    let actual = runtime_mode(context)?;
    if actual != expected {
        bail!(
            "installation runtime is {}; this command requires {}",
            actual.as_str(),
            expected.as_str()
        );
    }
    Ok(())
}

fn validate_service(service: &str) -> Result<()> {
    if !["postgres", "api", "scheduler", "worker"].contains(&service) {
        bail!("service must be one of: postgres, api, scheduler, worker");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::{
        InstallationState, INSTALLATION_SCHEMA_VERSION, INSTALLATION_STATE_FILE,
    };

    fn context() -> (tempfile::TempDir, InstallationContext) {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().canonicalize().unwrap();
        std::fs::write(home.join("docker-compose.yml"), "services: {}\n").unwrap();
        std::fs::write(home.join(".env"), "BEAMPIPE_VERSION=test\n").unwrap();
        let state = InstallationState {
            schema_version: INSTALLATION_SCHEMA_VERSION,
            beampipe_version: "test".into(),
            runtime: RuntimeMode::Docker,
            database_mode: "compose".into(),
            home: home.clone(),
            environment_file: home.join(".env"),
            config_file: home.join("beampipe.yaml"),
            credential_root: home.join("credentials/ssh"),
            operator_bundle_version: "test".into(),
            compose_project: "operator-test".into(),
        };
        std::fs::write(
            home.join(INSTALLATION_STATE_FILE),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let context = InstallationContext::from_home(home).unwrap();
        (directory, context)
    }

    #[test]
    fn compose_command_uses_explicit_installation_paths() {
        let (_directory, context) = context();
        let command = compose_command(&context).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(args.contains(&"--project-directory".into()));
        assert!(args.contains(&context.home.display().to_string()));
        assert!(args.contains(&context.environment_file.display().to_string()));
        assert!(args.contains(&"operator-test".into()));
    }

    #[test]
    fn logs_rejects_unknown_compose_services_before_running_docker() {
        let (_directory, context) = context();
        let error = logs(&context, Some("database"), false, 100).unwrap_err();
        assert!(error.to_string().contains("service must be one of"));
    }

    #[test]
    fn compose_ps_parser_accepts_json_lines() {
        let value = parse_compose_ps(b"{\"Service\":\"api\"}\n{\"Service\":\"worker\"}\n");
        assert_eq!(value.as_array().unwrap().len(), 2);
    }
}

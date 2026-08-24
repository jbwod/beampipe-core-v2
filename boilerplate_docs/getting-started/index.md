# Quick start

This path leaves external execution mocked and gets a healthy, project-neutral Beampipe control plane running locally. Allow about five minutes after the release image has downloaded.

## Before you begin

For the recommended path you need:

- Linux or macOS on AMD64 or ARM64;
- Docker with Compose v2 (`docker compose version`);
- `curl`, `tar`, and either `sha256sum` or `shasum`; and
- free local ports `5432`, `18080`, and `9090`.

You can change all three ports in the wizard. Native-host and existing-PostgreSQL installations are covered in [Install and configure](installation.md).

## 1. Run the guided installer

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

The installer shows three stages:

```text
[1/3] Check this machine
[2/3] Download and install
[3/3] Configure Beampipe
```

Choose **Docker Compose**, **managed PostgreSQL**, and the default ports for the shortest path. The wizard walks through install home, runtime, PostgreSQL, network ports, optional Dash, project and deployment choices, and a final review before configuration begins. It creates private random secrets, installs the release binary under `~/.local/bin`, writes the operator bundle under `~/beampipe`, applies database migrations, and starts the API, scheduler, and worker.

No scientific project is selected and no real DALiuGE or Slurm submission is enabled implicitly.

!!! tip "Want a tailored command?"

    Use the [interactive command builder](../index.md#install-builder) to select guided or unattended setup, Docker or host runtime, a custom install directory, ports, an optional project, and Dash.

## 2. Verify the installation

Open a new terminal, or make the freshly installed command available now:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then run the local checks:

```bash
beampipe status
beampipe doctor
curl -fsS http://127.0.0.1:18080/api/v2/health
```

`status` should show the configured services, `doctor` should finish without error diagnostics, and the health request should succeed. If you chose another API port, use it in the URL.

The last screen is intentionally ordered as **SETUP COMPLETE → ACCESS → NEXT ACTIONS**, so login details and the next safe command stay together. An unattended generated administrator password is written to `~/beampipe/credentials/admin/password` with mode `0600`; it is never printed to standard output.

Useful day-two commands:

```bash
beampipe logs --follow
beampipe restart
beampipe stop
beampipe start
```

## 3. Add one project

Choose one path; Core does not assume a project.

=== "Your project"

    Validate first, then add the immutable project revision:

    ```bash
    beampipe project validate -f PROJECT_CONFIG.yaml
    beampipe project add -f PROJECT_CONFIG.yaml
    ```

    Start from [`minimal_survey.v2.yaml`](https://github.com/jbwod/beampipe-core-v2/blob/main/config/examples/minimal_survey.v2.yaml), or follow [Project-neutral acceptance](neutral-project.md) for an offline end-to-end proof.

=== "WALLABY sample"

    Rerun the idempotent setup command and explicitly materialize the sample:

    ```bash
    beampipe setup --sample wallaby-hires
    ```

    Then continue with [WALLABY first workflow](first-run.md). Selecting this sample is the only quick-start path that installs WALLABY-specific project files.

## 4. Connect an execution backend

Add a typed profile and qualify it while external execution is still mocked:

```bash
beampipe profile add -f DEPLOYMENT_PROFILE.json
beampipe profile validate PROFILE_NAME
beampipe doctor --profile PROFILE_NAME
```

Only after the profile-specific doctor passes, edit `~/beampipe/.env`:

```dotenv
BEAMPIPE_USE_REAL_BACKENDS=true
```

Apply the change:

```bash
beampipe restart
beampipe doctor --profile PROFILE_NAME
```

!!! warning "Real backends are an explicit safety boundary"

    A successful local install does not qualify DALiuGE, Slurm, SSH, or archive credentials. Keep mock mode enabled until the selected profile and provider checks pass. SSH commands and remote workload submission should remain deliberate operator actions.

## If setup stops

The installer keeps the verified binary and prints a copyable resume command. Setup is idempotent, so after addressing the reported problem you can safely run:

```bash
beampipe --home "$HOME/beampipe" setup
```

Common checks:

| Symptom | Check |
|---|---|
| `beampipe: command not found` | Open a new terminal or export `~/.local/bin` into `PATH`. |
| Port already in use | Rerun setup and select different API, PostgreSQL, or metrics ports. |
| A service is unhealthy | Run `beampipe status`, then `beampipe logs --follow`. |
| Existing database is unreachable | Confirm its hostname works from both the host and the chosen runtime. |
| Unsure what was written | Inspect `~/beampipe/installation.json`; it contains runtime identity, not secrets. |

For every installation mode and upgrade behavior, see [Install and configure](installation.md). For optional web-console installation and operation, see [Dashboard setup and tour](dashboard.md).

# CLI command reference

The `beampipe` binary is the common entrypoint for local evaluation, API service, workers, diagnostics, and operator actions. Run `beampipe <command> --help` for the exact options supported by the installed release.

## Command families

| Intent | Commands | Typical role |
|---|---|---|
| Bootstrap | `init`, `setup`, `migrate`, `admin` | Installer |
| Run services | `start`, `serve`, `worker` | Operator |
| Verify | `doctor`, `security`, `bench` | Operator / security |
| Configure | `config`, `project`, `profile`, `wasm` | Project maintainer |
| Inspect backends | `scheduler`, `daliuge`, `slurm` | HPC operator |
| Operate executions | `execution`, `graph`, `timeline`, `status`, `console` | Operator |
| Maintain contracts | `openapi`, `migrate-data`, `purge-provenance` | Engineer |

## Bootstrap and run

```bash
beampipe init --directory operator-local
cd operator-local
beampipe setup
beampipe doctor
beampipe start
```

`start` runs the normal API plus embedded worker path. Use `serve` and `worker` separately when processes must scale or fail independently.

## Inspect before acting

```bash
beampipe status
beampipe worker list
beampipe scheduler status --profile PROFILE
beampipe daliuge inspect --profile PROFILE
beampipe timeline execution "$EXECUTION_ID" --table
```

The CLI and API use the same structured diagnostics. Errors carry a stable code, severity, path, message, and optional hint.

## Project and profile validation

```bash
beampipe project validate -f config/wallaby_hires.v2.yaml
beampipe profile validate config/deployment_profile.slurm-remote.json
beampipe graph prepare \
  --project wallaby_hires \
  --source WALLABY_J123456-123456
beampipe graph diff --execution "$EXECUTION_ID"
```

Validation should happen before upload or activation. Graph preparation writes deterministic artifacts and reports graph-patch matches before external submission.

## Recovery actions

```bash
beampipe execution retry "$EXECUTION_ID" \
  --reason "Translator endpoint restored after planned maintenance"

beampipe execution cancel "$EXECUTION_ID"
```

Retry and cancellation are stage-aware. An uncertain submission is fenced until reconciliation proves whether external work exists.

## Output conventions

- Human-readable tables are the default for interactive inspection.
- Structured diagnostics preserve stable fields for scripts and API clients.
- Secrets are redacted from explanations and diagnostic output.
- Non-zero exit status means at least one error diagnostic or command failure occurred.

Continue with the [API workflow](../api/index.md) for HTTP equivalents or the [operator handbook](../operations/index.md) for task-oriented procedures.

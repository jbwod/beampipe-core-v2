# CLI reference

`beampipe` is the common entry point for setup, services, diagnostics, project/profile management, and guarded operator actions. The installed release's `beampipe COMMAND --help` is authoritative for flags.

## Command families

| Intent | Commands |
|---|---|
| Bootstrap | `init`, `setup`, `migrate`, `admin create-user` |
| Run | `start`, `serve`, `worker` |
| Verify | `doctor`, `security check`, `config explain`, `bench` |
| Configure | `project`, `profile`, `wasm` |
| Inspect backends | `scheduler`, `daliuge`, `slurm` |
| Operate | `status`, `console`, `timeline`, `execution`, `graph` |
| Maintain | `openapi export`, `purge-provenance`, `migrate-data` |

## Bootstrap

```bash
beampipe init --directory operator-local
cd operator-local
beampipe setup
beampipe doctor
beampipe start
```

`start` runs a compact API plus worker. Use `serve --worker false` and worker-only processes when roles need independent scaling.

## Inspect

```bash
beampipe status
beampipe worker list
beampipe scheduler status --profile PROFILE
beampipe daliuge inspect --profile PROFILE
beampipe timeline execution "$EXECUTION_ID" --table
```

## Validate and prepare

```bash
beampipe project validate -f PROJECT.yaml
beampipe project add -f PROJECT.yaml
beampipe profile add -f PROFILE.json
beampipe profile validate PROFILE_NAME
beampipe profile render PROFILE_NAME
beampipe graph prepare --project PROJECT_ID --source SOURCE_ID
beampipe graph diff --execution "$EXECUTION_ID"
```

## Live checks

```bash
beampipe doctor --profile PROFILE_NAME
beampipe slurm ping --profile PROFILE_NAME
beampipe daliuge ping --profile PROFILE_NAME
beampipe scheduler status --profile PROFILE_NAME
```

## Guarded actions

```bash
beampipe worker drain "$WORKER_ID"
beampipe execution retry "$EXECUTION_ID" --reason "dependency restored"
beampipe execution cancel "$EXECUTION_ID"
```

Retries and cancellation share the same safety policy as the API and console. Uncertain external work blocks resubmission.

## Output conventions

- Inspection commands default to human-readable output where appropriate.
- Diagnostics expose stable code, severity, path, message, and hint fields.
- Secret-bearing values and external errors are redacted.
- Non-zero exit status indicates a failed command or error diagnostic.

Use the [API workflow](../api/index.md) for HTTP equivalents.

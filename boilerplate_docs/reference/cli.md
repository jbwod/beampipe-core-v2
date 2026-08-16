# CLI reference

`beampipe` is the common entry point for setup, services, diagnostics, project/profile management, and guarded operator actions. The installed release's `beampipe COMMAND --help` is authoritative for flags.

## Command families

| Intent | Commands |
|---|---|
| Bootstrap | `init`, `setup`, `migrate`, `admin create-user` |
| Run | `start`, `serve`, `worker` |
| Verify | `doctor`, `security check`, `config explain`, `bench` |
| Configure | `project`, `profile`, `wasm` |
| Inspect backends | `scheduler`, `daliuge`, `slurm`, `slurm credentials` |
| Operate | `status`, `console`, `timeline`, `execution`, `graph` |
| Maintain | `openapi export`, `purge-provenance`, `migrate-data` |

## Bootstrap

Prefer the installer (no clone):

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

`beampipe setup` writes an operator directory (`--directory`, default `~/beampipe` when cwd has no `docker-compose.yml`) and **starts** Postgres and the stack. `--yes` requires `--runtime docker` or `--runtime host` (`--docker` / `--skip-docker` are aliases). Interactive default is Docker. `--no-start` writes files and prints a recipe only. `--yes` without `--admin-password` generates a password and prints it once.

```bash
beampipe setup --directory ~/beampipe --yes --runtime docker
beampipe setup --no-start --yes --runtime host
```

PostgreSQL is the Compose `postgres` service (default) or an existing URL (`--postgres existing`). Docker runtime always uses Compose Postgres. If `--no-start` and Postgres is down, migrate / admin / upload / doctor are skipped and the recipe starts with `docker compose up -d postgres`.

Setup does not create a deployment profile. Install one later with `beampipe profile add`. Dash is docker-only and opt-in (`--dashboard`). `./deploy/setup-docker.sh` is the checkout developer path and passes `--no-start`. Set `BEAMPIPE_BUILD=1` to compile this checkout instead.

`beampipe init --directory` writes the same pull-only Compose file, sample project, and SSH dirs. Then run `beampipe setup --directory ...`. `start` runs a compact API plus worker.

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
beampipe slurm credentials init --slot setonix
beampipe slurm credentials check --slot setonix
beampipe doctor --profile PROFILE_NAME
beampipe slurm ping --profile PROFILE_NAME
beampipe daliuge ping --profile PROFILE_NAME
beampipe scheduler status --profile PROFILE_NAME
```

`slurm credentials init` writes `private_key`, optional `passphrase`, and `known_hosts` under the credentials root. It never accepts a passphrase on the command line; use a TTY prompt, `--passphrase-file`, or `--no-passphrase`. Setup does not create SSH slots; run this command when you add a Slurm profile.

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

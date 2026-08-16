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

From the repository root (where `docker-compose.yml` lives). `--yes` requires `--runtime docker` or `--runtime host` (`--docker` / `--skip-docker` are aliases). Setup never starts containers; it prints one recipe. PostgreSQL is either the Compose `postgres` service or an existing URL (`--postgres compose|existing`). The Compose service is line 1 of the recipe when you chose it (default when `docker-compose.yml` exists).

```bash
# Docker path (no host Rust)
./deploy/setup-docker.sh --yes --skip-admin --skip-upload

# Host path
beampipe setup --yes --runtime host --skip-admin --skip-upload
```

If Compose Postgres is down, migrate / admin / upload / doctor are skipped on **both** paths and the recipe starts with `docker compose up -d postgres`. `--postgres existing` uses `DATABASE_URL` and fails if that URL is down.

Setup does not create a deployment profile. Install one later with `beampipe profile add`. Dash is docker-only and opt-in (`--dashboard`). `./deploy/setup-docker.sh` builds the image and runs setup; it does not `compose up`.

`beampipe init --directory operator-local` is a compact native footnote. That directory has no Compose file; use `--runtime host --postgres existing`. `start` runs a compact API plus worker.

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

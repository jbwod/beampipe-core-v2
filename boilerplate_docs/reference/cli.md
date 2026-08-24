# CLI reference

`beampipe` is the common entry point for setup, services, diagnostics, project/profile management, and guarded operator actions. The installed release's `beampipe COMMAND --help` is authoritative for flags.

## Command families

| Intent | Commands |
|---|---|
| Bootstrap | `init`, `setup`, `uninstall`, `migrate`, `admin create-user` |
| Run | `start`, `stop`, `restart`, `logs`, `serve`, `worker` |
| Verify | `doctor`, `security check`, `config explain`, `bench` |
| Configure | `project`, `profile`, `wasm` |
| Inspect backends | `scheduler`, `daliuge`, `slurm`, `slurm credentials` |
| Operate | `status`, `console`, `timeline`, `execution`, `graph` |
| Maintain | `uninstall`, `openapi export`, `purge-provenance`, `migrate-data` |

## Bootstrap

Prefer the installer (no clone):

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

`beampipe setup` writes the selected installation (`--home`, then `BEAMPIPE_HOME`, then `~/beampipe`) and starts the configured runtime. Current working directory never selects an installation. `--yes` requires `--runtime docker` or `--runtime host`. `--no-start` writes files and prints a recipe only. Existing secrets and data are preserved on rerun. Host publish ports default to API `18080`, PostgreSQL `5432`, and metrics `9090`; override with `--api-port`, `--postgres-port`, and `--metrics-port`. Interactive setup then prompts Next actions (live backends, a deployment profile, Slurm SSH credentials, CASDA credentials, `doctor --profile`). `--yes` prints that recipe. `--use-real-backends` writes `BEAMPIPE_USE_REAL_BACKENDS=true` during setup.

```bash
beampipe --home ~/beampipe setup --yes --runtime docker --postgres compose
beampipe --home ~/beampipe setup --no-start --yes --runtime host --postgres existing \
  --database-url 'postgres://beampipe@127.0.0.1/beampipe'
```

PostgreSQL is the managed Compose service or an existing URL. Both Docker and host runtimes support either selection. Fresh managed databases receive a random password.

Setup can install a deployment profile with `--profile-config` and can assign/import a Slurm SSH slot. Dash remains Docker-only and opt-in (`--dashboard`); setup starts it after Core is up. `./deploy/setup-docker.sh` is the checkout developer path.

`beampipe init --directory` writes the pull-only Compose file, project/profile examples, and SSH directories. `start` dispatches to the recorded Docker or host runtime; `serve` remains the low-level foreground API command.

`beampipe uninstall` removes the selected installation (`--home`, then `BEAMPIPE_HOME`, then `~/beampipe`). It stops Compose services and deletes installation files. Managed PostgreSQL volumes are removed unless `--keep-volumes` is passed. SSH credential roots outside the installation home are kept. `--purge-binary` also deletes `~/.local/bin/beampipe`; the binary is kept by default. `--yes` skips the confirmation prompt.

```bash
beampipe uninstall
beampipe uninstall --yes
beampipe uninstall --yes --purge-binary
beampipe --home /path/to/install uninstall --yes --keep-volumes
```

## Inspect

```bash
beampipe status
beampipe logs --service worker --follow
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
beampipe slurm credentials init --slot hpc --host login.example.org
beampipe slurm credentials copy-id --slot hpc --user USER --host login.example.org
beampipe slurm credentials import --slot hpc \
  --private-key ~/.ssh/id_ed25519 --known-hosts ~/.ssh/known_hosts --acl
beampipe slurm credentials sync --slot hpc
beampipe slurm credentials check --slot hpc
beampipe doctor --profile PROFILE_NAME
beampipe slurm ping --profile PROFILE_NAME
beampipe daliuge ping --profile PROFILE_NAME
beampipe scheduler status --profile PROFILE_NAME
```

Credential commands resolve the active installation's canonical root. `--slot` is a directory name, not a hostname. `init` generates a key; `import` copies an existing key; neither logs you in until the public key is in the login node's `authorized_keys` (skip that upload when importing a key the cluster already has). They never accept a passphrase on the command line; use a TTY prompt or `--passphrase-file`. `sync` checks the recorded read-only Docker bind and live container readability without copying key material.

After reassigning every profile that references a slot, remove it explicitly with `beampipe slurm credentials remove --slot SLOT --yes`.

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

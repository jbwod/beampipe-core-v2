# Deployment profiles

A deployment profile tells beampipe how to translate and deploy a prepared graph. Executions reference a profile by UUID, by `deployment_profile_name`, or by the project default.

## Choosing a profile

| Backend | Use when | Operator concern |
|---------|----------|------------------|
| REST remote | A 流 DIM is already running and reachable over HTTP | 流 DIM URL, deploy/poll host, TLS policy |
| Slurm remote | The graph should run on an HPC cluster through SSH and `sbatch` | SSH trust, Slurm account, DALiuGE paths, poll cadence |

<div class="terminal-diagram terminal-diagram--center">
<pre>manifest + graph
      |
      v
流 Translator Manager
      |
      +--> REST remote 流 DIM
      |
      +--> Slurm remote SSH + sbatch</pre>
</div>

## Top-level shape

```json
{
  "name": "slurm-remote",
  "description": "Setonix Slurm profile",
  "project_module": "wallaby_hires",
  "is_default": true,
  "translation": {
    "algo": "metis",
    "num_par": 1,
    "num_islands": 1,
    "tm_url": "http://dlg-tm.example"
  },
  "deployment": {
    "kind": "slurm_remote"
  }
}
```

| Field | Required | Notes |
|-------|----------|-------|
| `name` | yes | 1-50 characters; referenced by executions and project automation |
| `description` | no | Operator notes |
| `project_module` | no | `null` means global profile |
| `is_default` | no | Default profile when an execution omits a name |
| `translation` | yes | DALiuGE 流 Translator Manager settings |
| `deployment` | yes | `rest_remote` or `slurm_remote` |

Profile changes apply to future executions. In-flight runs keep the config/profile already recorded on the execution.

## Translation fields

| Field | Default | Validation | Purpose |
|-------|---------|------------|---------|
| `algo` | `metis` | `metis` or `mysarkar` | DALiuGE partition algorithm |
| `num_par` | `1` | `>= 1` | Partition count |
| `num_islands` | `0` | `>= 0` | Island count |
| `tm_url` | unset | URL string | 流 Translator Manager base URL |

`tm_url` must be reachable from the worker that performs translation, not just from the operator laptop.

## REST remote

Use REST remote when a 流 DIM is already running and reachable over HTTP.

```json
{
  "kind": "rest_remote",
  "dim_host_for_tm": "dlg-dim",
  "dim_port_for_tm": 8001,
  "deploy_host": "dlg-dim.example",
  "deploy_port": 8001,
  "verify_ssl": false
}
```

| Field | Required | Default | Purpose |
|-------|----------|---------|---------|
| `kind` | yes | none | Must be `rest_remote` |
| `dim_host_for_tm` | no | unset | 流 DIM hostname as seen by 流 Translator Manager |
| `dim_port_for_tm` | no | `8001` | 流 DIM port as seen by 流 Translator Manager |
| `deploy_host` | no | unset | 流 DIM host used by beampipe for deploy and polling |
| `deploy_port` | no | `8001` | 流 DIM deploy/polling port |
| `verify_ssl` | no | `false` | Verify TLS certificates |

Best for local 流 DIM stacks, staging systems, and integration tests where a long-running 流 DIM service already exists.

## Slurm remote

Use Slurm remote for HPC clusters. The worker translates through 流 Translator Manager, uploads artifacts over SSH, submits with `sbatch`, and polls with batched `squeue`/`sacct`.

```json
{
  "kind": "slurm_remote",
  "login_node": "login.hpc.example",
  "ssh_port": 22,
  "remote_user": "operator",
  "account": "project_account",
  "home_dir": "/scratch/project_account",
  "log_dir": "/scratch/project_account/operator/dlg/log",
  "dlg_root": "/scratch/project_account/operator/dlg",
  "modules": "module load python/3.11",
  "venv": "source /path/to/venv/bin/activate",
  "exec_prefix": "srun -l",
  "facility": "setonix",
  "job_duration_minutes": 30,
  "num_nodes": 1,
  "num_islands": 1,
  "verbose_level": 1,
  "max_threads": 0,
  "all_nics": false,
  "zerorun": false,
  "sleepncopy": false,
  "check_with_session": false
}
```

| Field | Required | Default | Purpose |
|-------|----------|---------|---------|
| `kind` | yes | none | Must be `slurm_remote` |
| `login_node` | yes | none | Slurm login hostname |
| `ssh_port` | no | `22` | SSH port |
| `remote_user` | no | current user fallback | Remote SSH user |
| `account` | yes | none | Slurm account/project |
| `home_dir` | yes | none | Remote scratch/home path |
| `log_dir` | yes | none | DALiuGE log directory |
| `dlg_root` | yes | none | DALiuGE install root |
| `modules` | no | unset | Module load snippet before submit |
| `venv` | no | unset | Virtualenv activation snippet |
| `exec_prefix` | no | `srun -l` | Execution prefix passed to DALiuGE |
| `facility` | no | `setonix` | Facility id passed to DALiuGE |
| `job_duration_minutes` | no | `30` | Slurm wall time |
| `num_nodes` | no | `1` | Slurm node count |
| `num_islands` | no | `1` | DALiuGE island count for deploy |
| `verbose_level` | no | `1` | DALiuGE verbosity |
| `max_threads` | no | `0` | DALiuGE max threads |
| `all_nics` | no | `false` | Use all NICs |
| `zerorun` | no | `false` | DALiuGE zero-run mode |
| `sleepncopy` | no | `false` | DALiuGE sleep-and-copy behavior |
| `check_with_session` | no | `false` | Session checking behavior |
| `slurm_template` | no | unset | Optional Slurm template override |

Worker environment for Slurm:

```bash
export BEAMPIPE_USE_REAL_BACKENDS=true
export SLURM_SSH_PRIVATE_KEY_FILE=/run/secrets/slurm_ssh_key
export SLURM_SSH_KNOWN_HOSTS_SOURCE=/run/slurm-ssh/known_hosts
```

## Slurm SSH keys

Deployment profiles describe the remote Slurm target. SSH private keys, passphrases, and host-key trust stay outside the profile and are supplied to the worker process through environment variables or mounted files. This keeps profiles safe to store in Postgres and return through the API.

Production setup:

```bash
export BEAMPIPE_ENV=production
export BEAMPIPE_USE_REAL_BACKENDS=true
export SLURM_SSH_PRIVATE_KEY_FILE=/run/secrets/slurm_ssh_key
export SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE=/run/secrets/slurm_ssh_passphrase
export SLURM_SSH_KNOWN_HOSTS_SOURCE=/run/slurm-ssh/known_hosts
```

Use plain OpenSSH known-hosts entries for each Slurm login node. Non-default ports must use bracket syntax:

```text
login.hpc.example ssh-ed25519 AAAAC3...
[login.hpc.example]:2222 ssh-ed25519 AAAAC3...
```

Production rejects group/world-readable private keys, symlinked key paths, missing or empty known-hosts files, hashed known-hosts entries, and host keys that do not match the selected `login_node` and `ssh_port`.

Development may use an inline PEM or home-directory fallback, but production should not:

```bash
export SLURM_SSH_PRIVATE_KEY='-----BEGIN OPENSSH PRIVATE KEY-----...'
export BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK=true
```

In production, inline PEM requires `BEAMPIPE_ALLOW_INLINE_SECRETS=true`; disabling strict host-key verification requires `BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS=true`. Treat both as break-glass only.

## Validation

Run offline checks first:

```bash
beampipe security check
```

Then test the live deployment profile:

```bash
beampipe slurm ping --profile slurm-remote
```

If the ping fails, check the profile `login_node`, `ssh_port`, and `remote_user`, then the key path, key permissions, passphrase file, and known-hosts entry.

## API

```bash
curl -s -X POST "$BASE/api/v2/deployment-profiles" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d @profile.json | jq .

curl -s "$BASE/api/v2/deployment-profiles?project_module=wallaby_hires" \
  -H "$AUTH" | jq .

curl -s -X PATCH "$BASE/api/v2/deployment-profiles/$PROFILE_ID" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"is_default":true}' | jq .
```

Next: connect profiles to survey automation in [Project config YAML](../project-configs/index.md#automation).

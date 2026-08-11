# Deployment profiles and SSH

A deployment profile is versioned non-secret infrastructure policy. Every execution pins the resolved profile snapshot, so later edits affect only future runs.

## Choose a backend

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="A prepared graph is translated and then deployed either to an existing DIM or through SSH to Slurm">
  <div class="bp-flow-node" data-tone="amber"><span>INPUT</span><strong>patched graph</strong><small>immutable artifact</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>TRANSLATE</span><strong>DALiuGE TM</strong><small>logical to physical</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>OPTION A</span><strong>REST DIM</strong><small>deploy + poll HTTP</small></div>
  <span class="bp-flow-link" aria-hidden="true">or</span>
  <div class="bp-flow-node" data-tone="green"><span>OPTION B</span><strong>Slurm</strong><small>SSH + SFTP + sbatch</small></div>
</div>

| Kind | Use when | Prove before enabling |
|---|---|---|
| `rest_remote` | A DALiuGE Data Island Manager already runs | worker-to-TM, TM-to-DIM, worker-to-DIM connectivity and TLS |
| `slurm_remote` | DALiuGE should start inside an HPC allocation | SSH trust, account/partition, paths, runtime environment, `sbatch`/`squeue`/`sacct` |

Profiles never contain private keys, passphrases, CASDA passwords, or tokens.

## Create a profile

The setup wizard is the easiest path:

=== "REST remote"

    ```bash
    beampipe setup \
      --deployment rest_remote \
      --profile-name local-daliuge \
      --tm-url https://translator.example.org \
      --dim-url https://manager.example.org:8001
    ```

=== "Slurm remote"

    ```bash
    beampipe setup \
      --deployment slurm_remote \
      --profile-name setonix \
      --facility setonix \
      --ssh-host setonix.pawsey.org.au \
      --ssh-user "$USER" \
      --slurm-account PROJECT \
      --slurm-partition work \
      --remote-home /scratch/PROJECT \
      --dlg-root /scratch/PROJECT/$USER/dlg \
      --remote-logs /scratch/PROJECT/$USER/dlg/log
    ```

For automation or review, place the JSON profile in a private operator directory and install it with:

```bash
beampipe profile add -f profile.json
beampipe profile validate PROFILE_NAME
beampipe profile render PROFILE_NAME
```

`profile validate` accepts an installed profile name. File validation occurs during `profile add`.

## Common fields

```json
{
  "name": "setonix",
  "description": "WALLABY qualification profile",
  "project_module": "wallaby_hires",
  "is_default": true,
  "max_concurrent_executions": 1,
  "translation": {
    "algo": "metis",
    "num_par": 1,
    "num_islands": 1,
    "tm_url": "https://translator.example.org"
  },
  "deployment": {"kind": "slurm_remote"}
}
```

`project_module=null` makes a profile global. Project automation resolves `deployment_profile_name`; otherwise Beampipe uses the applicable default. Keep `max_concurrent_executions` low until the target has passed a load qualification.

## REST remote

```json
{
  "kind": "rest_remote",
  "dim_host_for_tm": "dlg-dim.internal",
  "dim_port_for_tm": 8001,
  "deploy_host": "dlg-dim.example.org",
  "deploy_port": 8001,
  "use_https": true,
  "verify_ssl": true
}
```

- `dim_host_for_tm` is the DIM address visible from Translator Manager.
- `deploy_host` is the address visible from Beampipe workers.
- Keep TLS verification enabled. Use trusted CA configuration instead of disabling it.
- Validate the graph application/runtime package versions, not only endpoint health.

```bash
beampipe doctor --profile local-daliuge
beampipe daliuge inspect --profile local-daliuge
beampipe daliuge sessions --profile local-daliuge
```

## Slurm remote

Required profile fields are `login_node`, `account`, absolute `home_dir`, `log_dir`, and `dlg_root`. Resource settings belong under `resources`; manager placement belongs under `manager_topology`.

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
  "modules": "module load singularity",
  "venv": "source /software/project/venv/bin/activate",
  "exec_prefix": "srun -l",
  "facility": "setonix",
  "resources": {
    "partition": "work",
    "nodes": 1,
    "tasks": 1,
    "cpus_per_task": 1,
    "memory": "32G",
    "wall_time_minutes": 60
  },
  "manager_topology": {
    "nodes": 1,
    "islands": 1,
    "co_host_dim": false
  }
}
```

`beampipe profile render PROFILE_NAME` shows effective `#SBATCH` directives and DALiuGE settings before submission.

## Preferred SSH key model

Keep the private key outside Beampipe and mount or expose it only to scheduler/worker processes.

=== "Bare metal"

    ```bash
    install -d -m 0700 "$HOME/.config/beampipe/credentials"
    install -m 0600 /path/to/dedicated_service_key \
      "$HOME/.config/beampipe/credentials/slurm_key"
    export SLURM_SSH_PRIVATE_KEY_FILE="$HOME/.config/beampipe/credentials/slurm_key"
    export SLURM_SSH_KNOWN_HOSTS_SOURCE="$HOME/.config/beampipe/credentials/known_hosts"
    ```

    Provision a dedicated key for the service instead of copying a personal login key. It should normally be owned by the Beampipe service user and mode `0600` or `0400`.

=== "Docker / Kubernetes"

    Mount the key read-only as a secret and point the process at its in-container path:

    ```bash
    SLURM_SSH_PRIVATE_KEY_FILE=/run/secrets/slurm_ssh_key
    SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE=/run/secrets/slurm_ssh_passphrase
    SLURM_SSH_KNOWN_HOSTS_SOURCE=/run/slurm-ssh/known_hosts
    ```

    The file may be root-owned when the Beampipe process can read it. The container does not create or own the key; the orchestrator mounts it.

=== "systemd credentials"

    Point the variables at files under `%d`/`$CREDENTIALS_DIRECTORY`. Keep credentials out of the unit environment and filesystem image.

Production rejects symlinked or non-regular key files, group/world permissions, owners other than the process user or root, empty secret files, and home-key fallback.

## Host-key trust

Obtain the public host key through a trusted facility channel. Use ordinary OpenSSH entries:

```text
login.hpc.example ssh-ed25519 AAAAC3...
[login.hpc.example]:2222 ssh-ed25519 AAAAC3...
```

Hashed entries are not supported. Verification is host- and port-aware; unrelated keys do not satisfy the check.

```bash
export BEAMPIPE_ENV=production
export BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS=true
export BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK=false
export BEAMPIPE_ALLOW_INLINE_SECRETS=false
export BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS=false

beampipe security check
beampipe doctor --profile setonix
beampipe slurm ping --profile setonix
beampipe scheduler status --profile setonix
```

Inline PEM, home-directory fallback, and disabled host-key checks are development or break-glass features. Do not make them normal deployment configuration.

## Slurm scale checks

Before raising concurrency, qualify one run and then a paced batch. Watch login-node SSH/SFTP pressure, remote filesystem growth, TM availability, profile caps, and poll duration. Polling is batched by target through pooled SSH sessions, but submission still stages files per execution.

The strongest unresolved risk from local testing is graph/runtime package compatibility. Pin DALiuGE and project application versions in the Slurm runtime and record them with each qualification result.

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

`beampipe setup` does not create a profile. Write a JSON file, then install and check it:

=== "REST remote"

    ```bash
    beampipe profile add -f config/deployment_profile.dlg-dim.json
    beampipe profile validate dlg-dim
    beampipe profile render dlg-dim
    ```

=== "Slurm remote"

    ```bash
    beampipe profile add -f config/deployment_profile.slurm-remote.json
    beampipe profile validate slurm-remote
    beampipe profile render slurm-remote
    ```

Place operator-owned copies in a private directory if you do not want to edit the examples in `config/`. File validation occurs during `profile add`. `profile validate` accepts an installed profile name.

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
  "ssh_credential": "setonix",
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

Keep private keys outside Beampipe. Profiles store only a non-secret `ssh_credential` slot name. The binary and Docker both resolve files from the same directory tree.

```text
$BEAMPIPE_SSH_CREDENTIALS_DIR/
  known_hosts                         # optional shared host keys
  setonix/
    private_key                       # or slurm_key
    passphrase                        # required when the private key is encrypted
    known_hosts                       # optional per-slot host keys
  garrawarla/
    private_key
```

Passphrase-locked keys are supported. Put the passphrase in a mode-`0600` file named `passphrase` (or `passcode`) next to the key. Dash never accepts the passphrase. Inline `SLURM_SSH_PRIVATE_KEY_PASSPHRASE` is a development fallback.

Default directory: `$HOME/.config/beampipe/credentials` on a host, or `/run/beampipe/ssh` when that path exists in a container. Set `BEAMPIPE_SSH_CREDENTIALS_DIR` to pin it.

A profile with `"ssh_credential": "setonix"` uses only that slot. It never falls back to another project's key. Omit the field to keep the process-wide `SLURM_SSH_PRIVATE_KEY_*` files.

Create the files with the built-in tool:

```bash
beampipe slurm credentials init --slot setonix --acl
beampipe slurm credentials check --slot setonix
beampipe slurm ping --profile slurm-remote
```

Setup does not create SSH slots. Run `beampipe slurm credentials init` when you add a Slurm profile.

=== "Bare metal"

    ```bash
    install -d -m 0700 "$HOME/.config/beampipe/credentials/setonix"
    ssh-keygen -t ed25519 -f "$HOME/.config/beampipe/credentials/setonix/private_key" \
      -C "beampipe-setonix@$(hostname)"
    # ssh-keygen prompts for a passphrase; write the same value to passphrase
    install -m 0600 /dev/stdin \
      "$HOME/.config/beampipe/credentials/setonix/passphrase" <<<'your-passphrase'
    ssh-keyscan -t ed25519 login.hpc.example \
      > "$HOME/.config/beampipe/credentials/known_hosts"
    export BEAMPIPE_SSH_CREDENTIALS_DIR="$HOME/.config/beampipe/credentials"
    export BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS=true
    ```

    Provision a dedicated key per facility or project instead of copying a personal login key. Files should be mode `0600` or `0400`.

=== "Docker / Kubernetes"

    Mount the same credentials tree and point the process at the in-container path:

    ```yaml
    environment:
      BEAMPIPE_SSH_CREDENTIALS_DIR: /run/beampipe/ssh
      BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS: "true"
    volumes:
      - type: bind
        source: ${BEAMPIPE_SSH_CREDENTIALS_HOST:-./deploy/ssh/credentials}
        target: /run/beampipe/ssh
        read_only: true
    ```

    The repository Compose files already mount this tree for `api`, `scheduler`, and `worker`.

    The container user is uid `10001`. Keep host files `0600` and grant that uid read access with ACLs, or expose each key as a Docker secret with `uid: "10001"` and `mode: 0400` and set `SLURM_SSH_PRIVATE_KEY_PATH_<SLOT>`.

=== "systemd credentials"

    Point `BEAMPIPE_SSH_CREDENTIALS_DIR` or `SLURM_SSH_PRIVATE_KEY_PATH_<SLOT>` at files under `%d`/`$CREDENTIALS_DIRECTORY`. Keep credentials out of the unit environment and filesystem image.

Per-slot env overrides use the slot name in uppercase with `-` and `.` replaced by `_`. Example: slot `setonix-pawsey0411` reads `SLURM_SSH_PRIVATE_KEY_PATH_SETONIX_PAWSEY0411`.

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

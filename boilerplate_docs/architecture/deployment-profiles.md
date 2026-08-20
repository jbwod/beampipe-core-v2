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

Setup may install a profile during the wizard or with `--profile-config`. You can also install one later:

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
  "name": "slurm-hpc",
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
  "login_node": "login.example.org",
  "ssh_port": 22,
  "remote_user": "operator",
  "ssh_credential": "hpc",
  "account": "project_account",
  "home_dir": "/scratch/project_account",
  "log_dir": "/scratch/project_account/operator/dlg/log",
  "dlg_root": "/scratch/project_account/operator/dlg",
  "modules": "module load singularity",
  "venv": "source /software/project/venv/bin/activate",
  "exec_prefix": "srun -l",
  "facility": "hpc",
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

Profiles store only a non-secret slot name. The slot is a directory under the credential root, not a hostname. Every installation has one deterministic credential root:

```text
$BEAMPIPE_HOME/credentials/ssh/
|-- known_hosts
`-- hpc/
    |-- private_key
    |-- private_key.pub
    |-- passphrase
    `-- known_hosts
```

The host runtime reads this tree directly. The release Compose bundle mounts the same absolute host path read-only at `/run/beampipe/ssh` in API, scheduler, and worker services. Keys are never copied into images or containers.

`GET /api/v2/slurm/credentials` (and Dash's profile picker) lists slot names and whether `private_key`, `private_key.pub`, `passphrase`, and `known_hosts` files are present. The responses never include key material. Init, import, and `copy-id` remain CLI.

Beampipe does not use `ssh-agent`. Workers unlock `private_key` plus an optional `passphrase` file. Choose **one** path: generate a Beampipe-owned key, or import a key you already have. Do not run `ssh-keygen` and `init` for the same slot.

### Generate a Beampipe-owned key

`init` creates the Ed25519 key. You still must install `private_key.pub` on the login node before SSH will work. `--copy-id` (or the later `copy-id` command) is that install step when password SSH still works; otherwise use the site's key-registration process.

```bash
beampipe slurm credentials init \
  --slot hpc \
  --host login.example.org \
  --user alice \
  --port 22 \
  --acl
beampipe slurm credentials copy-id \
  --slot hpc \
  --user alice \
  --host login.example.org
```

Generation occurs inside Beampipe, so the passphrase is not exposed in an `ssh-keygen` process argument. Interactive `init` can prompt to run `ssh-copy-id`; `--yes` does not prompt.

### Import an existing key

Use this when the key already exists (including a key created with `ssh-keygen`). The source key is not modified. Skip uploading the public key if the cluster already has it in `~/.ssh/authorized_keys`. Prefer a facility-verified `known_hosts` file.

```bash
beampipe profile add \
  -f "$HOME/beampipe/config/deployment_profile.slurm-remote.json" \
  --ssh-slot hpc \
  --ssh-private-key "$HOME/.ssh/id_ed25519" \
  --ssh-known-hosts "$HOME/.ssh/known_hosts" \
  --ssh-acl
```

Or manage the slot separately:

```bash
beampipe slurm credentials import \
  --slot hpc \
  --private-key "$HOME/.ssh/id_ed25519" \
  --known-hosts "$HOME/.ssh/known_hosts" \
  --acl
beampipe slurm credentials sync --slot hpc
beampipe slurm credentials check --slot hpc --profile slurm-remote
```

For an encrypted key, pass `--passphrase-file` pointing to a protected file. Secret text is never accepted as a CLI argument.

### Installing the public key

`ssh-copy-id` and a manual append both keep existing `authorized_keys` entries when you use `>>`. Some sites require a portal or helpdesk process instead.

```bash
# password SSH still works
beampipe slurm credentials copy-id --slot hpc --user alice --host login.example.org

# or append manually (use >> so existing keys are kept)
cat "$BEAMPIPE_HOME/credentials/ssh/hpc/private_key.pub" \
  | ssh alice@login.example.org "mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys"
```

Pawsey-style login nodes follow the same `authorized_keys` pattern. See [Use of SSH Keys for Authentication](https://pawsey.atlassian.net/wiki/spaces/US/pages/51925870/Use+of+SSH+Keys+for+Authentication). That guide also covers `ssh-agent` for interactive laptop SSH; Beampipe workers do not use the agent.

### Permissions by runtime

| Runtime | Expected ownership and access |
|---|---|
| Native host | Service-user or root-owned regular file, mode `0600` or `0400` |
| Linux Docker | Same private mode plus narrow read/traverse ACLs for container uid `10001`; `--acl` applies them |
| Docker Desktop | Private host copy under the installation; `sync` performs the authoritative in-container readability check |
| Kubernetes/systemd | Mount a secret/credential file and point the slot resolver at the mounted credential root |

`beampipe slurm credentials sync` does not use `docker cp`. It verifies the recorded bind source and, when services are running, executes a read test as the actual scheduler and worker container user.

Production rejects symlinks, non-regular private keys, group/world permissions, unsupported ownership, empty passphrase files, home-key fallback, and inline key material.

## Host-key trust

Obtain the public host key through a trusted facility channel. Use ordinary OpenSSH entries:

```text
login.hpc.example ssh-ed25519 AAAAC3...
[login.hpc.example]:2222 ssh-ed25519 AAAAC3...
```

Hashed entries are not supported. Verification is host- and port-aware; unrelated keys do not satisfy the check.

When no file is supplied, the interactive credential command can run `ssh-keyscan`, print SHA-256 fingerprints, and ask for confirmation. Verify those fingerprints through the facility before accepting them. Non-interactive scanning requires the explicit `--accept-host-key` acknowledgement.

```bash
export BEAMPIPE_ENV=production
export BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS=true
export BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK=false
export BEAMPIPE_ALLOW_INLINE_SECRETS=false
export BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS=false

beampipe security check
beampipe doctor --profile PROFILE_NAME
beampipe slurm ping --profile PROFILE_NAME
beampipe scheduler status --profile PROFILE_NAME
```

Inline PEM, home-directory fallback, and disabled host-key checks are development or break-glass features. Do not make them normal deployment configuration.

## Slurm scale checks

Before raising concurrency, qualify one run and then a paced batch. Watch login-node SSH/SFTP pressure, remote filesystem growth, TM availability, profile caps, and poll duration. Polling is batched by target through pooled SSH sessions, but submission still stages files per execution.

The strongest unresolved risk from local testing is graph/runtime package compatibility. Pin DALiuGE and project application versions in the Slurm runtime and record them with each qualification result.

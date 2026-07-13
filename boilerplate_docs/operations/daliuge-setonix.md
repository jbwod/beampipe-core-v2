# DALiuGE and Setonix

Beampipe is an external control plane. It prepares immutable manifests and graphs,
requests logical-to-physical translation from DALiuGE, and either deploys to an existing
Data Island Manager or submits DALiuGE inside a SLURM allocation. DALiuGE continues to
own graph execution and Node Manager behavior.

## Configure with the wizard

For an existing Data Island Manager:

```bash
beampipe setup \
  --deployment rest_remote \
  --profile-name local-daliuge \
  --tm-url https://translator.example.org \
  --dim-url https://manager.example.org:8001
```

TLS certificate verification defaults to enabled. For Setonix:

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

The wizard writes endpoints and non-secret profile settings. SSH keys, passphrases,
CASDA credentials, and tokens remain environment or mounted-file secrets.

## Trust and credentials

```bash
export BEAMPIPE_ENV=production
export SLURM_SSH_PRIVATE_KEY_FILE=/run/secrets/slurm_ssh_key
export SLURM_SSH_KNOWN_HOSTS_SOURCE=/run/slurm-ssh/known_hosts
export BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS=true
```

Populate `known_hosts` from a trusted facility channel. Beampipe validates the selected
host and port and does not disable host-key verification by default.

## Preflight

```bash
beampipe profile validate setonix
beampipe profile render setonix
beampipe doctor --profile setonix
beampipe profile test setonix
beampipe scheduler status --profile setonix
beampipe daliuge inspect --profile setonix
```

`profile render` shows the computed resource request and `#SBATCH` directives before a
submission. Confirm account, partition, nodes, tasks, CPUs, memory, wall time,
constraint, QoS, modules, container runtime, environment setup, partition count, and
manager topology with the facility-approved WALLABY configuration.

## Dry-run graph preparation

After discovery metadata exists for a source:

```bash
beampipe graph prepare \
  --project wallaby_hires \
  --source HIPASSJ1313-15
```

The result contains project revision/hash, manifest checksum, source graph checksum,
patched graph checksum, and a node/field patch summary. Missing patch nodes or fields
are hard failures. No DALiuGE session or scheduler job is created.

For an existing execution, translate the persisted patched graph without deployment:

```bash
beampipe daliuge translate --execution "$EXECUTION_ID"
beampipe graph diff --execution "$EXECUTION_ID"
```

## Live inspection

```bash
beampipe daliuge sessions --profile setonix
beampipe daliuge session-inspect SESSION_ID --profile setonix
beampipe scheduler jobs --limit 100
```

The typed DALiuGE layer normalizes health, compatibility, translation, session creation,
deployment, inspection, cancellation, timeout, and retry classification. The exact
verified upstream contract and supported-version risk are recorded in the
[operator overhaul assessment](../architecture/operator-overhaul-assessment.md).

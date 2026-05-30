# Installation

## Clone the repository

```bash
git clone https://github.com/jbwod/beampipe-core.git
cd beampipe-core
```

## Run the setup wizard

```bash
python setup.py
```

The wizard lets you pick a deployment template, copies compose / Dockerfile / nginx / Restate config to the repo root, and generates a fresh **root `.env`** with secrets.

### Templates

| Template | Use when |
|----------|----------|
| **local** | Slim local dev: single instance of each service (`scripts/local_dev/`) |
| **production** | Full HA stack: nginx LB, 2 web, 3 worker, 3 Restate, 2 beamcore_rs (`scripts/production/`) |
| **custom** | Pick replica counts for web / worker / Restate / beamcore_rs (`scripts/custom_base/`) |

The wizard prompts for:

- Admin user (name, email, username, password)
- App contact metadata
- Optional CASDA credentials
- Optional Restate S3 snapshot credentials (production)
- Optional Slurm SSH key generation (production/custom)

!!! tip
    For local development, select **local** and let the wizard run `make dev` at the end if offered.

!!! note
    Re-run `python setup.py` any time you change deployment shape. The wizard prompts before overwriting existing files.

Templates live in [`scripts/local_dev`](https://github.com/jbwod/beampipe-core/tree/main/scripts/local_dev), [`scripts/production`](https://github.com/jbwod/beampipe-core/tree/main/scripts/production), and [`scripts/custom_base`](https://github.com/jbwod/beampipe-core/tree/main/scripts/custom_base).

## Python dependencies (optional)

For tests and OpenAPI export:

```bash
uv sync
```

With the reference survey module (optional extra):

```bash
uv sync --extra wallaby
```

## Slurm SSH (production/custom)

For **production** and **custom**, the wizard can generate SSH keys at `./deploy/ssh/id_slurm` via `ssh-keygen -t ed25519` and sync `./deploy/ssh/known_hosts` if missing. Existing keys are never overwritten.

1. Add `deploy/ssh/id_slurm.pub` to the HPC login node `authorized_keys`.
2. Sync known hosts: `make slurm-known-hosts-sync`

```bash
cat ./deploy/ssh/id_slurm.pub   # add to ~/.ssh/authorized_keys on the head node
make beampipe-start             # syncs known_hosts, brings compose up (when Slurm wired)
```

The canonical `.env` is written to the **repo root** (next to `docker-compose.yml`).

## Verify

=== "Local"

    ```bash
    make dev
    make beampipe-new-admin   # if admin not created
    ```

=== "Production / custom"

    ```bash
    make beampipe-start
    make beampipe-new-admin
    make urls
    ```

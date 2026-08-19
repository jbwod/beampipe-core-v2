# Install and configure

Beampipe has one installation directory and one management command. PostgreSQL is required; Docker is recommended but not mandatory.

```text
$BEAMPIPE_HOME/                  default: ~/beampipe
|-- installation.json           runtime and bundle identity, no secrets
|-- .env                        private runtime configuration, mode 0600
|-- docker-compose.yml          version-managed operator bundle
|-- config/                     project and profile examples
|-- credentials/casda/password  CASDA staging password, mode 0600
`-- credentials/ssh/<slot>/     managed SSH credential copies
```

The active installation is selected by global `--home`, then `BEAMPIPE_HOME`, then `~/beampipe`. The current directory does not select an installation.

## 1. Docker: recommended

Use this path for a workstation or a single-host service. It downloads the release binary and published container image; no repository clone or Rust toolchain is needed.

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

Choose Docker in the wizard. Setup creates a random JWT secret and PostgreSQL password, binds PostgreSQL/API/metrics to loopback (API host port `18080` by default), migrates the database, creates the first administrator, and uploads the reference project.

The installer writes `~/.local/bin/beampipe` and appends that directory to `~/.bashrc` and `~/.profile`. The current terminal still needs `export PATH="$HOME/.local/bin:$PATH"` (or a new terminal) before `beampipe` is found.

Unattended equivalent:

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh \
  | sh -s -- --yes --runtime docker --postgres compose \
      --api-port 18080 --postgres-port 5432 --metrics-port 9090
```

`--yes` skips the Next actions prompt and prints the recipe instead: add a REST or Slurm profile, run `beampipe doctor --profile NAME`, set CASDA credentials for staging, then set `BEAMPIPE_USE_REAL_BACKENDS=true` in the install `.env` and `beampipe restart`. Pass `--use-real-backends` only after that profile doctor is known to pass. Interactive setup offers those steps after the stack is up (live backends, profile file, Slurm SSH credentials, CASDA credentials, profile doctor).

Manage it from any directory:

```bash
beampipe status
beampipe doctor
beampipe logs --follow
beampipe restart
beampipe stop
beampipe start
beampipe uninstall
```

`beampipe uninstall` stops Compose services, deletes the installation directory, and by default removes managed PostgreSQL volumes. Confirmation is required unless `--yes` is passed. `--keep-volumes` retains Compose volumes. `--purge-binary` also removes `~/.local/bin/beampipe`. Sibling checkouts such as `~/beampipe-dash` are not deleted.

Use an existing PostgreSQL server instead:

```bash
beampipe setup --yes --runtime docker --postgres existing \
  --database-url 'postgres://beampipe@database.internal/beampipe'
```

The database hostname must be reachable from both the host setup command and Docker containers. `localhost` inside a container is the container itself.

## 2. Native host

Use this path when Beampipe processes should run directly under the service user. PostgreSQL may be an existing service or the installation's Compose PostgreSQL only.

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh \
  | sh -s -- --yes --runtime host --postgres existing \
      --database-url 'postgres://beampipe@127.0.0.1/beampipe'

beampipe start
```

For Compose PostgreSQL with a native Beampipe process:

```bash
beampipe setup --yes --runtime host --postgres compose --no-start
beampipe start
```

`beampipe start` starts the managed PostgreSQL container when required, then runs the compact API/scheduler process in the foreground. Production native deployments should run separate API, singleton scheduler, and worker units under systemd or another process supervisor; see [Deployment topologies](deployment.md).

## 3. Build from source

Use this path for development and commit qualification.

```bash
git clone https://github.com/jbwod/beampipe-core-v2.git
cd beampipe-core-v2
cargo build --locked --release -p beampipe-cli --bin beampipe
export PATH="$PWD/target/release:$PATH"
```

Native developer installation:

```bash
beampipe --home "$PWD/.local-install" setup \
  --yes --runtime host --postgres existing --no-start \
  --database-url 'postgres://postgres:postgres@127.0.0.1/beampipe'
```

Source-built Docker stack:

```bash
BEAMPIPE_BUILD=1 ./deploy/setup-docker.sh --yes --skip-admin --skip-upload
```

The checkout Compose file may build local images and includes developer tooling. Ordinary release installations use the embedded pull-only operator bundle.

## Add a deployment profile

Setup can install a profile immediately:

```bash
beampipe setup --profile-config config/deployment_profile.dlg-dim.json
```

Or add one later:

```bash
beampipe profile add -f "$HOME/beampipe/config/deployment_profile.dlg-dim.json"
beampipe profile validate dlg-dim
beampipe doctor --profile dlg-dim
```

Import and associate a Slurm key in the same operation (skip the public-key upload if the cluster already has this key):

```bash
beampipe profile add \
  -f "$HOME/beampipe/config/deployment_profile.slurm-remote.json" \
  --ssh-slot hpc \
  --ssh-private-key "$HOME/.ssh/id_ed25519" \
  --ssh-known-hosts "$HOME/.ssh/known_hosts" \
  --ssh-acl

beampipe slurm credentials sync --slot hpc
beampipe doctor --profile slurm-remote
```

To generate a new Beampipe-owned key instead, run `beampipe slurm credentials init --slot hpc --host LOGIN_NODE` and then install `private_key.pub` with `copy-id` or the site's key-registration process. The source key is never modified on import. Beampipe stores a private managed copy under the selected installation and mounts the credential root read-only into Docker services. See [Deployment profiles and SSH](../architecture/deployment-profiles.md).

## Upgrade and rerun setup

`beampipe setup` is idempotent. It preserves existing JWT/database secrets, profiles, project revisions, SSH slots, and data volumes. Unmodified generated bundle files are upgraded; operator-edited files are retained and reported.

```bash
beampipe setup
beampipe doctor
```

There is no implicit reset. Back up PostgreSQL before `beampipe uninstall` or before deleting a Compose volume.
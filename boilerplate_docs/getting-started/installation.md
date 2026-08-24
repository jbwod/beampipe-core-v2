# Install and configure

Beampipe has one installation directory and one management command. PostgreSQL is required; Docker is recommended but not mandatory. For a first installation, use the guided Docker wizard.

| Path | Best for | What you provide |
|---|---|---|
| Guided Docker | First evaluation or a single-host service | Docker Compose v2 and free local ports |
| Unattended Docker | Repeatable automation | Explicit `--yes`, runtime, and database mode |
| Native host | Supervised services or a host-only environment | PostgreSQL and a process supervisor |
| Source build | Development and commit qualification | Rust toolchain and this repository |

The [interactive command builder](../index.md#install-builder) creates a copyable command without putting passwords into it.

```text
$BEAMPIPE_HOME/                  default: ~/beampipe
|-- installation.json           runtime and bundle identity, no secrets
|-- .env                        private runtime configuration, mode 0600
|-- docker-compose.yml          version-managed operator bundle
|-- config/                     project/profile files selected by the operator
|-- credentials/                provider credentials, when configured
`-- credentials/ssh/<slot>/     managed SSH credential copies
```

The active installation is selected by global `--home`, then `BEAMPIPE_HOME`, then `~/beampipe`. The current directory does not select an installation. Setup never stores secrets in `installation.json`.

## 1. Docker: recommended

Use this path for a workstation or a single-host service. It downloads the release binary and published container image; no repository clone or Rust toolchain is needed.

### Guided setup

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

The wrapper checks local installer tools, selects the release for your platform, verifies its SHA-256 checksum, installs `beampipe`, and hands the terminal to the setup wizard. Choose Docker and managed PostgreSQL for the shortest path.

The wizard walks through **Runtime → PostgreSQL → Network → optional Dash → Project and deployment → Review**. It confirms the install home at the beginning and the complete plan before configuration starts. Setup then creates a random JWT secret and PostgreSQL password, binds PostgreSQL/API/metrics to loopback (API host port `18080` by default), migrates the database, creates the first administrator, and starts the selected services. No scientific project, provider integration, or real execution backend is enabled implicitly.

When setup finishes, open a new terminal or update this one, then verify it:

```bash
export PATH="$HOME/.local/bin:$PATH"
beampipe status
beampipe doctor
```

If setup stops, the verified binary remains installed and the installer prints a safely quoted resume command. `beampipe setup` is idempotent; fix the reported issue and rerun that command.

### Fresh database and migration ownership

In the managed Compose topology, the API is the migration gate. It starts with
`BEAMPIPE_MIGRATE_ON_SERVE=true`, applies every pending SQLx migration, and
becomes healthy only afterwards. Scheduler and worker services depend on that
health check. Do not race fresh setup with a separate `beampipe migrate` or
start workers against an unmigrated database.

For native or externally supervised roles, run `beampipe migrate` once as a
deployment step before starting API, scheduler, and workers unless the API role
is explicitly designated as migration owner. Back up an existing database
before upgrading across migrations.

The installer writes `~/.local/bin/beampipe` and appends that directory to the applicable login and interactive shell files. The current terminal still needs `export PATH="$HOME/.local/bin:$PATH"` (or a new terminal) before `beampipe` is found.

### Unattended setup

Headless setup never guesses operator intent. A process without a terminal must pass both `--yes` and an explicit runtime; otherwise the installer exits before downloading anything and prints the exact Docker command. A complete managed-Docker invocation is:

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh \
  | sh -s -- --yes --runtime docker --postgres compose \
      --api-port 18080 --postgres-port 5432 --metrics-port 9090
```

`--yes` answers setup questions from explicit flags or safe defaults. The CLI remains the single source of the resulting **SETUP COMPLETE → ACCESS → NEXT ACTIONS** handoff. Pass `--use-real-backends` only after `beampipe doctor --profile NAME` is known to pass.

For unattended administrator creation, omit a password to generate one or use a mode-`0600` file:

```bash
beampipe setup --yes --runtime docker --postgres compose \
  --admin-user operator --admin-email operator@example.test \
  --admin-password-file /run/secrets/beampipe-admin
```

Do not pass `--admin-password` in shell history, CI logs, or a copied installer command. When unattended setup generates the password, it writes it to `$BEAMPIPE_HOME/credentials/admin/password` with mode `0600` and does not print it to standard output.

Supply any project during setup with `--project-config PATH`. Omitting it produces a project-neutral installation. The bundled WALLABY HiRes example is opt-in:

```bash
beampipe setup --yes --runtime docker --postgres compose --sample wallaby-hires
```

That sample materializes its project configs, graphs, and REST/Slurm profiles and declares `staging:casda_uws`. Setup validates whichever project was selected with `--project-config` or `--sample`, then adds the capability required by its explicit staging provider to both backend and worker routing. `casda_uws` adds `staging:casda_uws`; `none` adds nothing, and existing capability settings are preserved without duplicates. Selecting a Slurm profile similarly adds `deployment:slurm_remote`; choosing REST adds `deployment:daliuge_rest`. Core requires SSH or CASDA credentials only when the corresponding capability is declared and real backends are enabled. Projects added after setup should explicitly configure any provider capability they require.

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

Production API startup requires a reachable Redis service configured through
`BEAMPIPE_REDIS_URL`. Setting `BEAMPIPE_REQUIRE_RATE_LIMITER=false` does not
disable that production requirement. Development may omit Redis; see
[API rate limiting and proxy trust](../operations/index.md#api-rate-limiting-and-proxy-trust).

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

`beampipe start` starts the managed PostgreSQL container when required, then runs the compact API/scheduler process in the foreground. Production native deployments should run separate API, singleton scheduler, and worker units under systemd or another process supervisor; see [Process roles](../operations/index.md).

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

# Install and configure

Beampipe ships as one Rust binary. Prefer a released binary on `PATH`; use a source build when qualifying a commit. PostgreSQL is always required.

## Install the binary

=== "Release"

    Download the archive for your platform, verify it against `SHA256SUMS`, and place `beampipe` on `PATH`.

    ```bash
    beampipe --version
    beampipe setup --yes --runtime host --skip-admin --skip-upload
    ```

=== "Build from source"

    ```bash
    git clone https://github.com/jbwod/beampipe-core-v2.git
    cd beampipe-core-v2
    cargo build --locked --release -p beampipe-cli --bin beampipe
    export PATH="$PWD/target/release:$PATH"
    beampipe setup --yes --runtime host --skip-admin --skip-upload
    ```

=== "Docker Compose"

    Compose builds the same binary into one image and assigns it API, scheduler, and worker roles.

    ```bash
    ./deploy/setup-docker.sh --yes --skip-admin --skip-upload
    docker compose up -d postgres
    docker compose run --rm api migrate
    docker compose run --rm api admin create-user \
      --username admin \
      --password 'replace-this-local-password' \
      --email admin@example.test \
      --superuser
    docker compose run --rm api project add -f config/wallaby_hires.v2.yaml
    docker compose up -d api scheduler worker
    ```

## Configuration precedence

Settings resolve in this order, with later sources winning:

<div class="bp-flow-diagram bp-flow-diagram--animated" role="img" aria-label="Configuration precedence from defaults to environment variables">
  <div class="bp-flow-node" data-tone="cyan"><span>LOW</span><strong>defaults</strong><small>safe development</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>FILE</span><strong>beampipe.yaml</strong><small>non-secret settings</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>LOCAL</span><strong>.env</strong><small>private runtime</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>HIGH</span><strong>environment</strong><small>deployment override</small></div>
</div>

Inspect the effective value and its source without exposing secrets:

```bash
beampipe config explain
```

Start from `.env.example`. The minimum host configuration is:

```bash
BEAMPIPE_ENV=development
DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
BEAMPIPE_JWT_SECRET=replace-with-at-least-32-random-characters
BEAMPIPE_USE_REAL_BACKENDS=false
```

## Process layout

For evaluation, one process is enough:

```bash
beampipe start
```

For production, separate the roles:

```bash
beampipe serve --worker false
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false BEAMPIPE_WORKER_CONCURRENCY=4 beampipe worker
```

Run exactly one scheduler-enabled process. API and worker-only processes can scale independently.

## Essential settings

| Area | Settings | Guidance |
|---|---|---|
| API | `BEAMPIPE_BIND_ADDR`, token lifetimes, CORS | Bind to loopback until ingress and TLS are configured |
| Workers | concurrency, lock seconds, pool, capabilities | Keep the lease longer than normal external calls |
| Admission | `BEAMPIPE_SHAPING_*` | Cluster-wide safety limits; project YAML adds survey limits |
| Metrics | `BEAMPIPE_METRICS_BIND_ADDR`, OTEL settings | Give each host process a unique metrics port |
| Real backends | `BEAMPIPE_USE_REAL_BACKENDS` | Enable only after profile-specific doctor checks pass |
| Secrets | mounted files or environment references | Never put credentials in project YAML or deployment profiles |

Use `beampipe config explain` for the complete release-specific list. Environment names and defaults are also documented in `.env.example` and `.env.template`.

## Choose a deployment layout

Installation places the same Beampipe binary into one or more runtime roles.
Where those roles run is an operator choice; REST/DIM and Slurm are execution
backends, not alternative Beampipe binaries.

| Layout | Best for | Process shape |
|---|---|---|
| Native compact | laptop evaluation | one `beampipe start` process |
| Docker Compose | local service or single host | API + one scheduler + worker replicas + PostgreSQL |
| Native services | managed VM or bare metal | separate systemd/supervisor units |
| Container platform | multi-host production | separately scaled API, scheduler, and worker workloads |

Continue with [Deployment topologies](deployment.md) for complete examples and
network diagrams.

## Production gates

```bash
BEAMPIPE_ENV=production beampipe security check
beampipe doctor --json
```

Production rejects weak JWT configuration, default database credentials, unsafe inline secrets, permissive SSH host-key policy, and other development defaults. See [Deployment profiles and SSH](../architecture/deployment-profiles.md) for backend credentials and [Production runbook](../operations/production-runbook.md) for rollout order.

# Quick start

This path gives you a real API, PostgreSQL ledger, scheduler, workers, and terminal console. External execution clients are mocked. TAP discovery becomes live only when you register and trigger a source in the [first workflow](first-run.md).

## 1. Prerequisites

- Rust stable, Docker Compose, `curl`, and `jq`.
- This repository checkout.
- Ports `5432`, `8080`, and `9090` available.

Build the single `beampipe` binary:

```bash
cargo build --locked --release -p beampipe-cli --bin beampipe
export PATH="$PWD/target/release:$PATH"
beampipe --version
```

## 2. Bootstrap

From the repository root:

```bash
docker compose up -d postgres
beampipe init --directory operator-local
cd operator-local

beampipe setup --yes \
  --admin-password 'replace-this-local-password' \
  --project-config ../config/wallaby_hires.v2.yaml \
  --profile-name slurm-remote
```

The final option makes the local mock profile match the profile name referenced by the WALLABY automation policy. Its backend kind remains `rest_remote`; replace it with a real profile before enabling real backends.

`setup` writes a mode-`0600` `.env` on Unix, generates a JWT secret, applies migrations, creates the administrator, installs the profile, uploads the project config, and runs diagnostics.

## 3. Start

```bash
beampipe doctor
beampipe start
```

Open a second terminal in `operator-local`:

```bash
beampipe status
beampipe worker list
beampipe console
```

The API is now available at `http://127.0.0.1:8080/api/v2`.

<div class="bp-flow-diagram bp-flow-diagram--animated" role="img" aria-label="Local setup flow from PostgreSQL through setup checks to running API and workers">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>PostgreSQL</strong><small>docker compose</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02</span><strong>setup</strong><small>migrate + seed</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03</span><strong>doctor</strong><small>fail before start</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04</span><strong>start</strong><small>API + worker</small></div>
</div>

## 4. Verify

```bash
curl -fsS http://127.0.0.1:8080/api/v2/health | jq .
curl -fsS http://127.0.0.1:9090/health
beampipe status
```

Expected: health is `ok`; `beampipe status` reports PostgreSQL and workers explicitly. The detailed `/api/v2/ready` endpoint requires an authenticated bearer token. A configured but unreachable external dependency may remain degraded until you connect that backend.

## What this proves

| Proven | Not yet proven |
|---|---|
| Binary, configuration, migrations, authentication | Live archive discovery |
| PostgreSQL jobs, scheduler, workers, console | CASDA staging |
| Mock execution and durable ledger transitions | TM/DIM or Slurm connectivity |
| Metrics endpoint | Scientific output correctness |

Continue with [First workflow](first-run.md) for live public TAP discovery and a no-submit graph preparation, or [Deployment profiles and SSH](../architecture/deployment-profiles.md) to connect real infrastructure.

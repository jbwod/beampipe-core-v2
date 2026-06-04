# Production runbook

Use this checklist when promoting beampipe-core into an operator-managed environment.

## Preflight

| Check | Command |
|-------|---------|
| Build image | `docker compose build api` |
| Apply migrations | `docker compose run --rm api migrate` |
| Create admin | `docker compose run --rm api admin create-user --username admin --password ... --email ...` |
| Validate config | `beampipe project validate -f config/wallaby_hires.v1.yaml` |
| Upload config | `POST /api/v2/project-configs` |
| Check API | `GET /api/v2/health`, `GET /api/v2/ready` |
| Check metrics | `GET /api/v2/metrics`, `:9090/metrics` |

## Backend preflight

<div class="terminal-diagram">
<pre>CASDA TAP ----\
VizieR TAP ----+--> beampipe worker --> Translator Manager --> DIM or Slurm
Postgres  ----/              |                 |                 |
                             v                 v                 v
                       provenance        translated graph    run status</pre>
</div>

Before setting `BEAMPIPE_USE_REAL_BACKENDS=true`, confirm:

| Backend | Check |
|---------|-------|
| CASDA | credentials, TAP availability, staging account access |
| VizieR | TAP availability from worker network |
| Translator Manager | `tm_url` reachable from workers |
| DIM REST | deploy host/port reachable and status endpoints readable |
| Slurm | SSH key, known hosts, login node, account, DALiuGE install path |

## Rollout sequence

1. Start Postgres and apply migrations.
2. Start API with `beampipe serve --worker false`.
3. Start one scheduler-enabled worker process.
4. Start worker-only replicas with `BEAMPIPE_WORKER_SCHEDULER_ENABLED=false`.
5. Upload project config and deployment profiles.
6. Register a small source batch and run discovery.
7. Execute one dry-run with `do_stage=false` and `do_submit=false`.
8. Enable real backends and submit one source.
9. Scale workers and source batch size only after metrics are stable.

## Incident checks

| Issue | Action |
|-------|--------|
| Queue growth | Reduce source registration rate, check worker logs, inspect queue depth in `/ready` |
| Duplicate scheduler ticks | Ensure only one scheduler-enabled process is running |
| TAP failures | Check dependency gauges and `GET /api/v2/health/tap` |
| Staging failures | Confirm CASDA credentials and source metadata readiness |
| Slurm polling failures | Check `SLURM_SSH_KNOWN_HOSTS_SOURCE`, key permissions, and login host reachability |
| DIM errors | Read execution debug URLs and graph/drop status |

## Python migration note

The Rust v2 docs do not preserve Python setup instructions. If comparing a legacy run during cutover, use ledger snapshots only as a temporary validation aid, then operate the Rust API and `/api/v2` contract as the source of truth.

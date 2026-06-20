# Production runbook

Use this page for promotion, incident response, and config change control. It assumes you already understand the process model from [Operator guide](operator-guide.md).

## Promotion checklist

| Stage | Check | Evidence |
|-------|-------|----------|
| Build | Image or binary comes from intended commit | release artifact, image tag, or `beampipe --version` |
| Database | Migrations have run once | `beampipe migrate` exits cleanly |
| Security | Production gates pass | `beampipe security check` |
| Identity | First operator exists | Admin login succeeds |
| Config | Project YAML validates | `beampipe project validate -f config/wallaby_hires.v2.yaml` |
| Profiles | Deployment profiles validate and upload | profile API response |
| API | Health and readiness are green | `GET /api/v2/health`, `GET /api/v2/ready` |
| Metrics | API and workers are scraped | Prometheus targets up |
| Backend | TAP, 流 Translator Manager, 流 DIM, or Slurm reachable | dependency checks, `beampipe slurm ping --profile <name>` |

## Backend preflight

<div class="terminal-diagram">
<pre>CASDA TAP ----\
VizieR TAP ----+--> worker --> metadata --> manifest --> 流 TM --> 流 DIM/Slurm
Postgres  ----/        |          |           |          |       |
                       v          v           v          v       v
                    events     signature    graph     deploy   poll</pre>
</div>

| Backend | Required before real runs |
|---------|---------------------------|
| CASDA | Credentials, TAP endpoint, staging account, expected collections |
| VizieR | TAP endpoint reachable from workers |
| 流 Translator Manager | `tm_url` reachable and compatible with the graph format |
| 流 DIM REST | Deploy host/port reachable; status URLs readable |
| Slurm | SSH key, known hosts, login node, account/partition, DALiuGE install path |

Keep `BEAMPIPE_USE_REAL_BACKENDS=false` until project config validation, manifest creation, and dry execution are proven.

## Rollout sequence

1. Start PostgreSQL.
2. Apply migrations.
3. Start the API role.
4. Start exactly one scheduler role.
5. Start worker-only replicas.
6. Upload project config and deployment profiles.
7. Register a small source batch.
8. Run discovery and inspect metadata/signatures.
9. Queue a dry execution with `do_stage=false` and `do_submit=false`.
10. Enable real backend work and submit one source.
11. Scale source count, worker count, and batch limits gradually while watching metrics.

## Incident decision tree

| Symptom | Immediate action | Follow-up |
|---------|------------------|-----------|
| API not ready | Check database connectivity, migrations, production security gates | Keep workers running only if queue recovery is desired |
| Queue depth rising | Pause source registration or scheduler, inspect oldest queued job and worker logs | Add workers only if dependencies are healthy |
| Duplicate scheduler ticks | Stop extra scheduler-enabled processes | Confirm one scheduler role |
| Discovery failures | Check TAP dependency health and query templates | Reduce discovery batch size if TAP latency is high |
| Metadata changes every run | Inspect signature exclusions and volatile TAP columns | Exclude URLs, sizes, and timestamps that should not trigger reruns |
| Execution stuck pending | Check metadata readiness, automation caps, queue depth | Confirm deployment profile name matches project config |
| Slurm polling failures | Check known hosts, key permissions, login node reachability | Run `beampipe slurm ping --profile <name>` |
| 流 DIM errors | Read execution debug URLs and provenance events | Verify 流 Translator Manager output and 流 DIM endpoint |
| Alerts silent | Send test notification | Check secret references and production redaction rules |

## Change control

| Change | Expected impact |
|--------|-----------------|
| Field maps | Prepared metadata shape and signatures may change |
| Discovery flags | Readiness gates and manifest values may change |
| Signatures | Skip/re-run behavior changes |
| Transform definitions | Query variables, metadata, and flags may change |
| Manifest grouping | Execution grouping changes |
| DALiuGE Graphs | Translated graph shape changes |
| Automation caps | Scheduler admission behavior changes |
| Deployment profile | Backend routing, staging, and polling behavior changes |

Validate YAML, upload a new config revision, and run a small discovery/execution sample after any change in this table.

## Cutover note

The Rust v2 docs do not preserve legacy stack setup instructions. During cutover, compare old and new ledgers only as temporary validation evidence. Once Rust v2 is live, operate `/api/v2`, project config YAML, and deployment profiles as the source of truth.

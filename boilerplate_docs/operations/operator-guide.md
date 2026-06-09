# Operator guide

This is the daily operating map for beampipe-core v2. Use it after installation to understand which process should run, what it owns, and where to look when work stalls.

## Operating model

beampipe-core is one binary with multiple roles. All roles share PostgreSQL as the durable state store.

<div class="terminal-diagram">
<pre>operator / API
      |
      v
+---------------+      +----------------+      +------------------+
| API process   | ---> | PostgreSQL     | <--- | worker replicas  |
| /api/v2       |      | configs/jobs   |      | claim/run jobs   |
| auth/uploads  |      | events/ledger  |      | TAP/TM/DIM/Slurm |
+-------+-------+      +-------+--------+      +--------+---------+
        |                      ^                        ^
        |                      |                        |
        +--------------+-------+------------------------+
                       |
                 scheduler role
                 enqueue ticks</pre>
</div>

Run exactly one scheduler-enabled process per environment. Scale API processes for HTTP traffic and worker-only processes for queue throughput.

## Role contract

| Role | Starts with | Owns | Scale rule |
|------|-------------|------|------------|
| API | `beampipe serve --worker false` | HTTP API, auth, uploads, readiness, metrics | Scale for request volume |
| Scheduler | `beampipe serve --worker true` or scheduler-enabled `beampipe worker` | Recurring discovery, execution, DIM, and Slurm poll ticks | Run exactly one |
| Worker | `BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker` | Queue claims, TAP calls, manifests, staging, translation, deployment, polling | Scale horizontally |
| Database | PostgreSQL | Configs, source metadata, jobs, executions, provenance | One logical primary |

## Operator workflow

| Phase | Action | Command or API |
|-------|--------|----------------|
| Bootstrap | Apply migrations | `beampipe migrate` |
| Bootstrap | Create first operator | `beampipe admin create-user --username admin --password ... --email ...` |
| Config | Validate survey YAML | `beampipe project validate -f config/wallaby_hires.v1.yaml` |
| Config | Upload survey YAML | `POST /api/v2/project-configs` |
| Config | Upload deployment profile | `POST /api/v2/deployment-profiles` |
| Source load | Register sources | `POST /api/v2/sources` or `POST /api/v2/sources/bulk` |
| Discovery | Trigger or schedule discovery | `POST /api/v2/sources/discover` |
| Execution | Create execution intent | `POST /api/v2/executions` |
| Execution | Queue execution | `POST /api/v2/executions/{id}/execute` |
| Monitoring | Check readiness | `GET /api/v2/ready` |
| Monitoring | Inspect provenance | `GET /api/v2/executions/{id}/events` |

Use [API workflow guide](../api/index.md) for concrete request examples.

## Mock to real backend path

Start with mock backends to validate project config, discovery, manifest construction, and dry execution. Move to real backends only after the environment has credentials, TAP reachability, Translator Manager access, and a tested DIM or Slurm deployment profile.

```bash
export BEAMPIPE_USE_REAL_BACKENDS=true
export BEAMPIPE_ENV=production
export CASDA_USERNAME=...
export CASDA_PASSWORD_FILE=/run/secrets/casda_password
export SLURM_SSH_PRIVATE_KEY_FILE=/run/secrets/slurm_ssh_key
export SLURM_SSH_KNOWN_HOSTS_SOURCE=/run/slurm-ssh/known_hosts
```

Run `beampipe security check` before production startup and `beampipe slurm ping --profile <name>` before live Slurm submission.

## What to watch

| Signal | Healthy shape | Where |
|--------|---------------|-------|
| Readiness | Database, queue, workers, and dependencies report ready | `GET /api/v2/ready` |
| Queue depth | Does not grow indefinitely after ticks | readiness payload, metrics |
| Oldest queued job | Stays near expected job time | metrics |
| Discovery metadata | Sources receive metadata and discovery flags | source events |
| Execution ledger | Runs move through stage, translate, submit, poll, terminal state | execution events |
| Backend debug fields | DIM or Slurm fields appear on execution responses | execution response |

## First triage

| Symptom | First checks | Next page |
|---------|--------------|-----------|
| API cannot start | `DATABASE_URL`, migrations, bind address, startup security gates | [Production runbook](production-runbook.md) |
| Login fails | Admin user exists, `BEAMPIPE_JWT_SECRET` is stable | [First run](../getting-started/first-run.md) |
| Discovery stalls | TAP health, source enabled state, queue depth, discovery caps | [Workers and scheduling](workers-scheduling.md) |
| Execution remains pending | Project config active, metadata ready, execution caps | [Workers and scheduling](workers-scheduling.md) |
| Slurm run stalls | Known hosts, SSH key, login node, poll events | [Deployment profiles](../architecture/deployment-profiles.md) |
| Redoc stale | Export OpenAPI and copy it into docs assets | [OpenAPI export](../tools/openapi.md) |

Next: tune worker capacity in [Workers and scheduling](workers-scheduling.md).

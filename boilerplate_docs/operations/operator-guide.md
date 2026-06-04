# Operator guide

This page is the daily operating map for beampipe-core v2.

## Process layout

<div class="terminal-diagram">
<pre>+-------------+       +--------------+       +----------------+
| API         |       | scheduler    |       | worker replicas |
| /api/v2     |       | tick source  |       | claim jobs      |
| auth/rate   |       | enqueue jobs |       | run backends    |
+------+------+       +------+-------+       +-------+--------+
       |                     |                       |
       +---------------------+-----------------------+
                             |
                      +------v------+
                      | PostgreSQL  |
                      | configs     |
                      | jobs        |
                      | ledger      |
                      +-------------+</pre>
</div>

Run exactly one scheduler-enabled process per environment. Scale API and worker-only processes independently.

```bash
beampipe serve --worker false
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false BEAMPIPE_WORKER_CONCURRENCY=4 beampipe worker
```

Compose already applies this split with `api`, `scheduler`, and `worker` services.

## Operator checklist

| Step | Command or endpoint |
|------|---------------------|
| Apply migrations | `beampipe migrate` |
| Create admin | `beampipe admin create-user --username admin --password ... --email ...` |
| Validate project config | `beampipe project validate -f config/wallaby_hires.v1.yaml` |
| Upload project config | `POST /api/v2/project-configs` |
| Register sources | `POST /api/v2/sources` or `POST /api/v2/sources/bulk` |
| Trigger discovery | `POST /api/v2/sources/discover` |
| Create execution | `POST /api/v2/executions` |
| Queue execution | `POST /api/v2/executions/{id}/execute` |
| Check readiness | `GET /api/v2/ready` |
| Scrape metrics | `GET /api/v2/metrics` or `:9090/metrics` |

## Real backends

Mock backends are the default. Enable real CASDA, Translator Manager, DIM, and Slurm clients only after connectivity has been tested.

```bash
export BEAMPIPE_USE_REAL_BACKENDS=true
export CASDA_USERNAME=...
export CASDA_PASSWORD=...
export SLURM_SSH_PRIVATE_KEY_FILE=./deploy/ssh/id_slurm
export SLURM_SSH_KNOWN_HOSTS_SOURCE=./deploy/ssh/known_hosts
```

Use `beampipe slurm ping --profile <name>` for Slurm SSH smoke tests when profile data exists.

## Failure triage

| Symptom | First checks |
|---------|--------------|
| API cannot start | `DATABASE_URL`, migrations, bind address |
| Login fails | admin user exists, `BEAMPIPE_JWT_SECRET` stable |
| Discovery stalls | TAP health, source enabled state, queue depth, discovery caps |
| Execution remains pending | project config active, metadata ready, execution admission caps |
| Slurm run stalls | known hosts, SSH key, login node reachability, poll tick events |
| Redoc stale | run `beampipe openapi export > openapi.json` and copy to docs |

Use [Observability](observability.md) for metric names and provenance endpoints.

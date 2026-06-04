# Control plane architecture

beampipe-core coordinates archive discovery and execution state. It does not run the science graph itself; it prepares manifests, calls translation/deployment services, and records what happened.

## System shape

<div class="terminal-diagram">
<pre>                 +----------------+
                 | operator/API   |
                 | /api/v2        |
                 +--------+-------+
                          |
                          v
+-----------+     +-------+--------+     +------------------+
| project   | --> | PostgreSQL     | <-- | workers          |
| config    |     | configs/jobs   |     | discovery/exec   |
+-----------+     | ledger/events  |     +--------+---------+
                  +-------+--------+              |
                          |                       v
                          |              +--------+---------+
                          |              | adapters/backends|
                          |              | CASDA/TM/DIM/SSH |
                          |              +------------------+
                          v
                   metrics + alerts</pre>
</div>

## Core responsibilities

| Area | Responsibility |
|------|----------------|
| API | Auth, source registry, executions, project configs, profiles, alerts |
| Database | Active project configs, source state, archive metadata, jobs, execution ledger |
| Worker | Scheduler ticks, TAP discovery, manifest build, staging, submit, polling |
| Project config | Survey-specific queries, transforms, manifest shape, graph patches, automation |
| Deployment profile | DALiuGE translation and REST/Slurm deployment settings |

## State boundaries

Project configs are versioned and active executions pin the config version used to build their run. Execution records keep manifest and run-record state so operators can inspect the exact inputs and backend transitions after the job completes.

## API contract

The Rust API exposes `/api/v2` and exports OpenAPI from `utoipa`. Use [Redoc reference](../api/reference.md) for schema details.

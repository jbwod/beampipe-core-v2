# Observability

beampipe-core exposes liveness, readiness, Prometheus metrics, provenance events, alerting resources, and backend debug fields. Use metrics for dashboards and events/run records for per-run forensics.

## Endpoints

| Endpoint | Purpose |
|----------|---------|
| `GET /api/v2/health` | Process liveness |
| `GET /api/v2/ready` | Postgres, queue, worker, and dependency readiness |
| `GET /api/v2/metrics` | API process metrics |
| `:9090/metrics` | Prometheus metrics server for API/worker containers |
| `GET /api/v2/executions/{id}/events` | Execution provenance stream |
| `GET /api/v2/sources/{id}/events` | Source provenance stream |
| `GET /api/v2/projects/{module}/events` | Project provenance stream |

## Debug order

For a stuck source or run, inspect in this order:

1. `GET /api/v2/ready` for database, queue, worker, and dependency state.
2. Prometheus gauges for queue depth, oldest queued job age, dependency health, and execution state counts.
3. Provenance events for source/execution milestones.
4. Execution response run record for DALiuGE DIM/Slurm submit and poll details.

## Metrics flow

<div class="terminal-diagram">
<pre>+-------------+      +-------------+      +-------------+
| API routes  | ---> | recorder    | ---> | /metrics    |
+-------------+      +-------------+      +-------------+
| workers     | ---> | recorder    | ---> | :9090 scrape|
+-------------+      +-------------+      +-------------+
| readiness   | ---> | gauges      | ---> | Prometheus  |
+-------------+      +-------------+      +-------------+</pre>
</div>

Key metric families include API request counters, job queue gauges, dependency health, source processing counts, scheduler tick duration, and execution state counts.

## Prometheus stack

```bash
docker compose --profile observability up -d prometheus
```

Prometheus listens on `http://127.0.0.1:9099` and scrapes API/worker metrics through the Compose network.

## Alerts

Alerting resources are API-managed:

| Resource | Endpoints |
|----------|-----------|
| Notification channels | `/api/v2/notification-channels` |
| Alert rules | `/api/v2/alert-rules` |
| Deliveries | `/api/v2/alert-deliveries` |

Use `POST /api/v2/notification-channels/{id}/test` before enabling production alerts.

## Execution debug fields

Execution responses include backend-specific fields when available:

| Field | Backend |
|-------|---------|
| `dim_session_status_url` | REST/DALiuGE DIM |
| `dim_graph_status_url` | REST/DALiuGE DIM |
| `slurm_session_dir` | Slurm |
| `slurm_login_node` | Slurm |
| `slurm_remote_user` | Slurm |

Next: keep the operational checklist in [Production runbook](production-runbook.md).

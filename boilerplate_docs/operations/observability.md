# Observability

beampipe-core exposes health, readiness, metrics, provenance events, alert rules, notification channels, and execution debug URLs.

## Endpoints

| Endpoint | Purpose |
|----------|---------|
| `GET /api/v2/health` | Process liveness |
| `GET /api/v2/ready` | Postgres, queue, worker, and dependency readiness |
| `GET /api/v2/metrics` | Prometheus metrics from the API process |
| `:9090/metrics` | Prometheus metrics server for API/worker containers |
| `GET /api/v2/executions/{id}/events` | Execution provenance stream |
| `GET /api/v2/sources/{id}/events` | Source provenance stream |
| `GET /api/v2/projects/{module}/events` | Project provenance stream |

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

Key metric families include API request counters, job queue gauges, dependency health, source processing counts, and execution state counts.

## Prometheus stack

```bash
docker compose --profile observability up -d prometheus
```

Prometheus listens on `http://127.0.0.1:9099` and scrapes API/worker metrics through the compose network.

## Alerts

Alerting resources are API-managed:

| Resource | Endpoints |
|----------|-----------|
| Notification channels | `/api/v2/notification-channels` |
| Alert rules | `/api/v2/alert-rules` |
| Deliveries | `/api/v2/alert-deliveries` |

Use `POST /api/v2/notification-channels/{id}/test` before enabling production alerts.

## Execution debug URLs

Execution responses include backend-specific fields when available:

| Field | Backend |
|-------|---------|
| `dim_session_status_url` | REST/DIM |
| `dim_graph_status_url` | REST/DIM |
| `slurm_session_dir` | Slurm |
| `slurm_login_node` | Slurm |
| `slurm_remote_user` | Slurm |

If a run is stuck, inspect provenance events first, then backend-specific debug URLs.

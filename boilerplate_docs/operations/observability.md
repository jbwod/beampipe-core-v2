# Observability

Use metrics to find the affected subsystem, then use execution/source events and immutable artifacts to explain one run. Logs alone are not the ledger.

The operator Compose bundle includes persistent Prometheus, Alertmanager, and
Grafana services plus a provisioned Beampipe operations dashboard. Metrics
listeners stay on the private Compose network; the three operator UIs bind to
loopback by default.

## Endpoints

| Endpoint | Scope |
|---|---|
| `GET /api/v2/health` | Process liveness; public |
| `GET /api/v2/ready` | Authenticated database, workers, queue, and configured dependency detail |
| `GET /api/v2/metrics` | API exposition; authenticated unless explicitly public |
| `BEAMPIPE_METRICS_BIND_ADDR/metrics` | Per-process Prometheus listener |
| `/executions/{id}/events` | Execution provenance |
| `/executions/{id}/observations` | Normalized external observations |
| `/executions/{id}/artifacts` | Manifest and graph evidence |
| `/sources/{id}/events` | Discovery provenance |

Each host process needs a unique metrics bind address. Containers can all use `0.0.0.0:9090` because they have separate network namespaces. Production Compose does not publish those process listeners; Prometheus reaches them by service name on the private network.

## Dashboard order

| Row | Operator question | Signals |
|---|---|---|
| API traffic | Is operator/API demand healthy? | request rate, error ratio, p50/p95 latency, route and status |
| Queue | Is work arriving faster than it completes? | queued/running jobs, oldest age, retries, dead letters |
| Workers | Is capacity healthy and correctly routed? | active workers, heartbeats, leases, utilization by pool/capability |
| Dependencies | Is pressure external? | global TAP health plus TM, DIM, and real SSH/Slurm probes for default or in-flight deployment profiles |
| Discovery | Are sources becoming ready? | checked/changed/error outcomes, duration, pending sources |
| Execution | Are runs progressing safely? | control phase, terminal outcomes, uncertain submissions, poll errors |
| Security | Are production policies being rejected? | security-check failures and inline-secret rejections |

Do not put high-cardinality source IDs, execution IDs, session IDs, URLs, or error strings into metric labels. Those belong in events and structured logs.
In-flight source gauges are aggregated by project and phase; deployment
dependency gauges are bounded by the configured profile registry.

## Prometheus and alerts

```bash
docker compose --profile observability up -d
curl -fsS http://127.0.0.1:9099/-/ready
curl -fsS http://127.0.0.1:9099/api/v1/targets | jq .data.activeTargets
```

Open Grafana at `http://127.0.0.1:3000` using
`BEAMPIPE_GRAFANA_ADMIN_USER` and `BEAMPIPE_GRAFANA_ADMIN_PASSWORD` from the
installation `.env`. Prometheus, Alertmanager, and Grafana data survive
container replacement in named volumes. Back up those volumes separately when
historical dashboards or alert state are operationally important.

Prometheus rules live in `deploy/prometheus/alerts.yml`. The packaged
Alertmanager receiver intentionally sends nowhere, so the default bundle never
contains an unexpanded secret placeholder. Replace its config with an
operator-owned, secret-backed receiver before relying on external paging:

```bash
./scripts/promtool-test-alerts.sh
```

Beampipe also manages notification channels, alert rules, and redacted deliveries through `/api/v2`. Dash operators configure and test these on **Alerts** (`/alerts`). Prometheus/Alertmanager is a separate infra-health path and does not write in-app deliveries.

### In-app trigger kinds

| `trigger_kind` | When it fires | Notes |
|---|---|---|
| `execution_terminal` | Immediate, on execution failure | Uses the rule's `severity` and `cooldown_minutes` |
| `discovery_changed` | Immediate, after a discovery batch records `discovery.changed` | One webhook per cooldown window; payload lists at most 20 source IDs plus `changed_count` |
| `pending_backlog` | Scheduler tick | `trigger_config.threshold` (default 50) pending sources |
| `pending_stale` | Scheduler tick | `trigger_config.max_age_seconds` (default 21600) |
| `discovery_stall` | Scheduler tick | `trigger_config.window_minutes` (default 120) with zero `discovery.changed` events |
| `dependency_down` | Scheduler tick | `trigger_config.dependency` (Postgres only today) |
| `daily_summary` | Scheduler tick | `trigger_config.window_hours` (default 24). Set `cooldown_minutes` to 1440 to match. Digest counts discoveries changed, executions completed/failed/uncertain, and remaining pending sources. Unknown `trigger_kind` values are rejected on create/update. |

Webhook channel `config` is `{ "url": "https://...", "template": "generic"|"slack"|"pagerduty" }` plus optional `headers`. Test a channel before enabling its rules:

```bash
# Create a Slack webhook channel (superuser)
curl -fsS -X POST "$BEAMPIPE_API/api/v2/notification-channels" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"name":"ops-slack","kind":"webhook","config":{"url":"https://hooks.slack.com/services/...","template":"slack"}}'

# Send a test payload
curl -fsS -X POST "$BEAMPIPE_API/api/v2/notification-channels/$CHANNEL_ID/test" \
  -H "Authorization: Bearer $TOKEN"
```

The test endpoint returns `{ "delivery_id": "...", "status": "sent_or_failed" }`. Inspect the redacted row at `GET /api/v2/alert-deliveries` (or Dash **Alerts**) for the actual send result.

## Debug order

1. Check readiness and the overview dashboard time window.
2. Identify queue, worker, or dependency pressure.
3. Open the affected source or execution timeline.
4. Compare control, submission, scheduler, DALiuGE, and output axes.
5. Inspect immutable config/profile snapshots and graph artifacts.
6. Use backend-native logs only after locating the persisted external ID.

For intervention, continue with [Recovery and cancellation](recovery.md).

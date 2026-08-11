# Observability

Use metrics to find the affected subsystem, then use execution/source events and immutable artifacts to explain one run. Logs alone are not the ledger.

## Signal path

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="API and workers expose metrics to Prometheus while PostgreSQL provides per-execution evidence">
  <div class="bp-flow-node" data-tone="cyan"><span>TRAFFIC</span><strong>API</strong><small>requests + latency</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>SCRAPE</span><strong>Prometheus</strong><small>time series + alerts</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>VIEW</span><strong>Grafana</strong><small>operator overview</small></div>
  <span class="bp-flow-link" aria-hidden="true">+</span>
  <div class="bp-flow-node" data-tone="cyan"><span>FORENSICS</span><strong>events</strong><small>ledger + artifacts</small></div>
</div>

Grafana is not currently included in Compose and no dashboard JSON is tracked in this repository. Connect an external Grafana to Prometheus at `http://prometheus:9090`. A useful overview should put API traffic first, followed by queue health, workers, dependencies, discovery, and executions.

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

Each host process needs a unique metrics bind address. Containers can all use `0.0.0.0:9090` because they have separate network namespaces.

## Dashboard order

| Row | Operator question | Signals |
|---|---|---|
| API traffic | Is operator/API demand healthy? | request rate, error ratio, p50/p95 latency, route and status |
| Queue | Is work arriving faster than it completes? | queued/running jobs, oldest age, retries, dead letters |
| Workers | Is capacity healthy and correctly routed? | active workers, heartbeats, leases, utilization by pool/capability |
| Dependencies | Is pressure external? | TAP, TM, DIM, SSH/Slurm health and latency |
| Discovery | Are sources becoming ready? | checked/changed/error outcomes, duration, pending sources |
| Execution | Are runs progressing safely? | control phase, terminal outcomes, uncertain submissions, poll errors |
| Security | Are production policies being rejected? | security-check failures and inline-secret rejections |

Do not put high-cardinality source IDs, execution IDs, session IDs, URLs, or error strings into metric labels. Those belong in events and structured logs.

## Prometheus and alerts

```bash
docker compose --profile observability up -d prometheus
curl -fsS http://127.0.0.1:9099/-/ready
curl -fsS http://127.0.0.1:9099/api/v1/targets | jq .data.activeTargets
```

Prometheus rules live in `deploy/prometheus/alerts.yml`. Alertmanager is available through the `alerting` profile:

```bash
docker compose --profile observability --profile alerting up -d
```

Beampipe also manages notification channels, alert rules, and redacted deliveries through `/api/v2`. Test a channel before enabling its rules.

## Debug order

1. Check readiness and the overview dashboard time window.
2. Identify queue, worker, or dependency pressure.
3. Open the affected source or execution timeline.
4. Compare control, submission, scheduler, DALiuGE, and output axes.
5. Inspect immutable config/profile snapshots and graph artifacts.
6. Use backend-native logs only after locating the persisted external ID.

For intervention, continue with [Recovery and cancellation](recovery.md).

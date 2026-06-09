# Configuration

Configuration is environment-driven. `DATABASE_URL` is required. Most `BEAMPIPE_*` variables have safe development defaults, but production should set explicit secrets, host-key policy, metrics binding, and queue caps.

## Minimum environment

| Variable | Default | Purpose |
|----------|---------|---------|
| `DATABASE_URL` | none | PostgreSQL connection string |
| `BEAMPIPE_JWT_SECRET` | `secret-key` | JWT signing secret; set a strong value outside local development |
| `BEAMPIPE_ENV` | `development` | `production` enables stricter startup checks |
| `BEAMPIPE_SECURITY_STRICT` | off in dev | Force startup validation without setting `BEAMPIPE_ENV=production` |

Local example:

```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me
```

## API service

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_BIND_ADDR` | `127.0.0.1:8080` | API bind address |
| `BEAMPIPE_ACCESS_TOKEN_EXPIRE_MINUTES` | `30` | Access token lifetime |
| `BEAMPIPE_REFRESH_TOKEN_EXPIRE_DAYS` | `7` | Refresh token lifetime |
| `BEAMPIPE_RATE_LIMIT_REQUESTS` | `10` | Sensitive endpoint request limit |
| `BEAMPIPE_RATE_LIMIT_PERIOD_SECONDS` | `3600` | Rate-limit window |
| `BEAMPIPE_REDIS_URL` | unset | Optional distributed rate-limit storage |

## Workers and scheduler

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_WORKER_POLL_INTERVAL_MS` | `1000` | Job claim loop delay |
| `BEAMPIPE_WORKER_LOCK_SECONDS` | `120` | Claimed job lease timeout |
| `BEAMPIPE_WORKER_CONCURRENCY` | `1` | Parallel consumers per process |
| `BEAMPIPE_WORKER_SCHEDULER_ENABLED` | `true` | Allow this process to enqueue recurring scheduler jobs |
| `BEAMPIPE_SCHEDULER_INTERVAL_SECONDS` | `60` | Scheduler cadence |
| `BEAMPIPE_DB_MAX_CONNECTIONS` | `10` | SQLx pool size per process |

Run one scheduler-enabled process per environment. Scale API and worker-only processes independently; see [Workers and scheduling](../operations/workers-scheduling.md).

## Discovery and execution admission

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY` | `5` | Parallel TAP work inside one discovery batch |
| `BEAMPIPE_DISCOVERY_TAP_HEALTH_CHECK_ENABLED` | `true` | Probe TAP adapters before discovery |
| `BEAMPIPE_DISCOVERY_TAP_HEALTH_TIMEOUT_SECONDS` | `10` | TAP probe timeout |
| `BEAMPIPE_SHAPING_DISCOVERY_MAX_IN_FLIGHT_BATCHES` | `4` | Global discovery batch cap |
| `BEAMPIPE_SHAPING_DISCOVERY_MAX_BATCHES_PER_TICK` | `4` | Batches admitted per scheduler tick |
| `BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS` | `2` | Global execution cap |
| `BEAMPIPE_SHAPING_QUEUE_MAX_DEPTH` | `200` | Queue depth admission guard |
| `BEAMPIPE_SHAPING_ENQUEUE_PACING_MS` | `0` | Optional delay between enqueued jobs |

Use environment shaping for cluster-wide safety limits. Use project config `automation.*` for survey policy.

## Backends and secrets

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_USE_REAL_BACKENDS` | `false` | Use real CASDA/TM/DIM/Slurm clients instead of mocks |
| `CASDA_USERNAME`, `CASDA_PASSWORD_FILE` | unset | CASDA staging credentials; prefer file secrets in production |
| `CASDA_PASSWORD` | unset | Inline CASDA password for local/dev injection |
| `SLURM_SSH_PRIVATE_KEY_FILE` / `SLURM_SSH_PRIVATE_KEY_PATH` | unset | Mounted Slurm SSH private key |
| `SLURM_SSH_PRIVATE_KEY` | unset | Inline PEM for local/dev only |
| `SLURM_SSH_KNOWN_HOSTS_SOURCE` / `SLURM_SSH_KNOWN_HOSTS` | unset | Known hosts file for Slurm SSH |
| `BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS` | production default | Reject SSH connections without trusted host keys |
| `BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS` | `false` | Break-glass only |
| `BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK` | `false` | Allow `~/.ssh/id_ed25519` in development only |

Deployment profiles describe the target backend; secrets stay in environment variables or mounted files. See [Deployment profiles](../architecture/deployment-profiles.md).

## Metrics and logs

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_METRICS_BIND_ADDR` | `127.0.0.1:9090` | Prometheus metrics server bind |
| `BEAMPIPE_METRICS_SERVER_ENABLED` | `true` | Start metrics server in API/worker process |
| `BEAMPIPE_LOG_JSON` | `false` | Emit JSON logs |
| `BEAMPIPE_OTEL_ENABLED` | `false` | Enable OpenTelemetry export |
| `BEAMPIPE_OTEL_ENDPOINT` | `http://127.0.0.1:4317` | OTLP endpoint |
| `BEAMPIPE_OTEL_SERVICE_NAME` | `beampipe-v2` | OTEL service name |

## Compose defaults

`docker-compose.yml` sets higher-throughput defaults for scheduler and worker containers: discovery batch caps, execution caps, worker lock seconds, metrics binding, and backend URLs. Override with shell environment variables before `docker compose up`.

Next: use [Operator guide](../operations/operator-guide.md) for the day-to-day process model.

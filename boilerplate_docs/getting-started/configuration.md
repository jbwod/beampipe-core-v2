# Configuration

Configuration is environment-driven. `DATABASE_URL` is required; all `BEAMPIPE_*` variables below have defaults unless noted.

## Required

| Variable | Default | Purpose |
|----------|---------|---------|
| `DATABASE_URL` | none | PostgreSQL connection string |
| `BEAMPIPE_JWT_SECRET` | `secret-key` | JWT signing secret; set a strong value outside local dev |

## API

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_BIND_ADDR` | `127.0.0.1:8080` | API bind address |
| `BEAMPIPE_ACCESS_TOKEN_EXPIRE_MINUTES` | `30` | Access token lifetime |
| `BEAMPIPE_REFRESH_TOKEN_EXPIRE_DAYS` | `7` | Refresh token lifetime |
| `BEAMPIPE_RATE_LIMIT_REQUESTS` | `10` | Sensitive endpoint request limit |
| `BEAMPIPE_RATE_LIMIT_PERIOD_SECONDS` | `3600` | Rate-limit window |
| `BEAMPIPE_REDIS_URL` | unset | Optional distributed rate-limit storage |

## Workers and scheduling

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_WORKER_POLL_INTERVAL_MS` | `1000` | Job claim loop delay |
| `BEAMPIPE_WORKER_LOCK_SECONDS` | `120` | Claimed job lock timeout |
| `BEAMPIPE_WORKER_CONCURRENCY` | `1` | Parallel job consumers per process |
| `BEAMPIPE_WORKER_SCHEDULER_ENABLED` | `true` | Enqueue recurring scheduler jobs in this process |
| `BEAMPIPE_SCHEDULER_INTERVAL_SECONDS` | `60` | Recurring scheduler bootstrap interval |
| `BEAMPIPE_DB_MAX_CONNECTIONS` | `10` | SQLx pool size per process |

## Discovery and admission

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

## Backends

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_USE_REAL_BACKENDS` | `false` | Use real CASDA/TM/DIM/Slurm clients instead of mocks |
| `CASDA_USERNAME`, `CASDA_PASSWORD` | unset | CASDA staging credentials |
| `SLURM_SSH_PRIVATE_KEY_FILE` | unset | SSH key for Slurm login nodes |
| `SLURM_SSH_KNOWN_HOSTS_SOURCE` | unset | Known hosts file for Slurm SSH |
| `BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS` | `false` | Reject Slurm SSH when known hosts are missing |

## Metrics and logs

| Variable | Default | Purpose |
|----------|---------|---------|
| `BEAMPIPE_METRICS_BIND_ADDR` | `127.0.0.1:9090` | Prometheus metrics server bind |
| `BEAMPIPE_METRICS_SERVER_ENABLED` | `true` | Start metrics server in API/worker process |
| `BEAMPIPE_LOG_JSON` | `false` | Emit JSON logs |
| `BEAMPIPE_OTEL_ENABLED` | `false` | Enable OpenTelemetry export |
| `BEAMPIPE_OTEL_ENDPOINT` | `http://127.0.0.1:4317` | OTLP endpoint |
| `BEAMPIPE_OTEL_SERVICE_NAME` | `beampipe-v2` | OTEL service name |

## Compose profile

`docker-compose.yml` sets higher-throughput defaults for scheduler and worker containers: discovery batch caps, execution caps, worker lock seconds, metrics binding, and backend URLs. Override those with shell environment variables before `docker compose up`.

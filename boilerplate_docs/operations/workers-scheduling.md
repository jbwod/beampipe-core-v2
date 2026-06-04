# Workers and scheduling

Workers claim Postgres jobs with row locks, so horizontal scaling is safe when replicas share the same database. Scheduler-enabled processes enqueue recurring ticks; worker-only processes consume the queue.

## Job flow

<div class="terminal-diagram">
<pre>scheduler bootstrap
       |
       v
+------------------+       +-------------------+       +------------------+
| scheduler_tick   | ----> | discover_batch    | ----> | metadata rows    |
+------------------+       +-------------------+       +------------------+
| execution_tick   | ----> | execute           | ----> | run ledger       |
+------------------+       +-------------------+       +------------------+
| slurm_poll_tick  | ----> | batched SSH poll  | ----> | execution state  |
+------------------+       +-------------------+       +------------------+
| dim_poll_tick    | ----> | batched DIM poll  | ----> | execution state  |
+------------------+       +-------------------+       +------------------+</pre>
</div>

## Recommended layouts

Local development:

```bash
beampipe serve --worker true
```

Production split:

```bash
# API only
beampipe serve --worker false

# one scheduler process
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true BEAMPIPE_WORKER_CONCURRENCY=2 beampipe worker

# N worker replicas
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false BEAMPIPE_WORKER_CONCURRENCY=4 beampipe worker
```

Compose equivalent:

```bash
docker compose up -d api scheduler
docker compose up -d --scale worker=8 worker
```

## Tuning knobs

| Variable | Effect |
|----------|--------|
| `BEAMPIPE_WORKER_CONCURRENCY` | Parallel consumers in one process |
| `BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY` | Parallel TAP requests inside each discovery batch |
| `BEAMPIPE_SHAPING_DISCOVERY_MAX_IN_FLIGHT_BATCHES` | Global discovery batch concurrency |
| `BEAMPIPE_SHAPING_DISCOVERY_MAX_BATCHES_PER_TICK` | Batch enqueue rate per scheduler tick |
| `BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS` | Global execute/stage/submit cap |
| `BEAMPIPE_SHAPING_QUEUE_MAX_DEPTH` | Queue admission guard |
| `BEAMPIPE_WORKER_LOCK_SECONDS` | Lock timeout for claimed jobs |
| `BEAMPIPE_SCHEDULER_INTERVAL_SECONDS` | Recurring scheduler cadence |

Project configs can further constrain discovery and execution under `automation.discovery` and `automation.execution`.

## 1000-source profile

The reference `config/wallaby_hires.v1.yaml` is tuned for large source sets:

```bash
docker compose up -d --scale worker=8
```

With compose defaults this gives 8 worker replicas and 4 consumers per worker. Discovery throughput is gated by CASDA/TAP latency and the configured in-flight caps; execution throughput is gated by staging, Slurm capacity, and `BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS`.

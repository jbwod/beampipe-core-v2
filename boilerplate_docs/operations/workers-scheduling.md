# Workers and scheduling

Workers claim jobs from PostgreSQL. Schedulers enqueue recurring work. This page is the queue and tuning reference; use [Operator guide](operator-guide.md) for the process overview.

## Queue lifecycle

<div class="terminal-diagram">
<pre>scheduler tick
     |
     v
+------------------+      +----------------------+      +------------------+
| enqueue job      | ---> | worker claims row    | ---> | job handler      |
| discovery/exec   |      | lock + lease         |      | TAP/DALiuGE/Slurm |
+------------------+      +----------+-----------+      +--------+---------+
                                      |                           |
                                      v                           v
                              renew or release             events + metrics</pre>
</div>

If a worker exits mid-job, the lease expires after `BEAMPIPE_WORKER_LOCK_SECONDS` and another worker can recover the job.

## Job families

| Job family | Enqueued by | Consumed by | Purpose |
|------------|-------------|-------------|---------|
| `scheduler_tick` | Scheduler bootstrap | Scheduler-enabled process | Periodically enqueue discovery/execution/poll ticks |
| `discover_batch` | Discovery tick or API request | Worker | Query adapters and prepare source metadata |
| `execution_tick` | Scheduler tick | Worker | Admit eligible sources into execution runs |
| `execute` | API or execution tick | Worker | Stage, translate, submit, and record a run |
| `slurm_poll_tick` | Scheduler tick | Worker | Batch Slurm state polling over SSH |
| `dim_poll_tick` | Scheduler tick | Worker | Poll DALiuGE DIM/REST deployment state |

## Process layouts

Single-process development:

```bash
beampipe serve --worker true
```

Split host processes:

```bash
beampipe serve --worker false
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true BEAMPIPE_WORKER_CONCURRENCY=2 beampipe worker
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false BEAMPIPE_WORKER_CONCURRENCY=4 beampipe worker
```

Compose:

```bash
docker compose up -d api scheduler
docker compose up -d --scale worker=8 worker
```

Do not run more than one scheduler-enabled process unless you are deliberately testing duplicate-tick protection.

## Tuning reference

| Variable | Applies to | Effect |
|----------|------------|--------|
| `BEAMPIPE_WORKER_CONCURRENCY` | Worker process | Parallel queue consumers inside one process |
| `BEAMPIPE_WORKER_LOCK_SECONDS` | Worker process | Lease duration for claimed jobs |
| `BEAMPIPE_SCHEDULER_INTERVAL_SECONDS` | Scheduler | Recurring scheduler cadence |
| `BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY` | Discovery job | Parallel TAP requests inside each discovery batch |
| `BEAMPIPE_SHAPING_QUEUE_MAX_DEPTH` | Scheduler/admission | Stops enqueue when queue depth is too high |
| `BEAMPIPE_SHAPING_DISCOVERY_MAX_BATCHES_PER_TICK` | Discovery tick | Batch enqueue limit per tick |
| `BEAMPIPE_SHAPING_DISCOVERY_MAX_IN_FLIGHT_BATCHES` | Discovery admission | Global discovery batch concurrency |
| `BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS` | Execution admission | Global execute/stage/submit cap |

## Survey automation caps

Project config adds survey-local caps under `automation.discovery` and `automation.execution`.

```yaml
automation:
  discovery:
    enabled: true
    tick_discovery_source_limit: 1000
    batch_size: 10
    tick_discovery_batch_limit: 100
    concurrent_discovery_batch_limit: 24
    stale_after_hours: 24
  execution:
    enabled: true
    max_sources_per_execution: 1
    tick_execution_source_limit: 1000
    tick_execution_run_limit: 50
    concurrent_execution_run_limit: 10
    deployment_profile_name: slurm-remote
```

Treat global `BEAMPIPE_SHAPING_*` values as cluster safety limits and project config as survey policy.

## Sizing patterns

| Scenario | Starting point | Watch |
|----------|----------------|-------|
| Local test | One API with embedded worker | API latency, queue depth |
| 100 sources | One scheduler, two workers, `BEAMPIPE_WORKER_CONCURRENCY=2` | TAP latency and discovery completion |
| 1000 sources | One scheduler, eight workers, `BEAMPIPE_WORKER_CONCURRENCY=4` | CASDA/VizieR latency, queue age, metadata freshness |
| Slurm-heavy execution | Keep discovery steady, raise workers slowly | Slurm poll duration, SSH failures, remote account limits |

Scale one dimension at a time. If queue age grows while dependency latency is normal, add worker capacity. If dependency latency grows, lower concurrency or batch size before adding more workers.

## Recovery behavior

| Situation | Expected behavior |
|-----------|-------------------|
| Worker exits mid-job | Lease expires; another worker can recover |
| Scheduler restarts | Recurring ticks resume from durable state |
| TAP outage | Discovery jobs fail or defer; dependency readiness shows cause |
| Slurm login outage | Poll jobs fail or retry; execution ledger remains inspectable |
| Queue depth too high | Admission pauses until depth returns below caps |

Next: use [Observability](observability.md) for metrics, events, and debug URLs.

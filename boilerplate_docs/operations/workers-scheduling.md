# Workers and scheduling

Schedulers create durable intent. Workers claim compatible PostgreSQL jobs under renewable leases. Fencing prevents a stale worker from committing after another worker has recovered the job.

## Queue lifecycle

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Durable job lifecycle from scheduler enqueue through worker claim and completion or retry">
  <div class="bp-flow-node" data-tone="cyan"><span>TICK</span><strong>enqueue</strong><small>idempotency key</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>DB</span><strong>queued</strong><small>pool + capability</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>LEASE</span><strong>claimed</strong><small>SKIP LOCKED</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>EFFECT</span><strong>handler</strong><small>heartbeat + fence</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>RESULT</span><strong>complete</strong><small>or delayed retry</small></div>
</div>

If a worker exits, the lease expires and another compatible worker can recover the job. Retries use exponential delay with jitter; exhausted work becomes operator-visible instead of looping forever.

## Job families

| Job | Purpose |
|---|---|
| `scheduler_tick` | Claim stale sources and enqueue discovery batches |
| `discover_batch` | Run project-configured TAP queries and persist metadata |
| `execution_scheduler_tick` | Admit workflow-pending sources under policy limits |
| `execute` | Stage, prepare, translate, and submit an execution |
| `dim_poll`, `dim_poll_tick` | Reconcile REST/DIM sessions |
| `slurm_poll_tick` | Batch `squeue`/`sacct` observations by Slurm target |
| `alert_evaluator_tick` | Evaluate configured alert rules |

## Scale safely

```bash
# one recurring scheduler
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true

# worker-only replicas
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false \
BEAMPIPE_WORKER_CONCURRENCY=4 \
beampipe worker
```

| Control | Scope | Use |
|---|---|---|
| `BEAMPIPE_WORKER_CONCURRENCY` | one process | Parallel claimed jobs |
| `BEAMPIPE_WORKER_LOCK_SECONDS` | one claim | Lease duration |
| `BEAMPIPE_WORKER_SUBMISSION_TIMEOUT_SECONDS` | `1800` | Maximum wall time for one post-intent backend submission attempt (range `1`–`86400`) |
| `BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY` | one discovery batch | Concurrent TAP requests |
| `BEAMPIPE_SHAPING_QUEUE_MAX_DEPTH` | environment | Stop enqueue under backlog |
| `BEAMPIPE_SHAPING_DISCOVERY_MAX_IN_FLIGHT_BATCHES` | environment | Protect TAP services |
| `BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS` | environment | Protect execution backends |
| `automation.*` | project | Survey-specific cadence and grouping |
| `max_concurrent_executions` | profile | Protect one DIM or Slurm target |

Start with low profile and global execution caps. Increase one limit at a time while watching queue age, dependency latency, SSH sessions, submission errors, and scheduler poll duration. For Slurm, polling is already batched by target; submission and SFTP still create per-execution login-node pressure.

## Inspect and drain

```bash
beampipe worker list --include-stopped
beampipe worker inspect "$WORKER_ID"
beampipe worker leases --include-expired
beampipe worker pools
beampipe worker drain "$WORKER_ID"
beampipe worker resume "$WORKER_ID"
```

Draining stops new claims and allows active leases to finish. During upgrades, drain workers before replacing binaries. During a dependency outage, lower admission or stop the scheduler before adding capacity: more workers amplify a slow TAP service or login node.

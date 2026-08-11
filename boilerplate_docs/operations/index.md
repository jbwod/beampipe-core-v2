---
hide:
  - toc
---

# Operator handbook

Operate from durable state outward: PostgreSQL first, then worker ownership, then the scheduler or DALiuGE observation. Never infer external success from a control-plane status alone.

## Start a shift

```bash
beampipe doctor
beampipe status
beampipe worker list
beampipe console
```

For a live profile, add:

```bash
beampipe doctor --profile PROFILE
beampipe scheduler status --profile PROFILE
beampipe daliuge inspect --profile PROFILE
```

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Operator inspection order from readiness through durable state to external systems">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>readiness</strong><small>process + dependencies</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02</span><strong>ledger</strong><small>intent + exact axes</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03</span><strong>worker</strong><small>claim + heartbeat</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04</span><strong>external</strong><small>Slurm + DALiuGE</small></div>
</div>

## Process roles

| Role | Command | Scale rule |
|---|---|---|
| API | `beampipe serve --worker false` | Scale for HTTP traffic |
| Scheduler | `BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true` | Exactly one per environment |
| Worker | `BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker` | Scale for queue throughput |
| PostgreSQL | external service | One logical primary; back it up |

All roles coordinate through PostgreSQL. The console is a projection of that state plus explicit live probes, not a second control plane.

## Console

```bash
beampipe console --refresh-ms 2000
```

The console covers overview, sources, executions, workers, scheduler, DALiuGE, logs, and confirmed operator actions. Use `?` for contextual keys, `/` to filter, `Enter` to inspect, `p` to pause refresh, and `q` to quit. Drain, retry, and cancellation require confirmation and are audited.

## Triage by symptom

| Symptom | First evidence | Next action |
|---|---|---|
| API not ready | `beampipe doctor`, database and migration checks | Correct configuration before restarting |
| Queue grows | queue age, worker heartbeat, dependency latency | [Separate admission from capacity](workers-scheduling.md) |
| Discovery stalls | source claim, TAP health, project query diagnostics | Reduce concurrency or correct project YAML |
| Execution stays pending | readiness, automation threshold, profile and global caps | Inspect scheduler decision events |
| Submission uncertain | stable external identity and observations | [Reconcile; do not resubmit](recovery.md) |
| Slurm polling fails | profile snapshot, SSH trust, `squeue`/`sacct` probe | Run profile doctor and Slurm ping |
| DALiuGE graph fails | session ID, graph status, error drops, artifact hashes | Compare graph/runtime versions |
| Metrics disappear | per-process metrics listener and Prometheus target | [Observability](observability.md) |

## Routine actions

```bash
beampipe timeline execution "$EXECUTION_ID" --table
beampipe graph diff --execution "$EXECUTION_ID"
beampipe worker leases --include-expired
beampipe scheduler jobs --limit 100
```

Use [Recovery and cancellation](recovery.md) before retrying failed work and [Production runbook](production-runbook.md) before changing binaries, project revisions, profiles, or secrets.

<div class="terminal-note" data-tone="amber">
<strong>When external state is uncertain, reconcile.</strong><br>
An SSH timeout after <code>sbatch</code> or an HTTP failure after DIM deployment is not proof that no external work exists.
</div>

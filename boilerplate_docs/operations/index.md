---
hide:
  - toc
---

# Operator handbook

## Process roles

| Role | Command | Scale rule |
|---|---|---|
| API | `beampipe serve --worker false` | Scale for HTTP traffic |
| Scheduler | `BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true` | Exactly one per environment |
| Worker | `BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker` | Scale for queue throughput |
| PostgreSQL | external service | One logical primary; back it up |

All roles coordinate through PostgreSQL. The console is a projection of that state plus explicit live probes, not a second control plane.

## API rate limiting and proxy trust

Sensitive API routes can use Redis-backed fixed-window rate limiting. Set
`BEAMPIPE_REDIS_URL` and, when the limiter is mandatory,
`BEAMPIPE_REQUIRE_RATE_LIMITER=true`. A required limiter fails startup when
Redis cannot be reached. Once configured, Redis errors fail requests closed in
production; development logs the dependency failure before allowing the
request.

The API always keys direct clients from the TCP peer address. It only consumes
`X-Forwarded-For` when that peer belongs to a network explicitly listed in
`BEAMPIPE_TRUSTED_PROXY_CIDRS` (a comma-separated list such as
`10.20.0.0/16,2001:db8:1::/64`). The chain is walked from right to left and
stops at the first untrusted address, so a caller cannot select a bucket by
prepending a spoofed address. Leave the setting empty when no trusted reverse
proxy is present.

Both `BEAMPIPE_RATE_LIMIT_REQUESTS` and
`BEAMPIPE_RATE_LIMIT_PERIOD_SECONDS` must be greater than zero.

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

Use [Recovery and cancellation](recovery.md) before retrying failed work. Back up PostgreSQL before changing binaries, project revisions, profiles, or secrets, and before `beampipe uninstall`.

<div class="terminal-note" data-tone="amber">
<strong>When external state is uncertain, reconcile.</strong><br>
An SSH timeout after <code>sbatch</code> or an HTTP failure after DIM deployment is not proof that no external work exists.
</div>

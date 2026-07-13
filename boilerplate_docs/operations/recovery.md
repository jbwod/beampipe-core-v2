# Failure, retry, and cancellation

Start with durable state, then inspect external systems. Do not resubmit manually while
Beampipe reports `submission_state=uncertain`; that state exists to prevent duplicate
SLURM jobs or DALiuGE sessions.

## Investigate

```bash
beampipe status
beampipe timeline execution "$EXECUTION_ID" --table
beampipe graph diff --execution "$EXECUTION_ID"
beampipe scheduler jobs --limit 100
beampipe daliuge sessions --profile PROFILE
```

The API exposes the same execution summary, observations, artifacts, and events. Keep
the stable execution ID in incident notes; it correlates scheduler jobs, DALiuGE
sessions, worker claims, artifacts, and provenance.

## Decide

| Observation | Operator action |
|-------------|-----------------|
| Failed before manifest generation | Retry after correcting archive/config cause |
| Manifest exists; translation or known pre-submit work failed | Retry can resume submission using pinned artifacts |
| Submission is `uncertain` | Wait for reconciliation; inspect stable scheduler job name |
| Scheduler job ID exists | Do not retry submission; inspect or cancel the existing job |
| DALiuGE is not definitively `not_created` | Inspect/cancel the session before creating new work |
| Scheduler and DALiuGE disagree | Treat as inconsistent; preserve both observations for review |
| Outputs failed verification | Investigate products; do not repeat irreversible work automatically |

## Retry

```bash
beampipe execution retry "$EXECUTION_ID" \
  --reason "Configuration corrected after graph patch validation failure"
```

The retry transaction locks the execution and source admission rows, verifies no active
claim or external work exists, increments the retry count once, records the reason, and
enqueues one idempotent retry job. A conflict response means no retry was created.

## Cancel

```bash
beampipe execution cancel "$EXECUTION_ID"
```

Beampipe first asks the pinned scheduler or DALiuGE deployment to cancel. It records the
ledger transition only after external cancellation is confirmed. The scheduler-specific
alias is:

```bash
beampipe scheduler cancel "$EXECUTION_ID"
```

## Worker recovery

```bash
beampipe worker list --include-stopped
beampipe worker leases --include-expired
beampipe worker drain "$WORKER_ID"
```

Claims are fenced by a persisted lease token. An active claim cannot be stolen; an
expired claim can be recovered with a new fence. Draining stops new claims while active
work completes. Exhausted poison work enters operator-review/dead-letter state instead
of looping indefinitely.

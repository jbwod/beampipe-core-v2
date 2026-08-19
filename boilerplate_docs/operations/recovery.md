# Recovery and cancellation

Start with the ledger, then reconcile external systems. Never resubmit while `submission_state=uncertain` or while a scheduler job or DALiuGE session may exist.

## Investigate

```bash
beampipe status
beampipe timeline execution "$EXECUTION_ID" --table
beampipe graph diff --execution "$EXECUTION_ID"
beampipe scheduler jobs --limit 100
```

For a known profile:

```bash
beampipe scheduler status --profile PROFILE
beampipe daliuge inspect --profile PROFILE
beampipe daliuge sessions --profile PROFILE
```

## Retry

```bash
beampipe execution retry "$EXECUTION_ID" \
  --reason "Translator endpoint restored after maintenance"
```

Retry locks the execution and source admission rows, refuses active or uncertain external work, increments the retry count, records the reason, and enqueues one idempotent job. A conflict means no retry was created.

## Cancel

```bash
beampipe execution cancel "$EXECUTION_ID"
```

The backend adapter first requests cancellation using the pinned profile and external identifier. Beampipe records the terminal transition only after the result can be classified. Use `beampipe scheduler cancel "$EXECUTION_ID"` for the scheduler-focused alias.

## Worker recovery

```bash
beampipe worker list --include-stopped
beampipe worker leases --include-expired
beampipe worker drain "$WORKER_ID"
```

An active lease cannot be stolen. An expired lease can be recovered with a new fence. If a worker process is gone, drain its record before maintenance so operators can distinguish intentional retirement from heartbeat loss.

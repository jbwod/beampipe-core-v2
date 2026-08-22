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

## PostgreSQL backup and restore drills

Back up the complete database before changing binaries, migrations, project
revisions, profiles, or secrets. The backup script writes a PostgreSQL custom
archive to a temporary file, validates its catalog, writes a SHA-256 sidecar,
and only then publishes the files in the backup directory. It retains 30 days
by default.

```bash
DATABASE_URL="$DATABASE_URL" \
BEAMPIPE_BACKUP_DIR=/srv/beampipe/backups \
BEAMPIPE_BACKUP_RETENTION_DAYS=30 \
./scripts/pg-backup.sh

./scripts/pg-restore-verify.sh \
  /srv/beampipe/backups/beampipe-YYYYMMDDTHHMMSSZ.dump
```

Copy both the `.dump` and `.dump.sha256` files to storage outside the runtime
host. A backup is not proven recoverable until it has passed a restore drill.
Create an empty disposable database whose name starts with
`beampipe_restore_drill_`, run the drill, inspect it, and then drop it
explicitly:

```bash
createdb --maintenance-db="$DATABASE_URL" beampipe_restore_drill_20260822

BEAMPIPE_RESTORE_DRILL_URL="postgresql://postgres@localhost/beampipe_restore_drill_20260822" \
  ./scripts/pg-restore-drill.sh \
  /srv/beampipe/backups/beampipe-YYYYMMDDTHHMMSSZ.dump

dropdb --maintenance-db="$DATABASE_URL" beampipe_restore_drill_20260822
```

The drill refuses a target without the reserved name prefix and refuses any
database that already contains user tables. It verifies the checksum and dump
catalog before restoring in one transaction, then checks the core ledger,
queue, registry, and project-config tables. Never point it at production.

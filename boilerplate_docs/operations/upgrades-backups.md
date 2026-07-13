# Upgrades, backup, and secrets

Database state is the durable control plane. Back it up before changing binaries,
profiles, project revisions, or PostgreSQL versions.

## Backup

```bash
pg_dump --format=custom --file "beampipe-$(date +%Y%m%dT%H%M%S).dump" "$DATABASE_URL"
```

Store the dump, deployed binary version, active project YAML, deployment-profile JSON,
and OpenAPI document together. Do not put `.env`, SSH private keys, passphrases, or
CASDA passwords in that archive.

## Upgrade

1. Drain Beampipe control-plane workers with `beampipe worker drain`.
2. Confirm no submission is `in_flight` or `uncertain`.
3. Take and verify a PostgreSQL backup.
4. Install the signed/checksummed release binary or immutable image tag.
5. Run `beampipe migrate` once.
6. Run `beampipe doctor --json` and profile-specific diagnostics.
7. Start API, one scheduler-enabled process, then worker replicas.
8. Resume drained workers and inspect reconciliation metrics/events.

Migrations are forward-only and designed as small compatible additions. Do not roll
back a binary across a migration unless that release explicitly documents schema
compatibility. Restore the database backup into a separate PostgreSQL instance for a
full rollback rehearsal.

## Restore rehearsal

```bash
createdb beampipe_restore_test
pg_restore --clean --if-exists --no-owner \
  --dbname beampipe_restore_test beampipe-TIMESTAMP.dump
DATABASE_URL=postgres://localhost/beampipe_restore_test beampipe doctor --json
```

Use environment-appropriate PostgreSQL authentication; the command above is a shape,
not a production credential example.

## Rotate secrets

- JWT secret: stop API mutation traffic, rotate the runtime secret, restart all API
  replicas, and require operators to authenticate again.
- CASDA password: replace the mounted file atomically, restart affected workers, then
  run `beampipe doctor`.
- SSH key: add the new public key remotely, replace the mounted private-key file,
  restart scheduler-capable workers, run `beampipe profile test`, then remove the old
  remote key.
- Known hosts: verify a facility-announced host-key change out of band before replacing
  the file. Never bypass verification as routine recovery.

`beampipe config explain` shows precedence and redacts sensitive values, which is useful
after rotation without exposing the secret itself.

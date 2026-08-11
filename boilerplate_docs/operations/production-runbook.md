# Production runbook

Promote in layers: durable state, identity and security, configuration, workers, external contracts, then one controlled execution.

## Promotion sequence

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Production rollout from backup and migrations through API scheduler workers and one controlled run">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>backup</strong><small>DB + versions</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02</span><strong>migrate</strong><small>one writer</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03</span><strong>API</strong><small>health + auth</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>04</span><strong>workers</strong><small>one scheduler</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>05</span><strong>qualify</strong><small>one bounded run</small></div>
</div>

## Before rollout

```bash
beampipe --version
beampipe project validate -f config/wallaby_hires.v2.yaml
BEAMPIPE_ENV=production beampipe security check
beampipe doctor --json
```

Take a PostgreSQL backup and retain it with the binary/image version, active project YAML, redacted profile export, and OpenAPI document:

```bash
pg_dump --format=custom \
  --file "beampipe-$(date -u +%Y%m%dT%H%M%SZ).dump" \
  "$DATABASE_URL"
```

Never include `.env`, private keys, passphrases, CASDA passwords, bearer tokens, or signed URLs in the evidence archive.

## Roll out

1. Stop recurring admission or drain scheduler-capable workers.
2. Confirm no submission is `in_flight` or `uncertain`.
3. Verify the backup can be listed with `pg_restore --list`.
4. Install the immutable binary or image.
5. Run `beampipe migrate` exactly once.
6. Start API-only replicas and verify health, readiness, login, and metrics.
7. Start one scheduler-enabled process.
8. Start worker-only replicas and verify heartbeats, pools, and capabilities.
9. Run `beampipe doctor --profile PROFILE` for each live backend.
10. Register one approved source and complete the [qualification run](end-to-end-demo.md) before increasing caps.

## Change impact

| Change | Revalidate |
|---|---|
| Project queries or transforms | discovery rows, flags, signature stability |
| Manifest or graph patches | graph diff, DALiuGE application/runtime compatibility |
| Deployment profile | TM/DIM or SSH/Slurm preflight and resource render |
| Runtime image/modules | graph application signatures and output parser contracts |
| Worker or admission settings | queue age, dependency load, profile caps |
| Database migration | backup restore and mixed-version compatibility |

Project and profile revisions affect future executions. In-flight executions retain pinned snapshots.

## Rotate secrets

| Secret | Procedure |
|---|---|
| JWT | Rotate across all API replicas and require re-authentication |
| CASDA password | Replace mounted file atomically, restart affected workers, run doctor |
| SSH key | Add new public key remotely, replace mounted file, restart workers, run Slurm ping, remove old key |
| Known hosts | Verify facility-announced key out of band before replacing the file |

`beampipe config explain` reports sources with redacted values. Never disable host-key verification as routine recovery.

## Restore rehearsal

```bash
createdb beampipe_restore_test
pg_restore --clean --if-exists --no-owner \
  --dbname beampipe_restore_test beampipe-TIMESTAMP.dump
DATABASE_URL=postgres://localhost/beampipe_restore_test beampipe doctor --json
```

Migrations are forward-only. Do not run an older binary against a migrated database unless that release explicitly documents compatibility.

## Roll back or stop

Stop promotion when readiness regresses, workers cannot heartbeat, a profile preflight fails, graph/runtime versions disagree, or submission becomes uncertain. Preserve the database and external identifiers, stop new admission, and follow [Recovery and cancellation](recovery.md).

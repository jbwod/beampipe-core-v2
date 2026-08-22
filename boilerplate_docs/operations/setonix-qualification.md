# Qualify one source on Setonix

This runbook qualifies the production WALLABY graph against Pawsey Setonix
without giving background services open-ended permission to use SSH. It is an
attended, one-source test. Treat every approval boundary below as a stop: list
the exact commands, paths, resource request, and job identifiers before moving
past it.

!!! danger "SSH is never implicit approval"

    Importing a key does not authorize a connection. Profile tests, Slurm
    dependency probes, submission, polling, cancellation, remote publication,
    and cleanup can all use SSH. Run them only inside the approval window that
    names the operation.

## Qualification topology

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Beampipe stages one selected source from CASDA, translates it, submits one outer DALiuGE allocation on Setonix, supervises dataset child jobs, publishes durable outputs, and verifies the evidence in Core">
  <div class="bp-flow-node" data-tone="cyan"><span>CONTROL</span><strong>Beampipe Core</strong><small>pinned source, SBID, profile</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>STAGE</span><strong>CASDA UWS</strong><small>visibilities + calibration tar</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>OUTER</span><strong>Setonix DALiuGE</strong><small>exact sbatch receipt</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>CHILDREN</span><strong>ASKAPsoft jobs</strong><small>one per selected dataset</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>EVIDENCE</span><strong>durable outputs</strong><small>inventory + hashes + acknowledgement</small></div>
</div>

Core owns durable intent and the aggregate ledger. CASDA, Slurm, DALiuGE, and
the output filesystem remain authoritative for their own facts. An `sbatch`
receipt is not execution success, and `COMPLETED` is not output verification.

## Safety controls before installing a usable key

Use an isolated qualification installation and database where practical.
Otherwise install a qualification copy of the project with execution
automation disabled. Do not enable project automation during this run.

Keep background Slurm dependency probes disabled:

```dotenv
BEAMPIPE_USE_REAL_BACKENDS=false
BEAMPIPE_METRICS_LIVE_SLURM_PROBE_ENABLED=false
```

`BEAMPIPE_USE_REAL_BACKENDS=true` is required for the approved execution
window. It does **not** authorize background metrics SSH. Leave
`BEAMPIPE_METRICS_LIVE_SLURM_PROBE_ENABLED=false` throughout the attended run;
use explicit status observations instead.

Before requesting the first SSH approval, record these non-secret values:

- profile name and revision;
- login host, port, remote user, and credential slot;
- account, partition, QoS/constraint, nodes, tasks, CPUs, memory, and wall time;
- exact `dlg_root`, session, staging, log, and output paths;
- module, virtual-environment, and environment-setup commands;
- immutable ASKAPsoft SIF path and checksum;
- TM endpoint and manager topology;
- output capacity, retention, durable URI, and trusted publisher identity.

Do not paste private keys, passphrases, CASDA passwords, signed URLs, access
tokens, or unredacted environment dumps into tickets or chat.

## Approval map

=== "A · read-only preflight"

    Connect to the named login node and inspect commands, runtime imports,
    paths, SIF readability, and scheduler availability. No allocation and no
    remote mutation beyond SSH session bookkeeping.

=== "B · one execution"

    Permit the exact CASDA staging work, remote session files, outer `sbatch`,
    bounded exact-ID polling, and the declared child-job envelope.

=== "C · publication"

    Verify and copy only the run's output inventory to the approved durable
    destination, then acknowledge that receipt to Core locally.

=== "D · cleanup"

    Remove only ledger-recorded UUID-scoped paths after all outer and child
    identifiers are terminal and durable evidence has been retained.

Cancellation is its own explicit approval unless the approval packet grants a
bounded emergency cancellation rule for the exact recorded job identifiers.

## 1. Local-only validation

The operator may validate and render the installed profile without connecting
to Setonix:

```bash
beampipe profile validate slurm-remote
beampipe profile render slurm-remote
beampipe security check
beampipe scheduler jobs --limit 100
```

Review the rendered shell setup as code. Profile-defined `modules`, `venv`, and
`environment_setup` execute remotely during preflight and submission; a command
cannot be certified read-only until those prefixes are understood.

Import credentials from protected local files only after validating the slot
name and host trust material. Prefer Pawsey's key-registration process and a
facility-verified `known_hosts` entry. `credentials init` can generate a local
key, `copy-id` changes the remote account, and host-key scanning contacts the
host; each needs separate intent.

The production qualification artifacts are:

| Artifact | Expected evidence |
|---|---|
| WALLABY graph | SHA-256 `56faf68f4e22bab5a5976c081f54ad8c2dd4c17c71d02c9b6843c531d2f9a47b` |
| Wallaby package | version `0.1.6` |
| Python wheel | SHA-256 `a0038041e5dd647139fc1b952ca388729b9d53ab60a2412a876bd85f012f487e` |

Installing the package into a local `/daliuge` runtime does not prove that the
Setonix login environment and compute-node environment contain the same build.

## 2. Approval A: read-only Setonix preflight

Show the operator the effective profile prefixes and the exact host first.
Core's scheduler and execution checks may then run these commands:

```text
command -v sbatch
command -v squeue
command -v sacct
command -v scancel
command -v scontrol
command -v srun
command -v python3
command -v wallaby_hires
wallaby_hires --version
sinfo --version
python3 -c 'import dlg.deploy.create_dlg_job; import wallaby_hires'
test -d '<DLG_ROOT>' && test -w '<DLG_ROOT>'
command -v singularity
test -f "$BEAMPIPE_ASKAPSOFT_SIF" && test -r "$BEAMPIPE_ASKAPSOFT_SIF"
```

Stop if Wallaby is not `0.1.6`, imports resolve from an unexpected environment,
the SIF differs, or the dedicated DLG root is not writable. Transferring or
installing the wheel is a separate remote-mutation approval. A compute-node
import or container smoke test consumes an allocation and is not part of this
read-only phase.

## 3. Discover and freeze one SBID

Run a fresh discovery, wait until the source has no active claim, then discover
again. Both runs must produce the same non-empty discovery signature. Archive
holdings are time-varying; never substitute a remembered row count for the
stored signature.

The known qualification candidate is `HIPASSJ1317-16`, SBID `72962`. Current
discovery remains authoritative. Its accepted metadata must have:

- only the explicitly selected source and SBID;
- unique dataset identities and complete access evidence;
- no missing or skipped selected dataset;
- exactly one evaluation object with `format=calibration`;
- an evaluation filename matching
  `calibration-metadata-processing-logs-SB72962_YYYY-MM-DD-HHMMSS.tar`.

Diagnostic or validation archives are not calibration substitutes. Missing or
ambiguous calibration results must fail preparation.

Prepare with an explicit selection:

```json
{
  "project_module": "wallaby_hires",
  "sources": [
    {
      "source_identifier": "HIPASSJ1317-16",
      "sbids": ["72962"]
    }
  ],
  "archive_name": "casda",
  "deployment_profile_name": "slurm-remote"
}
```

Send the frozen body to `POST /api/v2/executions/prepare`. Require `valid=true`,
the expected selected datasets, the pinned discovery signature, and the exact
profile revision. Create the execution using a persisted unique
`Idempotency-Key`. Any selection, signature, graph, or profile change invalidates
the approval packet.

## 4. Calculate the resource envelope

For the known three-dataset candidate, the qualified graph creates three child
imager jobs. Each child requests:

```text
partition=work  nodes=1  ntasks=2  ntasks-per-node=2
cpus-per-task=1  memory=12G  time=00:20:00
```

The nested upper bound is therefore three concurrent nodes, six tasks, 36 GB
requested memory, and 60 node-minutes. Add the separately rendered outer
DALiuGE allocation. The approval packet must show both; approving the outer job
does not conceal the nested request.

For one SBID, CASDA staging normally creates one visibility UWS job and one
calibration-evaluation UWS job. Record the exact selected filenames, byte
estimate, calibration filename, signed-URL lifetime, remote paths, session name,
poll cadence, and cutoff before submission.

## 5. Approval B: stage and submit once

Beampipe currently has no durable pause between authenticated CASDA staging and
Slurm dispatch. `do_stage=true, do_submit=false` creates a terminal
`not_submitted` execution; it is not a reusable staged receipt. Approval B must
therefore cover this one bundled operation:

```http
POST /api/v2/executions/{EXECUTION_ID}/execute
Content-Type: application/json

{"do_stage":true,"do_submit":true}
```

The worker may verify CASDA credentials, create and poll the two UWS jobs,
translate the graph, create the UUID-scoped remote PGT/INI/script/session files,
run the outer `sbatch --parsable`, and poll the validated exact outer ID. Never
retry an ambiguous submission. Core holds `in_flight` or `uncertain` work for
reconciliation by its stable session name.

The submission attempt has one persisted wall-clock deadline. Its default is
30 minutes (`BEAMPIPE_WORKER_SUBMISSION_TIMEOUT_SECONDS=1800`); the accepted
range is 1 to 86,400 seconds. The deadline covers the whole backend submit
future, not each SSH command independently. A timeout leaves the submission
`uncertain` because Setonix may have accepted work before the response was
lost. It never makes an automatic retry safe.

At intent creation Core also freezes a fingerprint of the resolved login host,
port, remote user, and credential slot. If environment fallback would resolve a
different user later, name reconciliation stops before opening SSH. Correct the
configuration and review the evidence; do not search a different scheduler
namespace and treat its absence as proof.

Signed URL expiry is measured against queue delay as well as staging time. If
the URL will expire before the job can consume it, cancel the exact run with
approval and create a newly staged execution; do not reuse stale URLs.

## 6. Attend the run

These views are local control-plane reads and do not initiate SSH:

```bash
beampipe scheduler jobs --limit 100
beampipe timeline execution "$EXECUTION_ID" --table
```

Also inspect the execution status, ledger snapshot, observations, events, and
artifacts through the API or Dashboard. Keep generic live metrics probing off.
Profile Test, `doctor --profile`, scheduler status/ping, execution polling, and
cancellation are SSH operations and remain approval-bound.

For every Slurm observation, preserve the exact outer ID, normalized state, raw
state, source (`squeue` or `sacct`), reason, and timestamp. Each Wallaby child is
submitted held, records its exact ID in a mode-0600 lifecycle directory, and is
then released. Capture every `BEAMPIPE_CHILD_JOB_ID`. The normal trap cancels
recorded children when the outer workflow exits, but `SIGKILL` or node loss can
still orphan one; never cancel by username, wildcard, or job-name prefix.

After a separately approved cancellation, Core issues `scancel -- <OUTER_ID>`
and confirms `CANCELLED` using the same SSH session. A zero exit from `scancel`
alone is not confirmation. Cancel any orphan child only by its validated exact
ID and with separate approval.

## 7. Last-resort unresolved-submission barrier

If the outer job ID is still unknown, do not retry and do not call cancellation
with a name or wildcard. The superuser abandonment endpoint is a local,
terminal safety fence; it does not contact Setonix and does not prove that no
external job exists.

Use it only under a separate operator approval after all of these are true:

- the persisted submission deadline and latest execute-worker activity have
  both been quiet for at least 24 hours;
- no execute worker has an active or incompletely fenced lease;
- the resolved login target and remote user still match the submission intent;
- at least three separately approved exact-name lookups completed against both
  `squeue` and `sacct`, span at least ten minutes, and the newest is no more than
  ten minutes old;
- the latest lookup is complete and negative, and no exact or ambiguous match
  has been observed after the quiet grace;
- the operator accepts that an external job may nevertheless exist.

Refresh the execution immediately before sending the compare-and-set values:

```http
POST /api/v2/executions/{EXECUTION_ID}/submission/abandon
Authorization: Bearer <LOCAL_SUPERUSER_TOKEN>
Content-Type: application/json

{
  "reason": "attended review completed; external submission remains unresolved",
  "expected_submission_state": "uncertain",
  "expected_daliuge_session_id": "<EXACT_PERSISTED_SESSION_ID>",
  "expected_submission_deadline_at": "<EXACT_PERSISTED_RFC3339_TIMESTAMP>",
  "acknowledge_external_job_may_exist": true
}
```

The transaction marks the ledger `failed` with outcome `inconsistent`, fences
queued or expired execute jobs, and records the evidence IDs and rationale. It
does not clear the unresolved submission axis. Automatic name reconciliation
stops for that execution. A late exact receipt is retained idempotently as
evidence but cannot reopen the terminal ledger, and retry remains blocked.

## 8. Approval C: publish and verify outputs

Slurm `COMPLETED` records compute evidence only. The production project requires
a non-empty Wallaby output inventory, durable publication, and trusted
acknowledgement. On the approved filesystem, verify and publish only the run's
paths:

```text
wallaby_hires verify-inventory <STAGING_ROOT> <INVENTORY>
wallaby_hires publish-local <STAGING_ROOT> <INVENTORY> <DURABLE_DESTINATION>
```

Re-hash the durable copy. Keep the Core superuser token off Setonix. From the
trusted local publisher, send the complete inventory, durable URI, receipt ID,
publisher, timestamp, and `publication.acknowledged=true` to
`POST /api/v2/executions/{id}/outputs/verify`.

Expected immutable Core artifacts are `manifest`, `source_graph`,
`patched_graph`, `physical_graph`, and the output inventory, each with a
non-empty SHA-256 and size. Only the atomic output-verification transition may
set the aggregate execution to succeeded.

## 9. Approval D: exact cleanup

Before cleanup, prove that the outer ID and every recorded child ID are terminal
or absent, retain sanitized receipts/logs/hashes/inventory/provenance, and verify
the durable outputs. Remove only the ledger-recorded session directory and
UUID-named staging files beneath the dedicated DLG root.

Never clean `/`, the DLG root itself, a username, an unresolved variable, or a
glob. Restore the original project configuration, disable real backends, keep
live Slurm metrics probes off, and retain Core's execution, source, observation,
artifact, and provenance records.

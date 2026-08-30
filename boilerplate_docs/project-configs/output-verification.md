# Output verification and publication

The Core output contract is project-neutral. A production project declares the
durable products that must exist before an execution can succeed:

```yaml
output_verification:
  required: true
  inventory_schema: beampipe-output-inventory/v1
  expected_patterns:
    - "**/result.bin"
```

This policy is copied into the execution ledger when the execution is created.
Changing or activating a later project revision cannot weaken an in-flight
execution. A graph that deliberately produces no durable products must set
`required: false`; it is not valid to enable verification with an empty pattern
list.

`expected_patterns` contains unique, safe relative globs. Core supports `**`,
`*`, and `?`; `**` must be a complete path component. Absolute paths,
traversal, backslashes, character classes, brace expansion, embedded `**`, and
runs of three or more stars are rejected. Patterns are project data. Core assigns no
meaning to a survey name, archive identifier, suffix, or scientific product
class. For example, the bundled WALLABY project uses image and weight patterns,
whereas the neutral example requires `**/result.bin`.

## Publication topology

For the EAGLE editor workflow, real component screenshots, and exact port
wiring, start with [Preparing DALiuGE graphs](preparing-daliuge-graphs.md).

The terminal `beampipe-publish` DALiuGE application belongs to the standalone
`beampipe-palette` package. Its initial storage adapters support an approved
project filesystem and S3-compatible object storage. The Core report contract
can also represent future NGAS or HTTPS-backed adapters, but their URI support
does not imply that the standalone package implements them yet.

`beampipe-publish` is a native DALiuGE barrier application, not a PyFunc with
runtime details exposed as graph arguments. Its logical-graph contract is kept
deliberately small:

- one `completion` input DROP, connected to the pipeline's successful terminal
  marker;
- one `inventory` output FileDROP containing canonical
  `beampipe-output-inventory/v1` JSON; and
- one non-secret project setting, `expected_patterns_json`.

Execution identity, retry generation, output root, durable destination, and the
Core-owned receipt-handoff path come from the submission runtime. They must not
appear as EAGLE fields, graph parameters, or persisted graph values.

The companion `beampipe-ingest` component is native too. Core still injects
the execution manifest through its stable `manifest_path` setting; the app
validates and canonicalizes it before exposing the single `manifest_bytes`
output. Neither component carries executable function source in the graph.

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Output publication flows from pipeline products through a terminal publisher and durable storage to the Core verification ledger">
  <div class="bp-flow-node" data-tone="cyan"><span>DALiuGE</span><strong>pipeline products</strong><small>execution workspace</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>PUBLISH</span><strong>beampipe-publish</strong><small>upload + re-read</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>STORAGE</span><strong>durable objects</strong><small>filesystem or S3-compatible</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>CORE</span><strong>verified inventory</strong><small>immutable ledger evidence</small></div>
</div>

For a remote Slurm execution, the application must, in this order:

1. Upload only the execution's selected output paths.
2. Re-read, checksum, or otherwise verify every durable object through the
   destination adapter.
3. Build `beampipe-output-inventory/v1`, atomically write it to the DALiuGE
   inventory FileDROP, re-read it, and fsync it.
4. Atomically write the byte-identical report to the fixed, execution-scoped
   handoff path beneath the remote DALiuGE session and re-read it.
5. Finish the graph. After Slurm reports `COMPLETED`, Core retrieves the handoff
   report over the same authenticated SSH/SFTP connection used for scheduler
   control, validates it, and commits it locally.

The fixed handoff is
`.beampipe/publication/attempt-<retry_count>/beampipe-output-inventory.json`
beneath the recorded `remote_session_dir`. The attempt directory preserves
immutable evidence when a retry reuses the stable DALiuGE session name. The
submission runtime supplies its absolute path as
`BEAMPIPE_OUTPUT_INVENTORY_HANDOFF_PATH`; project graphs cannot choose or
override it. The output DROP remains useful to the graph and its logs, but only
Core's committed inventory artifact is authoritative ledger evidence.

This inverted flow deliberately requires no route from a Setonix compute node
back to an operator laptop, no reverse tunnel, and no Core credential on the
remote system. A transient SFTP or filesystem visibility error leaves the
execution at `output_verification` and is retried by the recurring Slurm
reconciler. It never resubmits the compute job.

Deployment is fail-closed unless Core has an authenticated control-plane path
to retrieve the fixed handoff. Core never embeds a token in graph JSON,
translator input, scheduler files, or ordinary graph parameters. There is no
publisher callback endpoint and no publisher capability to deliver.

## Trusted publication report

The SSH pull reconciler passes the report through Core's internal trusted
verification service. The report JSON is:

```json
{
  "execution_id": "01994d50-1234-7abc-8def-0123456789ab",
  "execution_attempt": 0,
  "schema": "beampipe-output-inventory/v1",
  "patterns": ["**/result.bin"],
  "pattern_counts": {"**/result.bin": 1},
  "products": [
    {
      "path": "source-a/result.bin",
      "bytes": 1234,
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ],
  "inventory_sha256": "...",
  "durable_destination_uri": "s3://project-results/run-01",
  "publication": {
    "acknowledged": true,
    "publisher": "beampipe-publish/s3",
    "receipt_id": "publication-01",
    "published_at": "2026-08-25T00:00:00Z"
  }
}
```

`execution_id` must equal the ledger UUID and `execution_attempt` must equal the
execution's current `retry_count`. Both are part of the canonical publication
report and prevent a receipt from another run or retry generation being reused.
`inventory_sha256` is SHA-256 over compact
JSON for the `products` array with
object keys sorted (`bytes`, `path`, `sha256`) and array order preserved. The
request's patterns must exactly equal the pinned `expected_patterns`. Core
recomputes every pattern's match count from `products`; every expected pattern
must match at least one product and the claimed positive count must be exact.

Core also requires at least one non-empty product, lowercase SHA-256 values,
unique safe relative paths, and an `s3`, `gs`, `https`, or absolute `file`
destination URI. The endpoint has a 32 MiB request limit and accepts at most
100,000 products. It stores the full report as the immutable
`output_inventory` execution artifact. The artifact `sha256` and `size_bytes`
describe the canonical compact, recursively sorted-key report JSON;
`inventory_sha256` remains separately recorded in the report and artifact
metadata.

## Ordering and retry behavior

The pull flow is intentionally backend-first. Scheduler failure wins and
Core never accepts a handoff as proof of successful computation. Scheduler
success moves the execution to the output gate; Core then reads the exact
session-scoped path, rejects symlinks and non-regular files, enforces the
32 MiB bound while reading, and applies the same schema, attempt, pattern,
product, digest, and destination checks on every report. Exact re-reads are
idempotent. A transient transport or read failure preserves scheduler success,
records a redacted observation, and retries during the next reconciliation
tick; it cannot cause another submission.

## Trust boundary

Core validates policy, report shape, canonical inventory digest, destination
URI, execution generation, and publication acknowledgement. It binds retrieval
trust to the pinned SSH target, verified host key, authenticated remote account,
recorded UUID-scoped session directory, and exact handoff path. It deliberately
does not receive project storage credentials and cannot independently re-read
destination objects. The publisher's post-upload verification is therefore the
durable-publication trust root. Keep destination credentials inside the
publisher's deployment trust boundary and give them only the permissions needed
for the selected execution destination.

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

The terminal `beampipe-publish` DALiuGE application belongs to the standalone
`beampipe-pallette` package. Its initial storage adapters support an approved
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

Execution identity, retry generation, output root, durable destination, Core
callback URL, and publisher capability come from the submission runtime. They
must not appear as EAGLE fields, graph parameters, or persisted graph values.

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

The application must, in this order:

1. Upload only the execution's selected output paths.
2. Re-read, checksum, or otherwise verify every durable object through the
   destination adapter.
3. Build `beampipe-output-inventory/v1`, atomically write it to the DALiuGE
   inventory FileDROP, re-read it, and fsync it.
4. Send the byte-identical report to
   `POST /api/v2/executions/{id}/outputs/verify` with the execution-scoped
   publisher capability.

Only after Core returns the minimal execution/artifact acknowledgement may the
application finish and allow DALiuGE to complete the inventory DROP. This keeps
the graph causal without making Core credentials or callback plumbing visible
in EAGLE.

The output DROP is useful to the graph and its logs, but only Core's committed
inventory artifact is authoritative ledger evidence.

Remote REST deployment is fail-closed unless the worker and DALiuGE runtime
share a private credential-file path or an approved secret broker. Core does
not fall back to embedding the capability in graph JSON, translator input, or
an ordinary environment value when private delivery is unavailable.

## Execution-scoped publisher capability

Never give a graph a Core superuser token. Immediately before submission, a
superuser or the trusted submission worker issues one publisher capability for
the current execution attempt. The operator API is:

```http
POST /api/v2/executions/{execution_id}/outputs/publisher-token
Authorization: Bearer <SUPERUSER_ACCESS_TOKEN>
Content-Type: application/json

{"ttl_seconds":21600}
```

The default lifetime is six hours; the accepted range is five minutes through
24 hours. Automated submission should derive the lifetime from expected queue
delay, pinned outer wall time, and a small publication grace period, while
remaining within that cap.

For automatic Slurm delivery, pin that choice in the deployment profile as
`publication.credential_ttl_minutes`. Core requires 5–1440 minutes and rejects
a value shorter than the effective outer wall time plus 30 minutes. The
remaining allowance is the operator's queue-delay budget; increase it for a
busy partition instead of relying on a hidden default.

The response returns `access_token` exactly once and includes the credential
ID, expiry, execution attempt, audience
`beampipe-output-verification`, and exact scope
`execution:<uuid>:verify_outputs`. It carries `Cache-Control: no-store` and
`Pragma: no-cache`. Core stores only the token's SHA-256 digest.

Treat delivery as runtime secret handling:

- write the plaintext only to an execution-scoped, mode-`0600` runtime secret
  file (or an equivalent non-persisted secret mount);
- give the publisher the secret path, not the token as a command argument;
- retain that exact private file while DALiuGE may retry a post-return failure;
- never put the plaintext in a project config, manifest, logical graph,
  physical graph, INI, `sbatch` script, artifact, provenance payload, or log.

The token is single-purpose and Core accepts only an exact replay after its
first successful use. Terminal reconciliation revokes it. A Slurm session can
therefore retain the private file long enough for DALiuGE retries without
granting a second publication. Automatic removal on the outer job's `EXIT` is
not implemented yet; operators should treat stale session secret directories
as a cleanup residual and remove them under their normal workspace-retention
policy. Do not add an early graph cleanup node, because DALiuGE can retry the
publisher after the application has returned.

The capability is bound to the execution UUID, action, audience, and current
`retry_count`. Retry, cancellation, abandonment, or a terminal transition
revokes outstanding credentials. A credential from an earlier attempt cannot
verify a later attempt.

## Trusted publication report

The verification endpoint accepts either the matching publisher capability or
a superuser access token. Production graph code should always use the scoped
capability. Its JSON body is:

```json
{
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

`execution_attempt` must equal the execution's current `retry_count`; it is
part of the canonical publication report and prevents a receipt from one retry
generation being reused by another. `inventory_sha256` is SHA-256 over compact
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

Publication and scheduler observation may arrive in either order:

- **Receipt first:** while DALiuGE or the scheduler is active, a scoped
  publisher atomically stores the immutable inventory, sets
  `output_state: verified`, and records verification provenance. The aggregate
  execution remains non-terminal. A later successful backend poll finalizes
  sources and completes the execution.
- **Backend success first:** successful backend observation leaves the execution
  waiting at the output gate. A valid report then stores the inventory,
  finalizes sources, and completes the execution in the same transaction.

A later backend failure always wins, even when a receipt arrived first. Core
retains the output evidence but marks the aggregate execution failed and does
not finalize successful source signatures.

Capability consumption and report binding happen in the same database
transaction. If the publisher loses the `200` response, an exact retry with the
same capability and canonical report returns the already committed artifact
without another mutation, including after a terminal poll revokes the
credential. A revoked credential that was never consumed is unusable. Reusing a
consumed capability with a changed report or a different execution is rejected.

## Trust boundary

Core validates policy, report shape, canonical inventory digest, destination
URI, capability authorization, and publication acknowledgement. It deliberately
does not receive project storage credentials and cannot independently re-read
destination objects. The publisher's post-upload verification is therefore the
durable-publication trust root. Keep destination credentials inside the
publisher's deployment trust boundary and give them only the permissions needed
for the selected execution destination.

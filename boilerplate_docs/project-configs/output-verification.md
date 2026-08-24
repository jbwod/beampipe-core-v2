# Output verification and publication

The Core output contract is project-neutral. Production graphs can hold an
execution open until their products have been verified and durably published:

```yaml
output_verification:
  required: true
  inventory_schema: beampipe-output-inventory/v1
```

This policy is copied into the execution ledger when the execution is created.
Changing or activating a later project revision cannot weaken an in-flight
execution. The no-download test graph sets `required: false` explicitly because
it intentionally produces no downloadable products.

`beampipe-output-inventory/v1` is the generic v1 schema. Product paths and
optional pattern rules are project data; Core does not require a survey name,
archive identifier, file suffix, or scientific product class.

When required, successful DALiuGE/scheduler completion leaves the execution in
`running` with `output_state: pending`. It cannot reach terminal success until a
trusted publisher submits `POST /api/v2/executions/{id}/outputs/verify` and the
inventory artifact and ledger transition commit atomically.

## Trusted publication report

The endpoint is authenticated and restricted to superusers. Its JSON body uses:

```json
{
  "schema": "beampipe-output-inventory/v1",
  "products": [
    {
      "path": "source-a/result.bin",
      "bytes": 1234,
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ],
  "inventory_sha256": "...",
  "durable_destination_uri": "file:///durable/project/run-01",
  "publication": {
    "acknowledged": true,
    "publisher": "project-publisher",
    "receipt_id": "publication-01",
    "published_at": "2026-08-22T00:00:00Z"
  }
}
```

`inventory_sha256` is SHA-256 over compact JSON for the `products` array with
object keys sorted (`bytes`, `path`, `sha256`) and array order preserved. Projects
may additionally supply `patterns` and a positive `pattern_counts` entry for
each pattern. Core requires at least one non-empty product, lowercase SHA-256
values, unique safe relative paths, and an `s3`, `gs`, `https`, or absolute
`file` destination URI. It stores the full report as the immutable
`output_inventory` execution artifact. The artifact `sha256` and `size_bytes`
describe the canonical compact, recursively sorted-key report JSON;
`inventory_sha256` remains separately recorded in the report and artifact
metadata.

## Trust boundary

Core validates the schema, path/size/hash shape, canonical inventory digest,
durable destination URI, and authenticated publication acknowledgement. It does
not have storage credentials and therefore cannot independently read and re-hash
objects at the destination. The trusted publisher remains responsible for
re-hashing destination objects after durable publication before it sends the
acknowledgement. Protect superuser credentials and run the publisher in the
deployment trust boundary; an acknowledgement is the durable-publication trust
root.

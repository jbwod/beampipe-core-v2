# Output verification and publication

Production graphs can hold an execution open until their science products have
been verified and durably published:

```yaml
output_verification:
  required: true
  inventory_schema: wallaby-hires-output-inventory/v1
```

This policy is copied into the execution ledger when the execution is created.
Changing or activating a later project revision cannot weaken an in-flight
execution. The no-download test graph sets `required: false` explicitly because
it intentionally produces no downloadable products.

When required, successful DALiuGE/scheduler completion leaves the execution in
`running` with `output_state: pending`. It cannot reach terminal success until a
trusted publisher submits `POST /api/v2/executions/{id}/outputs/verify` and the
inventory artifact and ledger transition commit atomically.

## Trusted publication report

The endpoint is authenticated and restricted to superusers. Its JSON body uses:

```json
{
  "schema": "wallaby-hires-output-inventory/v1",
  "products": [
    {
      "path": "HIPASSJ1318-21/image.final_mosaic.fits",
      "bytes": 1234,
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ],
  "inventory_sha256": "...",
  "durable_destination_uri": "file:///durable/wallaby/run-01",
  "publication": {
    "acknowledged": true,
    "publisher": "wallaby-publisher",
    "receipt_id": "publication-01",
    "published_at": "2026-08-22T00:00:00Z"
  }
}
```

`inventory_sha256` is SHA-256 over compact JSON for the `products` array with
object keys sorted (`bytes`, `path`, `sha256`) and array order preserved. The API
requires at least one non-empty product, lowercase SHA-256 values, unique safe
relative paths, and an `s3`, `gs`, `https`, or absolute `file` destination URI.
It stores the full report as the immutable `output_inventory` execution artifact.

## Trust boundary

Core validates the schema, path/size/hash shape, canonical inventory digest,
durable destination URI, and authenticated publication acknowledgement. It does
not have storage credentials and therefore cannot independently read and re-hash
objects at the destination. The trusted publisher remains responsible for
re-hashing destination objects after durable publication before it sends the
acknowledgement. Protect superuser credentials and run the publisher in the
deployment trust boundary; an acknowledgement is the durable-publication trust
root.

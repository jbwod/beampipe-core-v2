# Graph preparation

Graph preparation combines a generated manifest with a logical DALiuGE graph, applies deterministic patches, and stores checksummed source and patched artifacts before translation.

## Patch a node

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: Scatter/GenericScatterApp/Record
    set:
      num_of_copies: "$count(groups[].records[])"
```

`match.kind` currently supports `node_name`. A missing or ambiguous target is an error. Values can be literals or manifest expressions:

| Expression | Result |
|---|---|
| `$count(path)` | number of selected values |
| `$sum(path)` | numeric sum of selected values |

Use a literal patch for runtime compatibility only when the installed application contract requires it:

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: process_CSV_str
    set:
      output_parser: pickle
```

Pin and record the runtime package version alongside such a patch. Graph configuration should not hide an unexplained environment mismatch.

## Manifest injection

Graphs using the `beampipe-ingest` palette can receive the generated manifest through the node's `manifest_path` field. Beampipe validates the node and field, injects read-only configuration, and records the resulting artifact.

Manifest templates may read prepared record fields directly or through logical
`flags.*` references. Persisted discovery values are resolved consistently in
either form. Expression paths address the emitted manifest shape, so projects
that explicitly rename `groups` or `records` must use those emitted names.

## Preview

```bash
beampipe graph prepare \
  --project minimal_survey \
  --source source-1
```

For an existing execution:

```bash
beampipe graph diff --execution "$EXECUTION_ID"
beampipe daliuge translate --execution "$EXECUTION_ID"
```

Before submission, verify the project revision, graph source, patch target count, changed fields, manifest checksum, source graph checksum, and patched graph checksum. A remote branch URL is mutable until fetched; use an immutable URL or retain and verify the expected hash for release qualification.

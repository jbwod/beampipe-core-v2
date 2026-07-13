# DALiuGE graph patches

Graph patches are the final deterministic graph-shaping step before translation and deployment. The v2 YAML key is `graph_patches`; every match, expression result, changed field, and artifact checksum remains inspectable.

## Preparation flow

<div class="bp-flow-diagram bp-flow-diagram--wide" role="img" aria-label="Logical graph and manifest are validated, patched, checksummed, translated, and deployed">
  <div class="bp-flow-node" data-tone="cyan"><span>INPUT</span><strong>logical graph</strong><small>URL or path</small></div>
  <span class="bp-flow-link" aria-hidden="true">+</span>
  <div class="bp-flow-node" data-tone="cyan"><span>CONTEXT</span><strong>manifest</strong><small>pinned artifact</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>VALIDATE</span><strong>patch reducer</strong><small>match + evaluate + set</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>ARTIFACT</span><strong>patched graph</strong><small>diff + sha256</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>EXTERNAL</span><strong>translator</strong><small>then deployment</small></div>
</div>

## Typed shape

Set the scatter count from manifest data:

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: Scatter/GenericScatterApp/Beam
    set:
      num_of_copies: "$count(sbids[].datasets[])"
```

`match.kind` is the closed enum `node_name`. `match.equals` identifies the full DALiuGE node name. Each `set` value is either a literal or one of the supported manifest expressions.

## Expressions

| Expression | Result | Typical use |
|---|---|---|
| `$count(path)` | Number of selected manifest elements | Scatter copies |
| `$sum(path)` | Numeric sum across selected values | Aggregate resource or workload field |

Expressions run against the immutable manifest context. Put archive normalization in typed transforms and reserve WASM for survey logic that cannot be expressed safely in the built-in model.

## Patch diagnostics

Graph preparation reports:

- the patch index and target node;
- whether zero, one, or multiple nodes matched;
- each field before and after mutation;
- expression input path and evaluated value;
- graph checksum before and after mutation;
- validation errors with a structured path and hint.

Precise matches are deliberate. A missing node is an error rather than a silent no-op; an ambiguous match is rejected rather than applied to multiple nodes.

## Manifest injection

Existing graphs can use the `beampipe-ingest` palette. During preparation, Beampipe locates the named node, validates its `manifest_path` field, creates readonly graph configuration, and embeds the generated manifest JSON before translation.

| Contract element | Meaning |
|---|---|
| `beampipe-ingest` node | Marker that the graph accepts a Beampipe manifest |
| `manifest_path` | Graph field or path where manifest JSON is exposed |
| Generated manifest | Project-shaped source, SBID, and dataset grouping |

Use injection when graph applications consume the manifest directly. Use ordinary patches for structural settings such as scatter counts.

## Operator preview

```bash
beampipe graph prepare \
  --project wallaby_hires \
  --source WALLABY_J123456-123456
beampipe graph diff --execution "$EXECUTION_ID"
```

Before live submission, confirm the target count, expression values, node/field diff, manifest injection path, and output checksums. Then use [deployment profiles](../architecture/deployment-profiles.md) to select the translator and deployment boundary.

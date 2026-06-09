# DALiuGE Graphs

DALiuGE Graph support covers the final graph-shaping step before translation and deployment. The YAML key is `graph_patches`; the operator-facing goal is to make graph changes deterministic, reviewable, and tied to the generated manifest.

## Flow

<div class="terminal-diagram terminal-diagram--center">
<pre>project config
     |
     v
manifest build
     |
     v
apply graph_patches
     |
     v
Translator Manager
     |
     v
REST / Slurm deployment</pre>
</div>

## Example

Set the scatter count from manifest data:

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: Scatter/GenericScatterApp/Beam
    set:
      num_of_copies: "$count(sbids[].datasets[])"
```

The expression runs against the manifest context. In this example, the graph receives one copy per discovered dataset across SBID groups.

## Matching

| Match kind | Purpose |
|------------|---------|
| `node_name` | Match a graph node by full DALiuGE node name |
| Additional kinds | Add only when the graph format and validation rules support them |

Keep matches precise. A patch that silently matches multiple nodes can make execution hard to audit.

## Expressions

Values beginning with `$` are evaluated against the manifest context. Use expressions for counts and manifest-derived scatter settings; keep complex survey logic in transforms or WASM hooks.

| Expression | Use |
|------------|-----|
| `$count(path)` | Count manifest elements |
| `$sum(path)` | Sum numeric manifest values |

## beampipe-ingest palette

Existing DALiuGE graphs can include the `beampipe-ingest` palette. At submit time, beampipe looks for a node named `beampipe-ingest` with a `manifest_path` field, creates a readonly graph configuration, and embeds the generated manifest JSON before translation.

Typical graph contract:

| Field | Meaning |
|-------|---------|
| `beampipe-ingest` node | Marker that the graph expects beampipe manifest injection |
| `manifest_path` | Path or graph parameter where the manifest JSON should be placed |
| Generated manifest | Source/SBID/dataset grouping from project config discovery |

Use the palette when the science graph should consume beampipe-generated manifest data directly. Use plain patches when the graph only needs structural changes such as scatter counts.

## Operator checks

| Check | Why |
|-------|-----|
| Patch target exists | Prevents silent no-op graph mutation |
| Expression output is expected | Avoids wrong scatter size or missing parameters |
| Manifest injection path is readable | Ensures graph apps can load the manifest |
| Dry execution passes | Confirms graph preparation before real staging/submission |

Next: use [Deployment profiles](../architecture/deployment-profiles.md) to select the backend that receives the prepared graph.

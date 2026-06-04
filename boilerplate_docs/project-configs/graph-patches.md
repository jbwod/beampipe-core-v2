# DALiuGE Graphs

DALiuGE graph support covers the final graph-shaping step before translation and deployment. Graph patches mutate the logical graph after manifest construction and before DALiuGE translation. They are deterministic YAML rules, so operators can review how a project config changes the graph.

## Flow

<div class="terminal-diagram">
<pre>logical graph
     |
     v
embed manifest JSON
     |
     v
apply graph_patches
     |
     v
Translator Manager
     |
     v
PGT / REST / Slurm deployment</pre>
</div>

## Example

The WALLABY reference config sets scatter copies from the manifest dataset count:

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: Scatter/GenericScatterApp/Beam
    set:
      num_of_copies: "$count(sbids[].datasets[])"
```

## Matching

Patch matching is intentionally small and explicit. Prefer stable node names over positional assumptions.

| Field | Purpose |
|-------|---------|
| `match.kind` | Match strategy |
| `match.equals` | Expected node identifier/value |
| `set` | Properties to write on the matched node |

## Expressions

Values beginning with `$` are evaluated against the manifest context. Use them for counts and manifest-derived scatter settings; keep complex survey logic in config transforms or WASM hooks.

## beampipe-ingest palette

Existing DALiuGE graphs can opt into beampipe by importing the `beampipe-ingest` palette in EAGLE and adding the `beampipe-ingest` PyFunc drop to the logical graph. The drop is the handoff point between the beampipe execution manifest and the translated DALiuGE graph.

At submit time, beampipe looks for a node named `beampipe-ingest` with a string field named `manifest_path`. When that node exists, beampipe creates a readonly graph configuration, embeds the generated manifest JSON into the `manifest_path` field, and sets the configuration as the active graph config before translation.

```yaml
graph:
  ingest_node:
    name: beampipe-ingest
    manifest_field: manifest_path
```

Operator notes:

| Item | Requirement |
|------|-------------|
| Palette | Import the `beampipe-ingest` palette into EAGLE before editing the graph |
| Node name | Keep the graph node name exactly `beampipe-ingest` |
| Field name | Keep the manifest field exactly `manifest_path` |
| Field type | Use a string-compatible field because beampipe writes manifest JSON text |
| Graph config | beampipe creates a readonly `beampipe-core Auto-generated Manifest` graph config |

The embedded manifest excludes internal `graph_overrides`; those overrides are consumed by beampipe while patching the graph and are not passed to the ingest drop. This keeps the drop focused on the science-run manifest that the graph needs at runtime.

## Operator checks

Run validation before upload:

```bash
beampipe project validate -f config/wallaby_hires.v1.yaml
```

Then create a dry-run execution with `do_stage=false` and `do_submit=false` to inspect the built manifest and patched graph path without contacting real backends.

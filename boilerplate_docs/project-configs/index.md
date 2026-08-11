# Project YAML

Project config is immutable, dynamically loaded survey policy. It defines source identity, TAP queries, metadata preparation, manifests, graph preparation, and scheduler automation. No project query is hardcoded in the Rust worker.

## Data flow

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Project YAML drives query rendering metadata normalization manifest generation graph patches and automation">
  <div class="bp-flow-node" data-tone="cyan"><span>YAML</span><strong>identity + queries</strong><small>survey policy</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>TAP</span><strong>rows</strong><small>CASDA + VizieR</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>PREPARE</span><strong>metadata</strong><small>map + flag + sign</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>BUILD</span><strong>manifest + graph</strong><small>immutable artifacts</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>POLICY</span><strong>automation</strong><small>admit + execute</small></div>
</div>

## Start from an example

- `config/wallaby_hires.v2.yaml`: production-shaped WALLABY discovery and Slurm automation.
- `config/examples/minimal_survey.v2.yaml`: smallest single-archive example.

```bash
beampipe project validate -f config/wallaby_hires.v2.yaml
beampipe project explain -f config/wallaby_hires.v2.yaml
beampipe project render -f config/wallaby_hires.v2.yaml
beampipe project add -f config/wallaby_hires.v2.yaml
```

`validate` returns structured diagnostics and a canonical SHA-256. `add` stores a new immutable revision and activates it. Existing executions retain their pinned revision.

## Document shape

```yaml
apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata: {}
definitions: {}
source_identity: {}
adapters: {}
graph: {}
discovery: {}
manifest: {}
graph_patches: []
automation: {}
extension: {}
```

| Section | Owns |
|---|---|
| `metadata` | stable project ID and description |
| `definitions`, `source_identity` | named transforms and query variables |
| `adapters` | required TAP adapters, endpoints, retry/timeout policy |
| `discovery` | project-specific ADQL, enrichments, mappings, flags, signature |
| `manifest` | source/SBID/dataset grouping and output templates |
| `graph`, `graph_patches` | logical graph source and deterministic mutations |
| `automation` | discovery cadence and execution admission limits |
| `extension` | optional pinned WASM hooks |

## Dynamic TAP queries

Queries and enrichments live in YAML and are rendered from source identity plus prior results:

```yaml
source_identity:
  canonical: source_identifier
  template_vars:
    source_identifier:
      from: canonical
    source_name:
      transform: hipass_source_name

adapters:
  required: [casda, vizier]
  tap:
    timeout_seconds: 90
    retries: 2
    fail_open: false

discovery:
  queries:
    - name: visibility
      adapter: casda
      template: |
        SELECT o.* FROM ivoa.obscore o
        WHERE o.filename LIKE '{source_identifier}%'
    - name: ra_dec_vsys
      adapter: vizier
      template: |
        SELECT HIPASS, RAJ2000, DEJ2000, RVmom
        FROM "VIII/73/hicat" WHERE HIPASS = '{source_name}'
  enrichments:
    - name: sbid_to_eval_file
      adapter: casda
      template: |
        SELECT * FROM casda.observation_evaluation_file WHERE sbid = '{sbid}'
```

Project-level `casda_tap_url` and `vizier_tap_url` can override runtime defaults. Keep credentials outside YAML.

## Metadata and signatures

`prepare_metadata` is nested under `discovery`:

```yaml
discovery:
  prepare_metadata:
    field_map:
      dataset_id:
        from: filename
      visibility_filename:
        from: filename
      sbid:
        from: obs_id
        transform: normalized_sbid
    discovery_flags:
      ra_dec_vsys_complete:
        from: enrichments.ra_dec_vsys
        transform: has_rows
    signature:
      exclude_fields: [access_url, filesize, t_max, t_min]
      include_discovery_flags: true
```

Every prepared dataset needs `sbid` and either `dataset_id` or `visibility_filename`. Invalid rows fail the whole persistence transaction. Exclude volatile fields only when their changes should not trigger another workflow.

## Manifest and graph

```yaml
manifest:
  group_by: [source_identifier, sbid]
  source_template:
    source_identifier: "{source_identifier}"
    ra_string: "{flags.ra_string}"
    dec_string: "{flags.dec_string}"
    vsys: "{flags.vsys}"

graph:
  url: https://example.org/pinned/wallaby.graph

graph_patches:
  - match:
      kind: node_name
      equals: Scatter/GenericScatterApp/Beam
    set:
      num_of_copies: "$count(sbids[].datasets[])"
```

Manifest templates resolve both logical `flags.*` values and the flat persisted fields produced by discovery. Graph bytes are checksummed and stored on the execution, but remote branch URLs remain mutable before preparation; use immutable URLs or externally verified hashes for qualification.

## Automation

```yaml
automation:
  discovery:
    enabled: true
    batch_size: 10
    stale_after_hours: 24
  execution:
    enabled: true
    archive_name: casda
    max_sources_per_execution: 1
    tick_execution_run_limit: 1
    concurrent_execution_run_limit: 1
    deployment_profile_name: setonix
```

Project limits express survey policy. Environment `BEAMPIPE_SHAPING_*` settings and profile concurrency are additional safety ceilings. Make sure the named profile exists before enabling execution automation.

## Optional WASM

Use WASM only when transforms, templates, and graph patches are insufficient. Supported hooks are `prepare_metadata`, `manifest`, and `graph_patches`.

```bash
beampipe wasm upload \
  --config-id PROJECT_CONFIG_UUID \
  -f target/wasm32-wasip1/release/project_hooks.wasm
```

Reference the returned digest:

```yaml
extension:
  wasm_sha256: "<sha256>"
  hooks: [prepare_metadata]
```

Hooks must be deterministic and secret-free. They are project logic, not an escape hatch for network calls or deployment behavior.

Continue with [Transforms](transforms.md) and [Graph preparation](graph-patches.md).

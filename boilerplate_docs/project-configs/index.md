# Project config YAML

Project configs are the survey configuration surface for beampipe-core. They describe source identity, archive queries, metadata preparation, manifest shape, DALiuGE Graphs, scheduler automation, and optional WASM hooks.

## Anatomy

```yaml
apiVersion: beampipe.dev/v1
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

| Section | Purpose |
|---------|---------|
| `apiVersion` | Config API version, currently `beampipe.dev/v1` |
| `kind` | Must be `ProjectConfig` |
| `metadata` | Project ID and description |
| `definitions` | Named reusable transforms |
| `source_identity` | Template variables derived from the canonical source identifier |
| `adapters` | Required archive adapters and TAP policy |
| `graph` | Logical graph URL or local path |
| `discovery` | TAP query templates, enrichments, field mapping, flags, signatures |
| `manifest` | Manifest grouping and JSON templates |
| `graph_patches` | YAML key for DALiuGE Graph mutations before translation |
| `automation` | Discovery and execution scheduler policy |
| `extension` | Optional WASM hook linkage |

Validate before upload:

```bash
beampipe project validate -f config/wallaby_hires.v1.yaml
```

Upload through the API:

```bash
curl -s -X POST "$BASE/api/v2/project-configs" \
  -H "$AUTH" \
  -H 'Content-Type: application/x-yaml' \
  --data-binary @config/wallaby_hires.v1.yaml | jq .
```

## Metadata

```yaml
metadata:
  id: wallaby_hires
  description: WALLABY HiRes CASDA pipeline reference config
```

`metadata.id` is the `project_module` used in source registration, executions, events, and deployment profile scoping.

## Definitions and transforms

Definitions hold named transforms. Give transforms survey-meaningful names so field maps stay readable.

```yaml
definitions:
  transforms:
    hipass_source_name:
      kind: strip_prefix
      prefix: HIPASS
    askap_sbid:
      kind: extract_digits
    scan_id_from_did:
      kind: split_last
      separators: ["/", ":", "#"]
    has_rows:
      kind: is_present
    normalized_sbid:
      kind: chain
      steps: [askap_sbid, trim]
    trim:
      kind: trim
```

This WALLABY example converts `HIPASSJ1313-15` into VizieR query variables, normalizes ASKAP SBIDs, splits scan IDs from publisher DIDs, and converts enrichment rows into readiness flags. See [Transforms](transforms.md) for the full reference.

## Source identity

```yaml
source_identity:
  canonical: source_identifier
  template_vars:
    source_identifier:
      from: canonical
    source_name:
      transform: hipass_source_name
```

For a registered source `HIPASSJ1313-15`, `{source_identifier}` remains `HIPASSJ1313-15` and `{source_name}` becomes `J1313-15`. This lets CASDA and VizieR use different query formats without changing source registration.

## Adapters

```yaml
adapters:
  required:
    - casda
    - vizier
  tap:
    timeout_seconds: 90
    retries: 2
    fail_open: false
```

| Field | Default | Purpose |
|-------|---------|---------|
| `required` | `[]` | Adapters that must be available |
| `casda_tap_url` | env/default | Optional CASDA TAP override |
| `vizier_tap_url` | env/default | Optional VizieR TAP override |
| `tap.timeout_seconds` | `30` | Query timeout |
| `tap.retries` | `1` | Retry count |
| `tap.fail_open` | `false` | Allow degraded discovery when adapter checks fail |

## Graph

```yaml
graph:
  url: https://raw.githubusercontent.com/jbwod/wallaby-hires-beampipe/refs/heads/main/dlg-graphs/wallaby-hires_deploy-setonix-beampipe.graph
```

Use `url` for remote graph sources or `path` for local graph files available to the worker.

## Discovery

WALLABY uses CASDA for visibility metadata and VizieR for catalogue enrichment.

```yaml
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
        SELECT HIPASS, RAJ2000, DEJ2000
        FROM "VIII/73/hicat" WHERE HIPASS = '{source_name}'
  enrichments:
    - name: sbid_to_eval_file
      adapter: casda
      template: |
        SELECT * FROM casda.observation_evaluation_file WHERE sbid = '{sbid}'
```

Field mapping turns TAP rows into persisted archive metadata. `from` reads a field from the current TAP row or enrichment result; `transform` normalizes it before storage.

```yaml
prepare_metadata:
  field_map:
    source_identifier:
      from: source_identifier
    dataset_id:
      from: filename
    sbid:
      from: obs_id
      transform: normalized_sbid
    scan_id:
      from: obs_publisher_did
      transform: scan_id_from_did
  discovery_flags:
    ra_dec_vsys_complete:
      from: enrichments.ra_dec_vsys
      transform: has_rows
  signature:
    exclude_fields:
      - access_url
      - filesize
      - t_max
      - t_min
    include_discovery_flags: true
```

Discovery signatures decide whether source metadata changed enough to trigger future execution. Exclude volatile fields when changes should not trigger reruns.

## Manifest

```yaml
manifest:
  group_by:
    - source_identifier
    - sbid
  source_template:
    source_identifier: "{source_identifier}"
    ra_string: "{flags.ra_string}"
    dec_string: "{flags.dec_string}"
    vsys: "{flags.vsys}"
```

`group_by` controls how metadata rows become manifest groups. Templates can read metadata fields, discovery flags, and staging-derived values.

## DALiuGE Graphs

The YAML key is still `graph_patches`, but the operator-facing concept is DALiuGE Graph preparation.

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: Scatter/GenericScatterApp/Beam
    set:
      num_of_copies: "$count(sbids[].datasets[])"
```

Patches are applied after manifest generation and before DALiuGE translation. Graphs that include the `beampipe-ingest` palette can also receive the generated manifest through a `beampipe-ingest` node with a `manifest_path` field.

## Automation

```yaml
automation:
  discovery:
    enabled: true
    tick_discovery_source_limit: 1000
    batch_size: 10
    tick_discovery_batch_limit: 100
    concurrent_discovery_batch_limit: 24
    stale_after_hours: 24
  execution:
    enabled: true
    archive_name: casda
    max_sources_per_execution: 1
    tick_execution_source_limit: 1000
    tick_execution_run_limit: 50
    min_sources_to_trigger: 1
    max_wait_minutes: 1440
    claim_ttl_minutes: 180
    concurrent_execution_run_limit: 10
    deployment_profile_name: slurm-remote
```

Project automation limits combine with global `BEAMPIPE_SHAPING_*` environment variables. Use project config for survey policy and environment variables for cluster-wide safety caps.

## Extension

```yaml
extension:
  wasm_sha256: "<uploaded-module-sha256>"
  hooks:
    - prepare_metadata
    - manifest
```

Use WASM hooks only when transforms, templates, and DALiuGE Graph patches are not expressive enough. Next: review [Transforms](transforms.md) for concrete normalization examples.

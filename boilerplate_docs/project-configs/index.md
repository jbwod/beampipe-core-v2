# Project config YAML

Project configs are the survey configuration surface for beampipe-core. They describe source identity, archive queries, metadata preparation, manifest shape, graph mutation, and scheduler automation.

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
| `graph_patches` | Graph mutations before translation |
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

## Definitions

Definitions hold named transforms. Keep them small, explicit, and reusable.

```yaml
definitions:
  transforms:
    hipass_source_name:
      kind: strip_prefix
      prefix: HIPASS
    normalized_sbid:
      kind: chain
      steps: [askap_sbid, trim]
```

The WALLABY config uses transforms to convert `HIPASSJ1313-15` into query variables, normalize SBIDs, split scan IDs, and convert enrichment results into flags.

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

Discovery SQL templates can then use `{source_identifier}` and `{source_name}`. This keeps source registration stable while still allowing survey-specific archive query formats.

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

Field mapping turns TAP rows into persisted archive metadata:

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

Discovery signatures let beampipe decide whether the source metadata has changed since the last execution.

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

`group_by` controls how rows become manifest groups. Templates can read metadata fields and flags.

## DALiuGE Graphs

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: Scatter/GenericScatterApp/Beam
    set:
      num_of_copies: "$count(sbids[].datasets[])"
```

Graph patches are applied after manifest generation and before DALiuGE translation. Keep them deterministic and easy to audit. Graphs that include the `beampipe-ingest` palette can also receive the generated manifest through a `beampipe-ingest` node with a `manifest_path` field.

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

Use WASM hooks only when transforms, templates, and graph patches are not expressive enough.

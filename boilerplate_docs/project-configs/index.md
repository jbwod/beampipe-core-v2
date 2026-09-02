# Project YAML

Project configuration is immutable, dynamically loaded workflow policy. It
defines source identity, named TAP endpoints, discovery queries, metadata
preparation, staging, manifests, graph preparation, output verification (in progress), and
automation. Core does not assign scientific meaning to a project identifier,
endpoint name, group, or record.

## Start with the neutral example

The smallest complete example is
`config/examples/minimal_survey.v2.yaml`. Its matching graph is
`config/examples/neutral_demo.graph`.

```bash
beampipe project validate -f config/examples/minimal_survey.v2.yaml
beampipe project explain -f config/examples/minimal_survey.v2.yaml
beampipe project render -f config/examples/minimal_survey.v2.yaml
beampipe project add -f config/examples/minimal_survey.v2.yaml
```

The example endpoint uses the reserved `.invalid` domain deliberately. Use it
for validation and the offline acceptance test, or replace it with a real TAP
endpoint before live discovery. `validate` returns structured diagnostics and
a canonical SHA-256. `add` stores and activates a new immutable revision;
existing executions keep their pinned revision.

The WALLABY HiRes bundle is an explicit provider example, not a Core default:

```bash
beampipe setup --yes --runtime docker --sample wallaby-hires
```

## Core identity contract

The neutral hierarchy is:

```text
project_module -> source_identifier -> group_key -> record_id
```

| Name | Meaning |
|---|---|
| `project_module` | Stable project ID from `metadata.id` |
| `source_identifier` | Stable unit scheduled for discovery and execution |
| `group_key` | Project-defined grouping key within one source |
| `record_id` | Stable identity of one prepared archive/catalog record |

`group_key` and `record_id` are the required prepared names. API execution
selections use `groups`; persisted archive metadata responses expose
`group_key`, and each stored group payload contains `records`.

This is a clean-break contract. Older project-specific names such as `sbid`,
`sbids`, `dataset`, and `datasets` are not API aliases. A project may still
emit those words inside its own manifest by setting
`groups_output_field`, `group_key_output_field`, and
`records_output_field` explicitly, as the WALLABY sample does.

## Document shape

```yaml
apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata: {}
definitions: {}
source_identity: {}
adapters: {}
staging: {}
graph: {}
discovery: {}
manifest: {}
graph_patches: []
output_verification: {}
automation: {}
extension: {}
```

| Section | Owns |
|---|---|
| `metadata` | Stable project ID and description |
| `definitions`, `source_identity` | Named transforms and query variables |
| `adapters` | Named TAP endpoints, transport mode, retry, and timeout policy |
| `staging` | Explicit input-staging provider |
| `discovery` | Queries, iteration, result policies, mappings, flags, and signature |
| `manifest` | Source/group/record grouping and project-shaped output templates |
| `graph`, `graph_patches` | Logical graph source and deterministic mutations |
| `output_verification` | Generic durable-product inventory policy |
| `automation` | Discovery cadence and execution admission limits |
| `extension` | Optional pinned WASM hooks |

## Arbitrary adapters and endpoints

Adapter names are project-owned. Every query names an adapter, and the same
name selects its endpoint:

```yaml
adapters:
  required: [catalog, context]
  endpoints:
    catalog:
      url: https://catalog.example.org/tap
      mode: sync_post
    context:
      url: https://context.example.org/tap
      mode: async_job
  tap:
    timeout_seconds: 60
    retries: 1
    fail_open: false
```

Endpoint `mode` is `sync_get`, `sync_post`, or `async_job`. Credentials remain
runtime secrets; they do not belong in project YAML. The names `catalog` and
`context` have no built-in behavior, and neither do provider-specific names
used by sample projects.

## Query order, iteration, and results

The first `discovery.queries` entry is the primary record query. Its row set
feeds `prepare_metadata`; an empty row set means that discovery found no
metadata. Later queries run in declaration order and apply the configured
result policy. Enrichments use the same query type and can iterate over distinct
values prepared from the primary rows:

```yaml
discovery:
  queries:
    - name: records
      adapter: catalog
      template: |
        SELECT object_id, collection_id, access_url
        FROM project_records
        WHERE source_id = '{source_identifier}'
    - name: source_context
      adapter: context
      template: |
        SELECT label FROM source_context
        WHERE source_id = '{source_identifier}'
      result: first
  enrichments:
    - name: collection_context
      adapter: context
      for_each:
        field: group_key
        variable: collection
      template: |
        SELECT label FROM collection_context
        WHERE collection_id = '{collection}'
      result: exactly_one
      required: true
```

`for_each.field` names a target in `prepare_metadata.field_map`. Beampipe
derives its distinct values from primary rows and runs the query once per
value. `variable` defaults to the field name when omitted.

| `result` | Stored query result |
|---|---|
| `many` | All returned rows; this is the default. Empty is allowed only when the query is optional. |
| `first` | The first returned row. Empty becomes `null` only when the query is optional. |
| `exactly_one` | The sole row; any other successful row count is always an error. |

`required: true` makes query or adapter failure fatal and requires at least one
row for `many` or `first`. A failed optional query produces its empty result
shape instead. Keep `fail_open: false` for admission facts unless the project
has an explicit safe degraded mode.

## Prepare and persist metadata

Map provider fields into the neutral record contract:

```yaml
discovery:
  prepare_metadata:
    field_map:
      source_identifier:
        from: source_identifier
      group_key:
        from: collection_id
      record_id:
        from: object_id
    required_fields:
      - access_url
    signature:
      exclude_fields: [last_modified]
      include_discovery_flags: true
```

Every prepared row needs a non-empty `group_key` and `record_id`, plus every
configured `required_fields` entry. Invalid rows fail persistence rather than
creating partial group state. Exclude a volatile field from the discovery
signature only when changing it must not trigger another workflow.

## Select a staging provider

Staging behavior is selected only by `staging.provider`:

```yaml
staging:
  provider: none
```

`none` is the project-neutral pass-through provider. `casda_uws` is the
currently bundled CASDA-specific provider and must be selected explicitly by a
project that uses that protocol. Core does not infer a staging provider from
an adapter name, `archive_name`, or manifest field.

## Shape the manifest

Defaults preserve the neutral words `groups`, `group_key`, and `records`:

```yaml
manifest:
  group_by:
    - source_identifier
  source_template:
    source_identifier: "{source_identifier}"
  record_template:
    record_id: "{record_id}"
    access_url: "{access_url}"
```

Manifest templates can read prepared record fields and logical `flags.*`
values. A project that must satisfy an existing graph contract may rename only
the emitted manifest fields:

```yaml
manifest:
  groups_output_field: batches
  group_key_output_field: batch_id
  records_output_field: inputs
```

These names do not change Core's database or API contract.

## Pin the graph and output contract

```yaml
graph:
  path: config/examples/neutral_demo.graph
  sha256: f1feee266fcdbdc7006e090533b94ec3d8bee14d91b11b7b9dcb22bcc8ef21c3

output_verification:
  required: true
  inventory_schema: beampipe-output-inventory/v1
  expected_patterns:
    - "**/result.bin"
```

A graph may use an immutable URL instead of a path. Every source is bounded,
hashed, and checked against the configured SHA-256 before parsing. Successful
backend completion does not satisfy required output verification (in progress); a trusted
publisher must durably verify at least one product for every pinned pattern and
submit the generic inventory described in
[Output verification (in progress)](output-verification.md).

## Automation

```yaml
automation:
  discovery:
    enabled: true
    batch_size: 10
    tick_discovery_batch_limit: 10
    concurrent_discovery_batch_limit: 4
    stale_after_hours: 24
```

Project limits express workflow policy. Environment `BEAMPIPE_SHAPING_*`
settings and deployment-profile concurrency are additional safety ceilings.
Configure execution automation only after its named deployment profile exists
and has passed its doctor checks.

## Optional WASM

Use WASM only when transforms, templates, and graph patches are insufficient.
Supported hooks are `prepare_metadata`, `manifest`, and `graph_patches`.

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

Hooks must be deterministic and secret-free. They are project logic, not an
escape hatch for network calls or deployment behavior.

Continue with [Transforms](transforms.md) and
[Graph preparation](graph-patches.md).

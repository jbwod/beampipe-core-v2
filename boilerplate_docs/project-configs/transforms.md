# Transforms

Transforms convert source identifiers, TAP fields, and enrichment results into
stable query variables, prepared records, and discovery flags. Define a named
transform once, then reference it by project meaning.

## Define and use

```yaml
definitions:
  transforms:
    trim:
      kind: trim
    lowercase:
      kind: lowercase
    normalized_collection:
      kind: chain
      steps: [trim, lowercase]

source_identity:
  canonical: source_identifier
  template_vars:
    source_name:
      transform: lowercase

discovery:
  prepare_metadata:
    field_map:
      group_key:
        from: collection_name
        transform: normalized_collection
```

For source `SOURCE-A`, `{source_name}` becomes `source-a`. A primary row whose
`collection_name` is ` Batch-01 ` receives `group_key: batch-01`.

## Built-in kinds

| Kind | Parameters | Purpose |
|---|---|---|
| `identity` | none | Explicit pass-through |
| `trim` | none | Remove surrounding whitespace |
| `lowercase`, `uppercase` | none | Normalize case |
| `replace` | `from`, optional `to` | Replace substrings |
| `add_prefix`, `add_suffix` | `prefix` or `suffix` | Construct names |
| `default_if_empty` | `default` | Supply a missing value |
| `chain` | `steps` | Compose named transforms |
| `strip_prefix` | `prefix` | Remove a known namespace prefix |
| `extract_digits` | none | Retain numeric characters from an identifier |
| `split_last` | `separators` | Select the final path or identifier component |
| `is_present` | none | Convert non-empty input into a Boolean flag |
| `degrees_to_hms` | none | Format numeric degrees as hours/minutes/seconds |
| `degrees_to_dms` | none | Format numeric degrees as degrees/minutes/seconds |
| `regex_extract` | `pattern`, optional `group` | Extract a capture from structured text |

Unknown transform names and removed project-specific selectors are rejected.
Provider selection or tie-breaking belongs in a query, enrichment result
policy, or project extension—not in a hidden Core transform.

## Where transforms run

<div class="bp-flow-diagram bp-flow-diagram--animated" role="img" aria-label="Transforms run first on source identity then on metadata fields and discovery flags">
  <div class="bp-flow-node" data-tone="cyan"><span>SOURCE</span><strong>identity</strong><small>query variables</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>TAP</span><strong>row fields</strong><small>provider values</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>MAP</span><strong>records</strong><small>group_key + record_id</small></div>
  <span class="bp-flow-link" aria-hidden="true">+</span>
  <div class="bp-flow-node" data-tone="green"><span>DERIVE</span><strong>flags</strong><small>readiness + manifest</small></div>
</div>

Transforms can be referenced from:

- `source_identity.template_vars.*.transform`;
- `discovery.prepare_metadata.field_map.*.transform`;
- `discovery.prepare_metadata.discovery_flags.*.transform`.

A field-map or discovery-flag transform may be a single named transform or an
inline list of names. A named `chain` is easier to review when reused.

## Recipes

Select the final component of a publisher identifier:

```yaml
record_id_from_did:
  kind: split_last
  separators: ["/", ":", "#"]
```

Extract an integer suffix:

```yaml
partition_number:
  kind: regex_extract
  pattern: "partition[_-]([0-9]+)"
  group: 1
```

Derive a readiness flag from enrichment data:

```yaml
has_context:
  kind: is_present

discovery:
  prepare_metadata:
    discovery_flags:
      context_complete:
        from: enrichments.source_context
        transform: has_context
```

## Validation

```bash
beampipe project validate -f PROJECT.yaml
beampipe project explain -f PROJECT.yaml
```

Validation rejects unknown kinds, missing transform references, absent required
parameters, and invalid chains.

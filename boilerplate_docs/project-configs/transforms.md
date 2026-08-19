# Transforms

Transforms convert source identifiers, TAP fields, and enrichment rows into stable query variables, metadata, and discovery flags. Define named transforms once, then reference them by survey meaning.

## Define and use

```yaml
definitions:
  transforms:
    hipass_source_name:
      kind: strip_prefix
      prefix: HIPASS
    askap_sbid:
      kind: extract_digits
    trim:
      kind: trim
    normalized_sbid:
      kind: chain
      steps: [askap_sbid, trim]

source_identity:
  canonical: source_identifier
  template_vars:
    source_name:
      transform: hipass_source_name

discovery:
  prepare_metadata:
    field_map:
      sbid:
        from: obs_id
        transform: normalized_sbid
```

For `HIPASSJ1313-15`, `{source_name}` becomes `J1313-15`. For `ASKAP-72962`, `sbid` becomes `72962`.

## Built-in kinds

| Kind | Parameters | Purpose |
|---|---|---|
| `identity` | none | explicit pass-through |
| `trim` | none | remove surrounding whitespace |
| `lowercase`, `uppercase` | none | normalize case |
| `replace` | `from`, optional `to` | replace substrings |
| `add_prefix`, `add_suffix` | `prefix` or `suffix` | construct names |
| `default_if_empty` | `default` | supply a missing value |
| `chain` | `steps` | compose named transforms |
| `strip_prefix` | `prefix` | derive catalogue identifiers |
| `extract_digits` | none | normalize SBIDs and numeric IDs |
| `split_last` | `separators` | extract a final path/DID segment |
| `is_present` | none | turn non-empty enrichment data into a flag |
| `regex_extract` | `pattern`, optional `group` | parse structured filenames |
| `select_eval_file_by_size` | none | choose the largest CASDA evaluation-file row by `filesize` |

## Where they run

<div class="bp-flow-diagram bp-flow-diagram--animated" role="img" aria-label="Transforms run first on source identity then on metadata fields and discovery flags">
  <div class="bp-flow-node" data-tone="cyan"><span>SOURCE</span><strong>identity</strong><small>query variables</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>TAP</span><strong>row fields</strong><small>archive values</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>MAP</span><strong>metadata</strong><small>persisted records</small></div>
  <span class="bp-flow-link" aria-hidden="true">+</span>
  <div class="bp-flow-node" data-tone="green"><span>DERIVE</span><strong>flags</strong><small>readiness + manifest</small></div>
</div>

Transforms can be referenced from:

- `source_identity.template_vars.*.transform`;
- `discovery.queries[].source_id_transform` for legacy query contexts;
- `discovery.prepare_metadata.field_map.*.transform`;
- `discovery.prepare_metadata.discovery_flags.*.transform`.

Inline transform chains are accepted as a list of names, but named `chain` transforms are easier to review when reused.

## Recipes

Select the final component of a publisher DID:

```yaml
scan_id_from_did:
  kind: split_last
  separators: ["/", ":", "#"]
```

Extract a beam number:

```yaml
beam_number:
  kind: regex_extract
  pattern: "beam[_-]([0-9]+)"
  group: 1
```

Derive a readiness flag from enrichment rows:

```yaml
has_rows:
  kind: is_present

discovery:
  prepare_metadata:
    discovery_flags:
      ra_dec_vsys_complete:
        from: enrichments.ra_dec_vsys
        transform: has_rows
```

## Validation

```bash
beampipe project validate -f PROJECT.yaml
beampipe project explain -f PROJECT.yaml
```

Validation rejects unknown kinds, missing transform references, and absent required parameters. Prefer explicit named definitions over legacy implicit transform names in new projects.

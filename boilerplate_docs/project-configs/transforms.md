# Transforms

Transforms turn survey-specific strings, TAP rows, enrichment results, and manifest values into stable beampipe fields. Define reusable transforms under `definitions.transforms`, then reference them from source identity, field maps, and discovery flags.

## Where transforms run

| Location | Input | Output is used for |
|----------|-------|--------------------|
| `source_identity.template_vars.*.transform` | Registered source identifier | Query template variables such as `{source_name}` |
| `discovery.queries[].source_id_transform` | Registered source identifier | Legacy query variable support |
| `discovery.prepare_metadata.field_map.*.transform` | One TAP row field or enrichment value | Persisted source metadata |
| `discovery.prepare_metadata.discovery_flags.*.transform` | Enrichment rows or mapped values | Readiness flags and manifest template values |

Prefer named transforms for repeated survey rules. Inline chains are useful for one-off field maps.

## WALLABY definitions

The reference config starts by naming each small operation:

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

The important pattern is that the names describe survey meaning, not just string mechanics. `normalized_sbid` is easier to review than seeing `extract_digits` repeated through field maps.

## Source identity

Use source identity transforms when the source registry uses one canonical identifier, but an archive query needs a different form.

```yaml
source_identity:
  canonical: source_identifier
  template_vars:
    source_identifier:
      from: canonical
    source_name:
      transform: hipass_source_name
```

For a registered source `HIPASSJ1313-15`, the template context becomes:

| Template variable | Value |
|-------------------|-------|
| `{source_identifier}` | `HIPASSJ1313-15` |
| `{source_name}` | `J1313-15` |

That lets CASDA search by the full filename prefix while VizieR searches the HIPASS catalogue value:

```sql
SELECT HIPASS, RAJ2000, DEJ2000, RV50max, RV50min, RVmom
FROM "VIII/73/hicat"
WHERE HIPASS = '{source_name}'
```

## Field maps

Field maps copy values from TAP rows into beampipe metadata. Add a transform when archive fields contain prefixes, suffixes, mixed separators, or noisy formatting.

```yaml
discovery:
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
```

Example inputs:

| Target field | Raw input | Transform | Output |
|--------------|-----------|-----------|--------|
| `sbid` | `ASKAP-12345` | `normalized_sbid` | `12345` |
| `scan_id` | `ivo://askap/scan/2024-01-31#879912` | `scan_id_from_did` | `879912` |
| `dataset_id` | `HIPASSJ1313-15_sbid12345.ms` | none | `HIPASSJ1313-15_sbid12345.ms` |

Inline chains are accepted in field maps when a named transform would be noise:

```yaml
field_map:
  sbid:
    from: obs_id
    transform: [askap_sbid, trim]
```

## Discovery flags

Discovery flags are stored with prepared metadata and can act as readiness gates. They are also available to manifest templates under `flags.*`.

```yaml
discovery:
  prepare_metadata:
    discovery_flags:
      ra_dec_vsys_complete:
        from: enrichments.ra_dec_vsys
        transform: has_rows
```

If the VizieR enrichment returns at least one row, `has_rows` produces `true`. If it returns an empty array, empty object, empty string, or `null`, it produces `false`.

Use flags for values that are derived from discovery but should not be treated as raw dataset metadata:

```yaml
manifest:
  source_template:
    source_identifier: "{source_identifier}"
    ra_string: "{flags.ra_string}"
    dec_string: "{flags.dec_string}"
    vsys: "{flags.vsys}"
```

## Supported transform kinds

| Kind | Required fields | Example input | Example output | Use for |
|------|-----------------|---------------|----------------|---------|
| `identity` | none | `HIPASSJ1313-15` | `HIPASSJ1313-15` | Explicit pass-through |
| `trim` | none | `  ASKAP-123  ` | `ASKAP-123` | Removing whitespace |
| `lowercase` | none | `Wallaby_HiRes` | `wallaby_hires` | Case-normalized IDs |
| `uppercase` | none | `casda` | `CASDA` | Adapter/profile labels |
| `replace` | `from`, optional `to` | `wallaby_hires` | `wallaby-hires` | Normalizing separators |
| `add_prefix` | `prefix` | `J1313-15` | `HIPASSJ1313-15` | Rebuilding canonical IDs |
| `add_suffix` | `suffix` | `manifest` | `manifest.json` | File names and labels |
| `default_if_empty` | optional `default` | empty string | configured default or `null` | Optional metadata defaults |
| `strip_prefix` | `prefix` | `HIPASSJ1313-15` | `J1313-15` | Archive/catalogue query values |
| `extract_digits` | none | `ASKAP-12345` | `12345` | SBIDs and numeric identifiers |
| `split_last` | optional `separators` | `ivo://x#scan-7` | `scan-7` | Publisher DIDs and paths |
| `is_present` | none | `[{"row": 1}]` | `true` | Readiness flags from enrichment rows |
| `regex_extract` | `pattern`, optional `group` | `beam_03.fits` | `03` | Structured filename parsing |
| `select_eval_file_by_size` | none | enrichment rows | filename string | CASDA evaluation file choice |
| `chain` | `steps` | ` ASKAP-123 ` | step output | Named multi-step transforms |

## Practical recipes

### Normalize a source slug

```yaml
definitions:
  transforms:
    trim:
      kind: trim
    lower:
      kind: lowercase
    underscores_to_dashes:
      kind: replace
      from: "_"
      to: "-"
    source_slug:
      kind: chain
      steps: [trim, lower, underscores_to_dashes]
```

`" Wallaby_HiRes "` becomes `wallaby-hires`.

### Extract a beam number

```yaml
definitions:
  transforms:
    beam_number:
      kind: regex_extract
      pattern: "beam[_-]([0-9]+)"
      group: 1
```

`beam_03_image.fits` becomes `03`.

### Default an optional catalogue field

```yaml
definitions:
  transforms:
    default_unknown:
      kind: default_if_empty
      default: unknown

discovery:
  prepare_metadata:
    field_map:
      catalogue_release:
        from: release_name
        transform: default_unknown
```

Empty `release_name` values become `unknown`; non-empty values pass through unchanged.

### Select a CASDA evaluation file

```yaml
definitions:
  transforms:
    eval_file:
      kind: select_eval_file_by_size

discovery:
  prepare_metadata:
    discovery_flags:
      evaluation_file:
        from: enrichments.sbid_to_eval_file
        transform: eval_file
```

When enrichment rows include `format: calibration`, the transform chooses the largest calibration file. Otherwise it chooses the largest row with a `filename`.

## Validation rules

Run validation before upload:

```bash
beampipe project validate -f config/wallaby_hires.v2.yaml
```

Validation checks that referenced transforms exist, supported kinds are used, and required fields such as `prefix`, `from`, `pattern`, or `steps` are present. Legacy transform names such as `strip_hipass_prefix`, `extract_askap_sbid`, and `extract_scan_id` still resolve, but new configs should define explicit names under `definitions.transforms`.

# Transforms

Transforms normalize source identifiers, TAP fields, metadata rows, and manifest values. Define them once under `definitions.transforms`, then reference them from `source_identity`, `field_map`, and `discovery_flags`.

## Example

```yaml
definitions:
  transforms:
    hipass_source_name:
      kind: strip_prefix
      prefix: HIPASS
    askap_sbid:
      kind: extract_digits
    normalized_sbid:
      kind: chain
      steps: [askap_sbid, trim]
    trim:
      kind: trim
```

Use in a field map:

```yaml
discovery:
  prepare_metadata:
    field_map:
      sbid:
        from: obs_id
        transform: normalized_sbid
```

## Common transform kinds

| Kind | Use |
|------|-----|
| `strip_prefix` | Remove a known prefix |
| `extract_digits` | Keep numeric characters |
| `split_last` | Split by separators and keep the last token |
| `is_present` | Convert presence/rows into a truthy flag |
| `trim` | Trim whitespace |
| `chain` | Run named transforms in order |

## Template variables

`source_identity.template_vars` builds named values for query templates. This keeps SQL templates stable even when the source registry uses a different canonical ID format.

```yaml
source_identity:
  canonical: source_identifier
  template_vars:
    source_name:
      transform: hipass_source_name
```

Then:

```sql
SELECT HIPASS, RAJ2000, DEJ2000
FROM "VIII/73/hicat"
WHERE HIPASS = '{source_name}'
```

## Validation

`beampipe project validate` checks that referenced transforms exist. Keep transform names short and survey-specific so warnings are easy to read in upload reports.

# WASM hooks

WASM hooks are optional extensions for survey logic that cannot be expressed with templates, transforms, discovery flags, manifests, or DALiuGE Graph patches.

## When to use WASM

Prefer declarative YAML first. Reach for WASM only when the logic needs richer parsing, external algorithmic behavior, or a reusable survey-specific module.

| Use YAML when | Use WASM when |
|---------------|---------------|
| A value can be normalized with a transform | Parsing needs custom code |
| A manifest field can be templated | Manifest construction needs branching logic |
| A graph value can be patched from `$count` or `$sum` | Graph preparation needs custom validation |
| A readiness flag can be derived from enrichment presence | Readiness requires a survey algorithm |

## Upload flow

```bash
beampipe wasm upload ./target/wasm32-wasip1/release/wallaby_hooks.wasm
```

The upload returns a SHA-256 identifier. Store that identifier in the project config so future executions pin the hook module they used.

## Config link

```yaml
extension:
  wasm_sha256: "<uploaded-module-sha256>"
  hooks:
    - prepare_metadata
    - manifest
```

Validate the config after adding or changing hooks:

```bash
beampipe project validate -f config/wallaby_hires.v1.yaml
```

## Boundary

WASM hooks should be deterministic from their inputs. Do not use them to hide credentials, call mutable external systems, or duplicate backend deployment logic. Keep operator-readable behavior in YAML whenever possible.

## Reference

| Hook | Typical purpose |
|------|-----------------|
| `prepare_metadata` | Extra metadata normalization after TAP rows and enrichments are available |
| `manifest` | Custom manifest shaping before DALiuGE Graph preparation |

Next: keep the generated API contract current with [OpenAPI export](../tools/openapi.md).

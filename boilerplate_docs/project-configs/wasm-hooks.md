# WASM hooks

WASM hooks are optional project-config extensions for survey logic that cannot be expressed with templates, transforms, and graph patches alone.

## Upload flow

Validate and upload a module for an active project config:

```bash
beampipe wasm upload \
  --config-id wallaby_hires \
  --file target/wasm32-wasip1/release/hook.wasm
```

The CLI validates the module, stores bytes in PostgreSQL, and prints the module SHA-256.

## Config link

Reference the uploaded module in the project config:

```yaml
extension:
  wasm_sha256: "<sha256>"
  hooks:
    - prepare_metadata
    - manifest
```

Upload the updated YAML through `POST /api/v2/project-configs`.

## Boundary

<div class="terminal-diagram">
<pre>project config ---> WASM host ---> hook module
      |                 |              |
      |                 |              v
      +---------- validated JSON <-----+</pre>
</div>

Use WASM only for logic that is hard to express declaratively. Prefer YAML transforms and graph patches for deterministic operator-readable behavior.

## Reference

The WIT interface lives in the repository at `wit/beampipe-hooks.wit`. Link to the GitHub source when publishing docs outside the repository:

```txt
https://github.com/jbwod/beampipe-core-v2/blob/main/wit/beampipe-hooks.wit
```

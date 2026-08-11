# Contributing

Keep implementation, generated contracts, examples, and operator procedures synchronized.

## Required checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
beampipe project validate -f config/wallaby_hires.v2.yaml
make docs-build
```

`make docs-build` exports OpenAPI, copies it into the docs tree, and runs MkDocs in strict mode.

## Documentation ownership

| Change | Update |
|---|---|
| CLI command or setup behavior | quick start and CLI reference |
| Environment setting or security policy | install/configuration and deployment profiles |
| Project schema | project YAML, transforms, graph preparation |
| API type or route | generated OpenAPI and API workflow if task order changes |
| Execution transition | architecture state model and recovery procedure |
| Metric or alert | observability dashboard order and alert guidance |
| Backend behavior | deployment profiles and qualification run |

## Writing rules

- Put a procedure in one page and link to it elsewhere.
- Use installed `beampipe` commands in operator docs; reserve `cargo run` for development.
- Keep all HTTP examples on `/api/v2`.
- State whether a command observes, prepares, or performs an external side effect.
- Never imply scheduler or DALiuGE completion proves scientific output.
- Prefer diagrams for ownership and sequence; keep them accessible and respect reduced motion.
- Link to generated API reference instead of duplicating every request field.

## Preview

```bash
make docs-serve
```

Review desktop and narrow layouts, keyboard behavior for interactive diagrams, code wrapping, navigation depth, and every changed link before committing.

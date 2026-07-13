# Contributing

Contributions should keep the Rust v2 operator story consistent: `beampipe` binary examples, `/api/v2` API examples, project config YAML, and generated OpenAPI/Redoc kept in sync.

## Local checks

```bash
cargo test
python3 -m mkdocs build --strict
beampipe openapi export > openapi.json
cp openapi.json boilerplate_docs/openapi.json
```

Use the `cargo run` OpenAPI command only when developing before installing the binary:

```bash
cargo run -p beampipe-cli --bin beampipe -- openapi export > openapi.json
```

## API contract

| Change | Required follow-up |
|--------|--------------------|
| Request/response schema | Regenerate OpenAPI and update Redoc docs asset |
| Auth flow | Update API workflow and Bruno collection |
| Project config schema | Update YAML model and transforms docs |
| Deployment profile schema | Update deployment profiles and first-run examples |
| Execution lifecycle | Update lifecycle, operator guide, and observability docs |

## Docs style

| Rule | Reason |
|------|--------|
| Prefer `beampipe` examples | Operators run the installed binary |
| Keep `cargo run` in dev-only sections | Avoid confusing production paths |
| Use `/api/v2` examples only | Avoid stale client code |
| Keep YAML keys literal | Operators copy config snippets |
| Say DALiuGE Graphs in prose | Keep the docs readable while preserving `graph_patches` YAML |
| Link to Redoc for schema detail | Avoid duplicating long request/response definitions |

Run `python3 -m mkdocs build --strict` before publishing documentation changes.

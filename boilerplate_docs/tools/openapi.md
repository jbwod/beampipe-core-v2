# OpenAPI export

The OpenAPI document is generated from the Rust `utoipa` definitions and drives Redoc, Bruno, and external clients.

## Export

Use the installed binary:

```bash
beampipe openapi export > openapi.json
cp openapi.json boilerplate_docs/openapi.json
```

When hacking Rust before installing the binary, the equivalent dev command is:

```bash
cargo run -p beampipe-cli --bin beampipe -- openapi export > openapi.json
```

## Files

| File | Purpose |
|------|---------|
| `openapi.json` | Repo-level exported contract |
| `boilerplate_docs/openapi.json` | Redoc input for the docs site |
| `api/reference.md` | MkDocs page that hosts the Redoc iframe |
| `api/redoc.html` | Redoc bootstrap and terminal theme |

## Contract policy

| Rule | Reason |
|------|--------|
| Keep examples on `/api/v2` | Avoid stale API clients |
| Regenerate after schema changes | Redoc and Bruno should match Rust types |
| Keep workflow examples concise | Redoc carries field-level schema detail |
| Build docs strictly | Catch stale links and nav drift |

Next: update [Bruno collection](bruno.md) when request bodies or auth flow changes.

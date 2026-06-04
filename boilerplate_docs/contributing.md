# Contributing 

## Local checks

```bash
cargo test
python3 -m mkdocs build --strict
```

For focused Rust changes:

```bash
cargo test -p beampipe-api
cargo test -p beampipe-jobs
cargo test -p beampipe-project
cargo test -p beampipe-orchestration
```

## API contract

After API schema changes:

```bash
beampipe openapi export > openapi.json
cp openapi.json boilerplate_docs/openapi.json
python3 -m mkdocs build --strict
```

From a fresh checkout, use the Cargo form until the release binary is on `PATH`:

```bash
cargo run -p beampipe-cli --bin beampipe -- openapi export > openapi.json
```

## Docs style

| Rule | Why |
|------|-----|
| Use `/api/v2` examples only | Avoid stale API examples |
| Prefer Rust CLI commands | Operators use `beampipe` directly |
| Keep diagrams ASCII-first | The docs render like terminal panes |
| Keep pages concise | Redoc carries schema detail |
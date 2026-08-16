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

## Cut a release

Bump these together so Compose pull and setup-created `.env` stay on the same image:

- `[workspace.package] version` in [`Cargo.toml`](https://github.com/jbwod/beampipe-core-v2/blob/main/Cargo.toml)
- `BEAMPIPE_VERSION` in [`.env.example`](https://github.com/jbwod/beampipe-core-v2/blob/main/.env.example), [`.env.template`](https://github.com/jbwod/beampipe-core-v2/blob/main/.env.template), and [`deploy/operator/.env.example`](https://github.com/jbwod/beampipe-core-v2/blob/main/deploy/operator/.env.example)
- the `${BEAMPIPE_VERSION:-0.1.0}` default in [`docker-compose.yml`](https://github.com/jbwod/beampipe-core-v2/blob/main/docker-compose.yml), [`deploy/operator/docker-compose.yml`](https://github.com/jbwod/beampipe-core-v2/blob/main/deploy/operator/docker-compose.yml), and the header comment in [`deploy/setup-docker.sh`](https://github.com/jbwod/beampipe-core-v2/blob/main/deploy/setup-docker.sh)
- the `0.1.0` fallback in [`setup.rs`](https://github.com/jbwod/beampipe-core-v2/blob/main/crates/beampipe-cli/src/setup.rs) (`default_env_skeleton` / `ensure_beampipe_version`)

Tag a matching semver. Pushing `v0.1.0` runs [`.github/workflows/release.yml`](https://github.com/jbwod/beampipe-core-v2/blob/main/.github/workflows/release.yml). Rust CI must pass before binaries or the container publish. The GitHub Release is created only when every binary matrix leg succeeds. The container job only needs CI, so GHCR can publish even when a host-archive leg fails:

- `beampipe-x86_64-unknown-linux-gnu.tar.gz`
- `beampipe-aarch64-unknown-linux-gnu.tar.gz`
- `beampipe-aarch64-apple-darwin.tar.gz`
- `beampipe-x86_64-apple-darwin.tar.gz`
- `SHA256SUMS`
- `install.sh`
- `ghcr.io/jbwod/beampipe-core-v2:0.1.0` (also `:0.1` and `:latest`; `linux/amd64` and `linux/arm64`)

```bash
git tag -a v0.1.0 -m "Beampipe 0.1.0"
git push origin v0.1.0
```

The first GHCR package is private. The release job tries to set it public and ignores failure. Confirm visibility once so `install.sh` and Compose users can pull without a GitHub token. `beampipe setup --runtime docker` does not compile from source when the pull fails. A git checkout can still use `./deploy/setup-docker.sh` which falls back to a local build.

## Preview

```bash
make docs-serve
```

Review desktop and narrow layouts, keyboard behavior for interactive diagrams, code wrapping, navigation depth, and every changed link before committing.

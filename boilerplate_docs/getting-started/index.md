# Quick start

Two equal paths. Both start with `beampipe setup`, which writes files and prints a recipe. It does **not** start PostgreSQL, the API, or Dash. Run every command from the repository root, where `docker-compose.yml` lives.

External execution stays mocked until you enable real backends. TAP discovery becomes live only when you register and trigger a source in the [first workflow](first-run.md).

## Prerequisites

- This repository checkout.
- Docker Engine with Compose v2 (used for Postgres on both paths, and for the whole stack on the Docker path).
- Ports `5432`, `8080`, and `9090` (`3000` if you attach Dash).
- A host Rust toolchain only for the host-binary path.

`--yes` requires `--runtime docker` or `--runtime host`. Interactive setup asks.

## Docker Compose

No host Rust. Setup runs inside the image and prints the Compose recipe.

```bash
docker context show
./deploy/setup-docker.sh --yes --skip-admin --skip-upload
```

Then run the printed recipe. Postgres is first:

```bash
docker compose up -d postgres
docker compose run --rm api migrate
docker compose run --rm api admin create-user \
  --username admin \
  --email admin@example.test \
  --password 'replace-this-local-password' \
  --superuser
docker compose run --rm api project add -f config/wallaby_hires.v2.yaml
docker compose up -d api scheduler worker
```

`./deploy/setup-docker.sh` builds the image and runs `beampipe setup --runtime docker`. It does not `compose up`. The image has no `git`; clone Dash on the host before adding `--dashboard`.

Verify:

```bash
curl -fsS http://127.0.0.1:8080/api/v2/health | jq .
docker compose ps
```

## Host binary

Same Compose Postgres service, then `beampipe start` on the host.

```bash
cargo build --locked --release -p beampipe-cli --bin beampipe
export PATH="$PWD/target/release:$PATH"
beampipe setup --yes --runtime host --skip-admin --skip-upload
```

Then run the printed recipe. Postgres is first:

```bash
docker compose up -d postgres
beampipe migrate
beampipe admin create-user \
  --username admin \
  --email admin@example.test \
  --password 'replace-this-local-password' \
  --superuser
beampipe project add -f config/wallaby_hires.v2.yaml
beampipe start
```

PostgreSQL is either the Compose `postgres` service (default when `docker-compose.yml` exists) or an existing URL (`--postgres existing`). If you already run PostgreSQL, pass `--postgres existing` and omit `docker compose up -d postgres`.

Verify:

```bash
curl -fsS http://127.0.0.1:8080/api/v2/health | jq .
beampipe status
```

<div class="bp-flow-diagram bp-flow-diagram--animated" role="img" aria-label="Setup writes files, then the operator starts Postgres and the chosen runtime">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>setup</strong><small>files + recipe</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02</span><strong>postgres</strong><small>compose up</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03</span><strong>seed</strong><small>migrate + admin</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04</span><strong>run</strong><small>compose or start</small></div>
</div>

Setup does not create a deployment profile. After the stack is up, install one with `beampipe profile add` when you are ready to connect REST or Slurm. Prefer interactive setup on shared systems so the admin password does not remain in shell history.

`beampipe init --directory operator-local` is a compact native footnote only. That directory has no Compose file; use `--runtime host --postgres existing` and your own PostgreSQL.

Continue with [First workflow](first-run.md) or [Deployment profiles and SSH](../architecture/deployment-profiles.md).

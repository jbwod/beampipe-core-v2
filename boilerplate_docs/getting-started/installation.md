# Installation

beampipe-core v2 ships as one Rust CLI binary named `beampipe`. Operators should prefer a released binary on `PATH`; Docker Compose is the typical local or small-deploy stack; `cargo run` is for Rust development only.

## One binary

The CLI is defined as `beampipe` in `crates/beampipe-cli/Cargo.toml`. The Docker image sets `ENTRYPOINT ["beampipe"]`, so container commands map directly to host commands.

| Command | Purpose |
|---------|---------|
| `beampipe setup` | Print setup guidance and environment checks |
| `beampipe migrate` | Apply database migrations |
| `beampipe admin create-user` | Create an operator account |
| `beampipe serve` | Run the HTTP API, optionally with embedded scheduler/worker ticks |
| `beampipe serve --worker false` | API-only process |
| `beampipe worker` | Worker-only process |
| `beampipe project validate` | Validate project config YAML/JSON |
| `beampipe wasm upload` | Upload WASM hook modules |
| `beampipe slurm ping` | Smoke-test a Slurm SSH deployment profile |
| `beampipe openapi export` | Export the OpenAPI contract |

## Preferred: binary on PATH

Download or build a release binary, put it on `PATH`, then bootstrap the database and API:

```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me

beampipe setup
beampipe migrate
beampipe admin create-user \
  --username admin \
  --password change-me \
  --email admin@example.test
beampipe serve --worker false
```

Run worker capacity from another shell:

```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker
```

For production-style process splits, run exactly one scheduler-enabled process and any number of API/worker-only replicas:

```bash
beampipe serve --worker false
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false BEAMPIPE_WORKER_CONCURRENCY=4 beampipe worker
```

## Build from source

Use this path when a release artifact is not available for the target host.

```bash
git clone https://github.com/jbwod/beampipe-core-v2.git
cd beampipe-core-v2
cargo build --release -p beampipe-cli --bin beampipe
```

Run the built binary directly:

```bash
target/release/beampipe setup
target/release/beampipe migrate
target/release/beampipe serve --worker false
```

Or install it into Cargo's binary directory:

```bash
cargo install --path crates/beampipe-cli
beampipe setup
```

## Docker Compose

Docker Compose starts PostgreSQL, an API process, a scheduler process, and worker replicas. It does not run migrations or create the first admin user for you.

```bash
docker compose build api
docker compose up -d
docker compose run --rm api migrate
docker compose run --rm api admin create-user \
  --username admin \
  --password change-me \
  --email admin@example.test
```

Compose services:

| Service | Runtime |
|---------|---------|
| `postgres` | PostgreSQL on `:5432` |
| `api` | `beampipe serve --worker false` on `:8080` |
| `scheduler` | `beampipe serve --worker true` for recurring ticks |
| `worker` | `beampipe worker`, scaled by Compose |

Optional observability:

```bash
docker compose --profile observability up -d
```

Prometheus is exposed on `http://127.0.0.1:9099`.

## Development with cargo run

Use `cargo run` only when hacking Rust on the host. It is the same command surface after Cargo compiles:

```bash
docker compose up -d postgres
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me

cargo run -p beampipe-cli --bin beampipe -- migrate
cargo run -p beampipe-cli --bin beampipe -- admin create-user \
  --username admin \
  --password change-me \
  --email admin@example.test
cargo run -p beampipe-cli --bin beampipe -- serve
```

## Health check

```bash
curl -s http://127.0.0.1:8080/api/v2/health | jq .
curl -s http://127.0.0.1:8080/api/v2/ready | jq .
```

Next: run [First run](first-run.md) to register a source, discover metadata, and queue a dry execution.

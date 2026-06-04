# Installation

beampipe-core v2 runs as one Rust binary: `beampipe`. The same executable is used on a host, inside the Docker image, and in development.

## One binary

The CLI binary is named `beampipe` in `crates/beampipe-cli/Cargo.toml`. Subcommands include:

| Command | Purpose |
|---------|---------|
| `beampipe serve` | Run the HTTP API, optionally with embedded workers |
| `beampipe serve --worker false` | API-only process |
| `beampipe worker` | Worker-only process |
| `beampipe migrate` | Apply SQLx migrations |
| `beampipe admin create-user` | Create an operator account |
| `beampipe project validate` | Validate project config YAML/JSON |
| `beampipe wasm upload` | Upload WASM hook modules |
| `beampipe slurm ping` | Slurm SSH smoke check |
| `beampipe openapi export` | Export the OpenAPI contract |

The Docker image also uses this binary: the Dockerfile installs `/usr/local/bin/beampipe` and sets `ENTRYPOINT ["beampipe"]`, so container commands are the same shape as host commands.

## Preferred: binary on PATH

Download or build a release binary, put it on `PATH`, then run:

```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me

beampipe migrate
beampipe admin create-user \
  --username admin \
  --password change-me \
  --email admin@example.test
beampipe project validate -f config/wallaby_hires.v1.yaml
beampipe serve --worker false
```

In another shell, run worker capacity:

```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker
```

Production-style split:

```bash
beampipe serve --worker false
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false BEAMPIPE_WORKER_CONCURRENCY=4 beampipe worker
```

## Build from source

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

To install from the local crate path:

```bash
cargo install --path crates/beampipe-cli
beampipe setup
```

## Development with cargo run

Use `cargo run` when hacking Rust on the host. It is the same command surface, with Cargo compiling first:

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

For faster iteration, keep Postgres in Docker and run API/workers from the host.

## Docker Compose

Docker Compose is the typical local or small-deploy path.

```bash
docker compose build api
docker compose up -d
```

That starts:

| Service | Runtime |
|---------|---------|
| `postgres` | PostgreSQL on `:5432` |
| `api` | `beampipe serve --worker false` on `:8080` |
| `scheduler` | `beampipe serve --worker true` for ticks and some job work |
| `worker` | `beampipe worker`, scaled by compose |

Compose does not run migrations or create an admin user for you. Run those once:

```bash
docker compose run --rm api migrate
docker compose run --rm api admin create-user \
  --username admin \
  --password change-me \
  --email admin@example.test
```

Optional observability:

```bash
docker compose --profile observability up -d
```

Prometheus is exposed on `http://127.0.0.1:9099`.

## Health check

```bash
curl -s http://127.0.0.1:8080/api/v2/health | jq .
curl -s http://127.0.0.1:8080/api/v2/ready | jq .
```

Read [First run](first-run.md) for the source discovery and execution workflow.

# Five-minute local start

This path starts PostgreSQL, creates operator-safe configuration, installs the WALLABY
project and a mock local DALiuGE profile, then opens the real control-plane console. It
does not contact CASDA, DALiuGE, SSH, or SLURM and cannot submit scientific work.

## Prerequisites

- The `beampipe` binary is on `PATH`, or use `target/debug/beampipe` after a source build.
- Docker Compose is available for PostgreSQL.
- Run these commands from the repository checkout.

## Bootstrap

```bash
docker compose up -d postgres
mkdir -p operator-local
beampipe init --directory operator-local
cd operator-local
beampipe setup --yes \
  --admin-password 'replace-this-local-password' \
  --project-config ../config/wallaby_hires.v2.yaml
```

`setup` creates `.env` with mode `0600` on Unix, generates a random JWT secret when
one was not supplied, applies migrations, creates the administrator, installs the
deployment profile, validates the project, and runs diagnostics. It refuses to replace
a different deployment-profile file.

## Start

```bash
beampipe doctor
beampipe start
```

In a second terminal, from `operator-local`:

```bash
beampipe status
beampipe console
```

The API is at `http://127.0.0.1:8080/api/v2`. The console reads PostgreSQL and probes
only configured integrations; it does not display fabricated scheduler or DALiuGE data.

## Before live work

Do not set `BEAMPIPE_USE_REAL_BACKENDS=true` yet. First configure a deployment profile,
run `beampipe graph prepare`, and pass all profile-specific doctor checks. Continue with
[DALiuGE and Setonix](../operations/daliuge-setonix.md).

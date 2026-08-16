# Quick start

Install without cloning. One script downloads the release binary, writes `~/beampipe`, and starts Postgres plus the stack. Interactive setup asks Docker Compose (default) or a host binary. Build a flagged command on the [docs home page](../index.md#install-builder).

External execution stays mocked until you enable real backends. TAP discovery becomes live only when you register and trigger a source in the [first workflow](first-run.md).

## Prerequisites

- Docker Engine with Compose v2 (Postgres on both paths; the whole stack on the Docker path).
- Ports `5432`, `8080`, and `9090` (`3000` if you attach Dash).
- Linux host archives need glibc and OpenSSL 3 (Ubuntu 22.04 / Debian bookworm or newer).

You do not need this repository checkout or a Rust toolchain.

## Install

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

Non-interactive Docker:

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh -s -- --yes --runtime docker
```

Host binary (Compose is used only for Postgres):

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh -s -- --yes --runtime host
```

The script installs `~/.local/bin/beampipe` and runs `beampipe setup --directory ~/beampipe`. Setup starts Postgres, migrates, creates `admin` (password printed once), and loads `wallaby_hires`. Docker `--yes` then starts the API. Host `--yes` leaves the API down; run `cd ~/beampipe && beampipe start` next. `--yes` requires `--runtime docker` or `--runtime host`. `--no-start` writes files and prints a recipe only.

Verify:

```bash
curl -fsS http://127.0.0.1:8080/api/v2/health | jq .
docker compose -f ~/beampipe/docker-compose.yml ps
```

<div class="bp-flow-diagram bp-flow-diagram--animated" role="img" aria-label="Installer writes files, starts Postgres, seeds, and runs the stack">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>install</strong><small>binary + ~/beampipe</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02</span><strong>postgres</strong><small>compose up</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03</span><strong>seed</strong><small>migrate + admin</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04</span><strong>run</strong><small>compose or start</small></div>
</div>

Setup does not create a deployment profile. After the stack is up, install one with `beampipe profile add` when you are ready to connect REST or Slurm.

`beampipe init --directory` writes the same operator Compose file and sample project without starting anything.

Clone this repository only when qualifying a commit. `./deploy/setup-docker.sh` is the developer path (`BEAMPIPE_BUILD=1` compiles the checkout) and passes `--no-start`.

Continue with [First workflow](first-run.md) or [Deployment profiles and SSH](../architecture/deployment-profiles.md).

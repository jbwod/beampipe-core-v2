<p align="center">
  <img src="assets/brand/beampipe-terminal-logo.svg" alt="Beampipe" width="920">
</p>
<p align="center">
<img width="443" height="49" alt="image" src="https://github.com/user-attachments/assets/cb7956bd-9e5c-4f6d-8b9f-a575b24b6402" />
</p>

> `beampipe-core` is a modular orchestration and triggering framework for data-driven radio astronomy workflows. It operates as an external control plane that continuously monitors scientific archives (ie; CASDA), determines when datasets are ready, and orchestrates scheduler-aware execution of distributed workflows (ie; DALiuGe) on heterogeneous HPC systems.


<p align="center">
  <a href="https://github.com/jbwod/beampipe-core-v2/actions/workflows/rust.yml"><img src="https://github.com/jbwod/beampipe-core-v2/actions/workflows/rust.yml/badge.svg" alt="Rust CI"></a>
  <a href="https://beampipe-core.readthedocs.io/"><img src="https://img.shields.io/badge/docs-operator_guide-7fd7e6?style=flat-square&labelColor=050505" alt="Documentation"></a>
  <img src="https://img.shields.io/badge/API-%2Fapi%2Fv2-d6c178?style=flat-square&labelColor=050505" alt="API v2">
  <img src="https://img.shields.io/badge/config-beampipe.dev%2Fv2-a7cfa3?style=flat-square&labelColor=050505" alt="Project config v2">
</p>

### `What it does`
<img width="1712" height="981" alt="image" src="https://github.com/user-attachments/assets/199f7eb9-48f9-4a5b-a0f4-ea7f2e6bb83e" />

<img width="1712" height="981" alt="image" src="https://github.com/user-attachments/assets/6fc85d71-c1a5-47fd-8f9a-304656c95dd3" />


## `Modular Orchestration by design`
> - **`Project-scoped automation`**: Designed from the ground-up to be Survey-Agnostic, a hotswap project based system to allow discovery and execution derrived by defined policy before enqueuing work. The example module was constructed for the [`wallaby-hires`](https://github.com/ICRAR/wallaby-hires) project and workflow, integrating ingestion with CASDA and HPC Compute on [pawsey-setonix](https://pawsey.org.au/systems/setonix/) to generate High Resolution data cubes with parameters.

> - **`Shaping and admission`**: global and per-project guards (rate budgets, queue depth, in-flight discovery batches / execution runs) coordinate so automation stays within configured capacity.

> - **`Execution ledger (batch runs)`**: API and workers create batch execution records over registered sources. The execution ledger validates that sources are registered, enabled, discovery-complete, and backed by archive metadata (including optional per-source filters and discovery flag checks defined dynamically by each project) before, if configured, executing a Job.

> - **`DALiuGE Integrated`**: Supports multiple translator and deployment configuration profiles (REST DIM, Slurm remote, compute limits) which can be assigned per-run, per-module or as global defaults. By use of a dedicated `beampipe-ingest` PyFunc Drop, `beampipe` can be adapted for use in existing Graphs to handle generated JSON manfiests upon translation. The following [beampipe.pallette]() can be downloaded and imported to [EAGLE](https://eagle.icrar.org/).

<img width="856" height="470" alt="image" src="https://github.com/user-attachments/assets/c542f64a-467a-4432-b435-ceba52d2dc8e" />
<table>
  <tr>
    <td>
      <img width="717" height="442" alt="graphout" src="https://github.com/user-attachments/assets/45f1ff28-71e4-4c6c-8b25-2f00f9ad2441" />
    </td>
    <td>
<img width="1270" height="714" alt="image" src="https://github.com/user-attachments/assets/124e48f7-6598-41a2-a835-9cfabcec8ee1" />

  </tr>
</table>






The key invariant is simple: persist deterministic intent before external I/O, record external IDs as soon as they are known, and reconcile ambiguity before retrying.

## Quick start

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh
```

That installs `beampipe` to `~/.local/bin`, writes `~/beampipe`, and starts Postgres plus the stack. Interactive setup asks Docker (default) or host. Non-interactive:

```bash
curl -fsSL https://github.com/jbwod/beampipe-core-v2/releases/latest/download/install.sh | sh -s -- --yes --runtime docker
```

The API is at `http://127.0.0.1:8080/api/v2`. Files live in `~/beampipe`. You do not need to clone this repository. Linux host archives need glibc and OpenSSL 3 (Ubuntu 22.04 / Debian bookworm or newer).

`--no-start` writes files and prints a recipe without starting anything. A source build is only for qualifying a commit and needs Cargo 1.78 or newer.

Use an override to bind API, metrics, and PostgreSQL to loopback on a workstation.
When Dash is also containerized, attach it to the Core Compose network and use
`http://api:8080`; the browser never needs direct API access.

See [deployment topologies](https://beampipe-core.readthedocs.io/getting-started/deployment/)
for secure bindings, Docker contexts, role configuration, system services,
container platforms, REST/DIM, and Slurm.

Install a deployment profile with `beampipe profile add` before setting `BEAMPIPE_USE_REAL_BACKENDS=true`.

Continue with the [quick start](https://beampipe-core.readthedocs.io/getting-started/) and [first workflow](https://beampipe-core.readthedocs.io/getting-started/first-run/).

## Runtime roles
<img width="1713" height="981" alt="image" src="https://github.com/user-attachments/assets/f836361d-f640-4fd5-a1c2-e84d56473552" />


| Role | Command | Scale rule |
|---|---|---|
| API | `beampipe serve --worker false` | scale for HTTP traffic |
| Scheduler | `BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true` | run exactly one |
| Worker | `BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker` | scale for queue throughput |
| Compact | `beampipe start` | local evaluation and small deployments |


## Documentation

| Task | Page |
|---|---|
| Install and reach a healthy system | [Quick start](https://beampipe-core.readthedocs.io/getting-started/) |
| Choose Docker, native, system-service, or orchestrated deployment | [Deployment topologies](https://beampipe-core.readthedocs.io/getting-started/deployment/) |
| Run one discovery and graph preparation | [First workflow](https://beampipe-core.readthedocs.io/getting-started/first-run/) |
| Demonstrate multiple no-download REST runs | [REST demonstration playbook](https://beampipe-core.readthedocs.io/getting-started/rest-no-downloads-demo/) |
| Connect REST or Slurm and manage SSH keys | [Deployment profiles and SSH](https://beampipe-core.readthedocs.io/architecture/deployment-profiles/) |
| Operate and recover work | [Operator handbook](https://beampipe-core.readthedocs.io/operations/) |
| Author project-defined TAP and graph policy | [Project YAML](https://beampipe-core.readthedocs.io/project-configs/) |
| Integrate over HTTP | [API workflow](https://beampipe-core.readthedocs.io/api/) |

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
beampipe project validate -f config/wallaby_hires.v2.yaml
make docs-build
```

The workspace crates separate API/auth/config, database/domain state, project/profile schemas, adapters/orchestration, jobs, security, metrics/alerts, and the CLI. See the [architecture](https://beampipe-core.readthedocs.io/architecture/) for ownership boundaries.

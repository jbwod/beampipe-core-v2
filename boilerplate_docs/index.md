---
hide:
  - toc
---

<div class="terminal-panel beampipe-hero" markdown>
<picture class="bp-hero-logo">
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/4414e79f-7431-4999-b2ef-28cf9f0b254e">
  <img src="https://github.com/user-attachments/assets/4414e79f-7431-4999-b2ef-28cf9f0b254e" alt="beampipe-core">
</picture>

# ~ beampipe-core

<p class="bp-tagline">Rust v2 control plane for archive-driven radio astronomy workflows. Register sources, discover archive metadata, build manifests, and submit DALiuGE pipelines to local or HPC backends.</p>

<div class="bp-terminal-actions" markdown>
[Install](getting-started/installation.md){ .terminal-button }
[API Reference](api/reference.md){ .terminal-button }
</div>
</div>

## Quick start

Use the installed `beampipe` binary for operator examples. Docker uses the same entrypoint, so host and container commands share the same CLI surface.

<div class="terminal-command">
```bash
docker compose up -d postgres
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me

beampipe migrate
beampipe admin create-user --username admin --password change-me --email admin@example.test
beampipe serve --worker false
```
</div>

Add workers from another shell:

<div class="terminal-command">
```bash
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker
```
</div>

## Control plane

<div class="terminal-diagram terminal-diagram--center">
<pre>+---------+   +-----------+   +----------+   +-----------+
| sources |-->| discovery |-->| ledger   |-->| backends  |
| config   |   | TAP rows  |   | manifest |   | DIM/Slurm |
+---------+   +-----------+   +----------+   +-----------+
     ^              |              |              |
     +--------- operator API / metrics -----------+</pre>
</div>

beampipe-core does not replace the science workflow. It coordinates the archive-facing and scheduler-facing work around that workflow: source registration, metadata discovery, manifest preparation, graph mutation, backend submission, state, provenance, metrics, and retryable jobs.

## Operator map

<div class="bp-feature-grid">
<a href="getting-started/installation/">
<strong>[.] Getting started</strong>
<span>Install `beampipe`, run migrations, create an operator, and complete the first source workflow.</span>
</a>
<a href="operations/operator-guide/">
<strong>[.] Operations</strong>
<span>Run API, scheduler, and workers; tune queues; watch metrics; and handle incidents.</span>
</a>
<a href="architecture/control-plane/">
<strong>[.] Architecture</strong>
<span>Understand control-plane state, discovery/execution lifecycle, and deployment profile selection.</span>
</a>
<a href="project-configs/">
<strong>[.] Project configs</strong>
<span>Define archive queries, transforms, manifests, DALiuGE Graphs, automation, and WASM hooks.</span>
</a>
</div>

## Next steps

1. Install and bootstrap from [Installation](getting-started/installation.md).
2. Run one source through [First run](getting-started/first-run.md).
3. Review environment variables in [Configuration](getting-started/configuration.md).
4. Move from mock work to real backends with [Deployment profiles](architecture/deployment-profiles.md).

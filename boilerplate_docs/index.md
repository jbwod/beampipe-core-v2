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

<p class="bp-tagline">Rust v2 control plane for archive-driven radio astronomy workflows. Register sources, discover archive metadata, build execution manifests, and submit DALiuGE pipelines to local or HPC backends.</p>

<div class="bp-terminal-actions" markdown>
[Get Started](getting-started/installation.md){ .terminal-button }
[API Reference](api/reference.md){ .terminal-button }
</div>
</div>

## Quick start

<div class="terminal-command">
```bash
docker compose up -d
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/beampipe
export BEAMPIPE_JWT_SECRET=change-me
beampipe migrate
beampipe admin create-user --username admin --password change-me --email admin@example.test
beampipe serve --worker false
```
</div>

## Control Plane

<div class="terminal-diagram terminal-diagram--center">
<pre>+---------+   +-----------+   +----------+   +-----------+
| sources |-->| discovery |-->| ledger   |-->| backends  |
| config   |   | TAP rows  |   | manifest |   | DIM/Slurm |
+---------+   +-----------+   +----------+   +-----------+
     ^              |              |              |
     +--------- operator API / metrics -----------+</pre>
</div>

## Operator Map

<div class="bp-feature-grid">
<a href="getting-started/installation/">
<strong>[.] Getting started</strong>
<span>Install the Rust CLI, run migrations, create an admin user, and start API/worker processes.</span>
</a>
<a href="operations/operator-guide/">
<strong>[.] Operations</strong>
<span>Scale workers, tune scheduler admission, expose metrics, and run production checks.</span>
</a>
<a href="architecture/control-plane/">
<strong>[.] Architecture</strong>
<span>Understand discovery, execution, deployment profiles, and how state moves through Postgres.</span>
</a>
<a href="project-configs/">
<strong>[.] Project configs</strong>
<span>Define archive queries, transforms, manifests, DALiuGE graph patches, automation, and WASM hooks.</span>
</a>
</div>

## Next steps



Start with [Installation](getting-started/installation.md), then run the [First run](getting-started/first-run.md) workflow against `wallaby_hires`.

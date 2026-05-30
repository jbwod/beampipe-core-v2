---
hide:
  - toc
---

<div class="beampipe-hero" markdown>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/4414e79f-7431-4999-b2ef-28cf9f0b254e">
  <source media="(prefers-color-scheme: light)" srcset="https://github.com/user-attachments/assets/648d6a14-e1ee-4297-aa36-ff58f130e5d8">
  <img alt="beampipe-core" src="https://github.com/user-attachments/assets/4414e79f-7431-4999-b2ef-28cf9f0b254e">
</picture>

<p class="bp-tagline">External control plane for data-driven radio astronomy. Watches the archive, maintains an execution ledger, and orchestrates <img class="bp-brand-icon bp-daliuge" src="assets/daliuge.png" alt="DALiuGE"> pipelines on HPC.</p>
</div>

<div class="bp-hero-actions" markdown>

[API reference :material-api:](api/index.md){ .md-button }
[View on GitHub :material-github:](https://github.com/jbwod/beampipe-core){ .md-button }

</div>

## Deployment topology

Local dev runs a single instance of each service. Production adds nginx, replicated web/worker/Restate services, PostgreSQL, Redis, and Slurm SSH to a remote HPC login node.

<div class="bp-topology-grid">
<picture class="bp-topology-cell bp-topology-cell--local">
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/ba78a75d-a84d-416e-93a8-39043c9524c1">
  <source media="(prefers-color-scheme: light)" srcset="https://github.com/user-attachments/assets/4a31dee1-5daf-4348-a03f-559c9f463dd3">
  <img src="https://github.com/user-attachments/assets/4a31dee1-5daf-4348-a03f-559c9f463dd3" alt="Local development topology">
</picture>
<picture class="bp-topology-cell bp-topology-cell--production">
  <source media="(prefers-color-scheme: light)" srcset="https://github.com/user-attachments/assets/5631237e-02e8-4be0-ae39-9e0343714187">
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/8ff3c5b4-e3b2-408c-aab7-782ba6fa2d16">
  <img src="https://github.com/user-attachments/assets/5631237e-02e8-4be0-ae39-9e0343714187" alt="Production deployment topology">
</picture>
</div>

## Quick start

```bash
git clone https://github.com/jbwod/beampipe-core.git
cd beampipe-core
python setup.py          # choose "local"
make dev
```

See [Installation](getting-started/installation.md) and [Configuration](getting-started/configuration.md) for setup details.

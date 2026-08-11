---
hide:
  - toc
---

<div class="bp-docs-hero" markdown>

<p class="bp-hero-kicker">[ RUST V2 / DURABLE CONTROL PLANE ]</p>

# beampipe

<p class="bp-tagline">Discover archive data, prepare reproducible DALiuGE graphs, and operate REST or Slurm execution from one PostgreSQL-backed control plane.</p>

<div class="bp-hero-actions" markdown>
[Start locally](getting-started/index.md){ .terminal-button }
[Configure a backend](architecture/deployment-profiles.md){ .terminal-button }
</div>

<div class="bp-hero-status" aria-label="Current product boundary">
  <span><b data-tone="cyan">API</b> /api/v2</span>
  <span><b data-tone="amber">TRUTH</b> PostgreSQL</span>
  <span><b data-tone="green">RUNTIME</b> REST or Slurm</span>
</div>

</div>

## Fastest safe start

Run the API, durable job system, and console locally. External execution stays mocked until you explicitly enable real backends. Discovery uses live TAP only after you register and trigger a source.

```bash
docker compose up -d postgres
cargo build --locked --release -p beampipe-cli --bin beampipe
export PATH="$PWD/target/release:$PATH"

beampipe init --directory operator-local
cd operator-local
beampipe setup --yes \
  --admin-password 'replace-this-local-password' \
  --project-config ../config/wallaby_hires.v2.yaml
beampipe start
```

From another terminal in `operator-local`:

```bash
beampipe doctor
beampipe status
beampipe console
```

[Open the complete quick start](getting-started/index.md){ .bp-inline-action }

## How work moves

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Archive discovery and project policy flow through the Beampipe ledger and workers to DALiuGE">
  <div class="bp-flow-node" data-tone="cyan"><span>01 / FACTS</span><strong>CASDA + VizieR</strong><small>project-defined TAP</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02 / INTENT</span><strong>PostgreSQL</strong><small>config + ledger + jobs</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03 / EFFECT</span><strong>workers</strong><small>leased and fenced</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04 / RUNTIME</span><strong>DALiuGE</strong><small>REST DIM or Slurm</small></div>
</div>

Beampipe owns durable intent, preparation artifacts, admission, submission records, and reconciliation. Archives, Slurm, and DALiuGE remain authoritative for their own facts. A successful scheduler submission is therefore never treated as proof of a successful graph.

## Choose a task

<div class="bp-feature-grid bp-feature-grid--routes">
<a href="getting-started/"><strong>[01] Set up</strong><span>Reach a healthy local control plane, then run one source workflow.</span></a>
<a href="operations/"><strong>[02] Operate</strong><span>Watch queues and workers, investigate failures, and recover safely.</span></a>
<a href="project-configs/"><strong>[03] Configure</strong><span>Define TAP queries, metadata, manifests, graph patches, and automation.</span></a>
<a href="architecture/deployment-profiles/"><strong>[04] Deploy</strong><span>Connect an existing DALiuGE DIM or a Slurm facility with strict SSH trust.</span></a>
<a href="architecture/"><strong>[05] Understand</strong><span>Follow durable state through discovery, preparation, submission, and polling.</span></a>
<a href="api/"><strong>[06] Integrate</strong><span>Use the authenticated API workflow and generated schema.</span></a>
</div>

## Current qualification

The implementation has been exercised through real CASDA/VizieR discovery, automatic admission, manifest and graph preparation, DALiuGE translation, and REST deployment to a local cluster. That run exposed and fixed manifest flag resolution. A graph/runtime package mismatch then prevented terminal graph success, and CASDA staging plus Slurm have not yet been qualified end to end.

Use the [qualification run](operations/end-to-end-demo.md) for the exact evidence required before calling a release production-ready.

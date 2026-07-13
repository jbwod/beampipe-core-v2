---
hide:
  - toc
---

<div class="bp-docs-hero" markdown>

<p class="bp-hero-kicker">[ RUST V2 / OPERATOR CONTROL PLANE ]</p>

# beampipe

<p class="bp-tagline">Turn archive metadata and project policy into observable, recoverable DALiuGE execution on local or HPC infrastructure.</p>

<div class="bp-hero-actions" markdown>
[Start safely](getting-started/index.md){ .terminal-button }
[Open operator handbook](operations/index.md){ .terminal-button }
</div>

<div class="bp-hero-status" aria-label="Beampipe operating boundary">
  <span><b data-tone="cyan">INPUT</b> CASDA + project YAML</span>
  <span><b data-tone="amber">TRUTH</b> PostgreSQL ledger</span>
  <span><b data-tone="green">EFFECT</b> workers + DALiuGE</span>
</div>

</div>

## Start in five minutes

This path runs the real control plane and console against PostgreSQL while keeping archive, scheduler, and DALiuGE side effects disabled.

<div class="terminal-command" data-title="quickstart.safe">

```bash
docker compose up -d postgres
mkdir -p operator-local
beampipe init --directory operator-local
cd operator-local
beampipe setup --yes \
  --admin-password 'replace-this-local-password' \
  --project-config ../config/wallaby_hires.v2.yaml
beampipe doctor
beampipe start
```

</div>

[Open the annotated local walkthrough](getting-started/five-minute-start.md){ .bp-inline-action }

## The control-plane boundary

<div class="bp-flow-diagram bp-flow-diagram--wide" role="img" aria-label="CASDA and project configuration flow through Beampipe durable state and workers to Slurm and DALiuGE">
  <div class="bp-flow-node" data-tone="cyan"><span>FACTS</span><strong>CASDA</strong><small>archive metadata</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>POLICY</span><strong>project YAML</strong><small>versioned intent</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>TRUTH</span><strong>Beampipe</strong><small>ledger + artifacts</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>LEASE</span><strong>workers</strong><small>fenced effects</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>RUN</span><strong>Slurm + DALiuGE</strong><small>science runtime</small></div>
</div>

Beampipe does not replace the science graph. It owns source registration, discovery, manifest and graph preparation, admission, backend submission intent, reconciliation, provenance, and operator-facing diagnostics. External systems retain authority over archive facts and scientific execution.

[Explore each boundary](architecture/index.md){ .bp-inline-action }

## Choose your working context

<div class="bp-switcher bp-terminal-frame" data-bp-switcher data-title="docs.route">
  <div class="bp-segmented" role="tablist" aria-label="Documentation route">
    <button type="button" role="tab" aria-selected="true" aria-controls="route-operator" id="route-tab-operator" data-bp-target="route-operator">operator</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="route-project" id="route-tab-project" data-bp-target="route-project">project author</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="route-integrator" id="route-tab-integrator" data-bp-target="route-integrator">integrator</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="route-oncall" id="route-tab-oncall" data-bp-target="route-oncall">on-call</button>
  </div>

  <section id="route-operator" role="tabpanel" aria-labelledby="route-tab-operator" data-bp-panel>
    <span class="bp-status" data-tone="green">RUN</span>
    <h3>Install, verify, observe, and act</h3>
    <p><a href="getting-started/">Choose a setup path</a> -&gt; <a href="getting-started/first-run/">complete one workflow</a> -&gt; <a href="operations/">open the handbook</a>.</p>
  </section>
  <section id="route-project" role="tabpanel" aria-labelledby="route-tab-project" data-bp-panel hidden>
    <span class="bp-status" data-tone="cyan">DEFINE</span>
    <h3>Shape discovery, manifests, and graphs</h3>
    <p><a href="project-configs/">Read the typed v2 model</a> -&gt; <a href="project-configs/transforms/">compose transforms</a> -&gt; <a href="project-configs/graph-patches/">prepare DALiuGE Graphs</a>.</p>
  </section>
  <section id="route-integrator" role="tabpanel" aria-labelledby="route-tab-integrator" data-bp-panel hidden>
    <span class="bp-status" data-tone="amber">CONNECT</span>
    <h3>Work from stable contracts</h3>
    <p><a href="architecture/adapters/">Review adapter boundaries</a> -&gt; <a href="api/">follow the API workflow</a> -&gt; <a href="api/reference/">inspect the generated schema</a>.</p>
  </section>
  <section id="route-oncall" role="tabpanel" aria-labelledby="route-tab-oncall" data-bp-panel hidden>
    <span class="bp-status" data-tone="red">TRIAGE</span>
    <h3>Move from symptom to durable fact</h3>
    <p><a href="operations/observability/">Use the debug order</a> -&gt; <a href="architecture/state-machine/">compare state axes</a> -&gt; <a href="operations/recovery/">retry or cancel safely</a>.</p>
  </section>
</div>

## Documentation map

<div class="bp-feature-grid bp-feature-grid--routes">
<a href="getting-started/"><strong>[01] Start</strong><span>Install and cross from mock operation to real infrastructure deliberately.</span></a>
<a href="operations/"><strong>[02] Operate</strong><span>Run shifts, scale workers, inspect backends, recover, and promote releases.</span></a>
<a href="project-configs/"><strong>[03] Configure projects</strong><span>Define typed discovery, transforms, manifests, graph patches, and extensions.</span></a>
<a href="architecture/"><strong>[04] Understand</strong><span>Learn the control-plane boundary, lifecycle, state axes, and adapter contracts.</span></a>
<a href="reference/cli/"><strong>[05] Reference</strong><span>Find CLI commands, vocabulary, API workflow, and generated schemas.</span></a>
</div>

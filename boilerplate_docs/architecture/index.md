---
hide:
  - toc
---

# Architecture map

<p class="bp-lede">Beampipe is the durable control plane between archive metadata, project policy, workers, schedulers, and DALiuGE. It decides and records what should happen; external systems perform the scientific work.</p>

## Explore the boundary

<div class="bp-explorer bp-terminal-frame" data-bp-explorer data-title="system.map">
  <div class="bp-system-map" aria-label="Interactive Beampipe system map">
    <button type="button" class="bp-system-node" data-tone="cyan" data-bp-target="system-inputs" aria-pressed="false"><span>INPUT</span><strong>CASDA + YAML</strong><small>facts and policy</small></button>
    <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
    <button type="button" class="bp-system-node" data-tone="amber" data-bp-target="system-api" aria-pressed="true"><span>CONTROL</span><strong>API + ledger</strong><small>intent and truth</small></button>
    <span class="bp-flow-link" aria-hidden="true">&lt;--&gt;</span>
    <button type="button" class="bp-system-node" data-tone="green" data-bp-target="system-workers" aria-pressed="false"><span>CLAIM</span><strong>workers</strong><small>leased effects</small></button>
    <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
    <button type="button" class="bp-system-node" data-tone="cyan" data-bp-target="system-external" aria-pressed="false"><span>EXTERNAL</span><strong>Slurm + DALiuGE</strong><small>science runtime</small></button>
  </div>

  <div class="bp-explorer-output" aria-live="polite">
    <section id="system-inputs" data-bp-panel hidden>
      <span class="bp-status" data-tone="cyan">READ / VERSION</span>
      <h2>Archive facts and project policy enter separately</h2>
      <p>CASDA supplies metadata. Versioned project YAML supplies queries, transforms, manifest shape, graph patches, and automation limits. An execution pins the configuration revision it used.</p>
    </section>
    <section id="system-api" data-bp-panel>
      <span class="bp-status" data-tone="amber">AUTHORITATIVE</span>
      <h2>PostgreSQL carries control-plane truth</h2>
      <p>The API records operator intent. The ledger, jobs, claims, artifacts, profile snapshots, and provenance make that intent recoverable across API and worker restarts.</p>
    </section>
    <section id="system-workers" data-bp-panel hidden>
      <span class="bp-status" data-tone="green">LEASED / FENCED</span>
      <h2>Workers perform bounded side effects</h2>
      <p>Workers claim compatible jobs with leases and fencing tokens. Heartbeats, labels, pool requirements, concurrency limits, and dead-letter state make horizontal operation explicit.</p>
    </section>
    <section id="system-external" data-bp-panel hidden>
      <span class="bp-status" data-tone="cyan">RECONCILED</span>
      <h2>Schedulers and DALiuGE remain external authorities</h2>
      <p>Beampipe stores their identifiers and observed states independently. A scheduler allocation, DALiuGE session, and verified output are related facts, not interchangeable definitions of success.</p>
    </section>
  </div>
</div>

## Follow a concept

<div class="bp-feature-grid">
<a href="control-plane/"><strong>[01] Control-plane boundaries</strong><span>Ownership, durable records, and responsibilities.</span></a>
<a href="lifecycle/"><strong>[02] Discovery and execution</strong><span>From source registration to verified outputs.</span></a>
<a href="state-machine/"><strong>[03] Execution state model</strong><span>Control phase, external axes, and uncertainty.</span></a>
<a href="adapters/"><strong>[04] Integration adapters</strong><span>Archive, scheduler, and DALiuGE contracts.</span></a>
<a href="deployment-profiles/"><strong>[05] Deployment profiles</strong><span>Immutable backend configuration per execution.</span></a>
</div>

## The invariant

<div class="terminal-note" data-tone="green">
<strong>One durable decision, many observed systems.</strong><br>
Every external action must be tied to persisted intent, deterministic identity, an immutable profile/config snapshot, and enough provenance to reconcile after interruption.
</div>

# Execution state model

Beampipe separates its control phase from submission, scheduler, DALiuGE, and output-verification facts. Those facts can disagree, so none is allowed to stand in for the others.

## Explore the control phase

<div class="bp-explorer bp-terminal-frame" data-bp-explorer data-title="execution.control_phase">
  <div class="bp-state-rail" aria-label="Execution control phases">
    <button type="button" data-bp-target="phase-discover" aria-pressed="true"><span>01</span>discover</button>
    <i aria-hidden="true">--&gt;</i>
    <button type="button" data-bp-target="phase-prepare" aria-pressed="false"><span>02</span>prepare</button>
    <i aria-hidden="true">--&gt;</i>
    <button type="button" data-bp-target="phase-submit" aria-pressed="false"><span>03</span>submit</button>
    <i aria-hidden="true">--&gt;</i>
    <button type="button" data-bp-target="phase-monitor" aria-pressed="false"><span>04</span>monitor</button>
    <i aria-hidden="true">--&gt;</i>
    <button type="button" data-bp-target="phase-verify" aria-pressed="false"><span>05</span>verify</button>
    <i aria-hidden="true">--&gt;</i>
    <button type="button" data-bp-target="phase-terminal" aria-pressed="false"><span>06</span>terminal</button>
  </div>

  <div class="bp-explorer-output" aria-live="polite">
    <section id="phase-discover" data-bp-panel><span class="bp-status" data-tone="cyan">NO EXTERNAL WORK</span><h2>Discover and admit</h2><p>Register source identity, prepare archive metadata, compare signatures, and evaluate readiness and automation limits.</p></section>
    <section id="phase-prepare" data-bp-panel hidden><span class="bp-status" data-tone="amber">IMMUTABLE INPUTS</span><h2>Generate manifest and graph</h2><p>Pin revisions, render the manifest, apply validated patches, translate when configured, and checksum every artifact.</p></section>
    <section id="phase-submit" data-bp-panel hidden><span class="bp-status" data-tone="red">SIDE-EFFECT BOUNDARY</span><h2>Persist intent, then submit</h2><p>Write deterministic external identity before I/O. A lost response becomes uncertainty, not an automatic failure.</p></section>
    <section id="phase-monitor" data-bp-panel hidden><span class="bp-status" data-tone="cyan">RECONCILE</span><h2>Observe independent authorities</h2><p>Poll scheduler and DALiuGE state without collapsing either into the public execution status.</p></section>
    <section id="phase-verify" data-bp-panel hidden><span class="bp-status" data-tone="amber">SCIENCE CHECK</span><h2>Verify expected outputs</h2><p>Scheduler or DALiuGE completion advances the execution to output verification; it does not prove scientific success.</p></section>
    <section id="phase-terminal" data-bp-panel hidden><span class="bp-status" data-tone="green">DURABLE OUTCOME</span><h2>Record outcome and provenance</h2><p>Complete, fail, or cancel only with the external identifiers, artifacts, and narrative required for later audit.</p></section>
  </div>
</div>

The public status is a compact operator projection: `pending`, `running`, `awaiting_scheduler`, `not_submitted`, `completed`, `failed`, `retrying`, or `cancelled`. Exact position remains in `control_phase` and `execution_phase`.

## Independent external axes

<div class="bp-axis-grid">
  <div><strong>submission</strong><span data-tone="cyan">not_started</span><span data-tone="amber">in_flight</span><span data-tone="green">submitted</span><span data-tone="red">uncertain / failed</span></div>
  <div><strong>scheduler</strong><span>not_submitted</span><span data-tone="amber">pending</span><span data-tone="cyan">running</span><span data-tone="green">succeeded</span><span data-tone="red">failed / cancelled / unknown</span></div>
  <div><strong>DALiuGE</strong><span>not_created</span><span data-tone="amber">building / deploying</span><span data-tone="cyan">running</span><span data-tone="green">finished</span><span data-tone="red">failed / unreachable</span></div>
  <div><strong>outputs</strong><span>not_started</span><span data-tone="amber">pending / verifying</span><span data-tone="green">verified</span><span data-tone="red">failed / unknown</span></div>
</div>

The reducer derives the next action from all axes. A succeeded scheduler allocation with an active DALiuGE session is `inconsistent` and requires review; it is never promoted silently to success.

## Submission uncertainty

Before external I/O, Beampipe persists submission intent and deterministic external identity. If SSH disconnects after `sbatch`, the submission axis becomes `uncertain`. Reconciliation searches by stable job name and checks `squeue` and `sacct`; retry stays blocked until the system can prove no scheduler job or DALiuGE session exists.

## Retry gate

<div class="bp-decision-table">
  <div class="bp-decision-table__head"><span>Observed state</span><span>Safe action</span></div>
  <div><span>Failed before manifest creation</span><span data-tone="green">Regenerate from pinned inputs</span></div>
  <div><span>Known translation or pre-submit failure</span><span data-tone="green">Resume with immutable artifacts</span></div>
  <div><span>Submission uncertain</span><span data-tone="red">Reconcile; retry blocked</span></div>
  <div><span>External job or session exists</span><span data-tone="red">Monitor or cancel; do not duplicate</span></div>
</div>

```bash
beampipe execution retry "$EXECUTION_ID" \
  --reason "Translator endpoint restored after planned maintenance"
```

Every retry requires a reason and increments `retry_count`. Meaningful transitions, claims, recovery, retry, cancellation, and administrative actions append provenance or claim-history records.

Continue with the operator procedure for [recovery and cancellation](../operations/recovery.md).

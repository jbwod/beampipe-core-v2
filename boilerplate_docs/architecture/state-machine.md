# Execution state model

Beampipe separates its own control phase from submission, scheduler, DALiuGE, and output observations. These facts can disagree, so none substitutes for another.

## Control phase

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
    <button type="button" data-bp-target="phase-terminal" aria-pressed="false"><span>05</span>terminal</button>
  </div>

  <div class="bp-explorer-output" aria-live="polite">
    <section id="phase-discover" data-bp-panel><span class="bp-status" data-tone="cyan">NO EXTERNAL WORK</span><h2>Discover and admit</h2><p>Compare archive metadata signatures, evaluate readiness, and apply automation limits.</p></section>
    <section id="phase-prepare" data-bp-panel hidden><span class="bp-status" data-tone="amber">IMMUTABLE INPUTS</span><h2>Prepare artifacts</h2><p>Pin revisions, render the manifest, patch the graph, and persist checksums.</p></section>
    <section id="phase-submit" data-bp-panel hidden><span class="bp-status" data-tone="red">SIDE-EFFECT BOUNDARY</span><h2>Persist, then submit</h2><p>A lost response becomes uncertainty and reconciliation, not an automatic retry.</p></section>
    <section id="phase-monitor" data-bp-panel hidden><span class="bp-status" data-tone="cyan">RECONCILE</span><h2>Observe external authorities</h2><p>Poll DIM or batched Slurm state and preserve raw plus normalized observations.</p></section>
    <section id="phase-terminal" data-bp-panel hidden><span class="bp-status" data-tone="green">LOCKED OUTCOME</span><h2>Complete, fail, or cancel</h2><p>Terminal locking prevents late pollers or stale workers from rewriting the result.</p></section>
  </div>
</div>

Public `status` is a compact projection such as `pending`, `running`, `awaiting_scheduler`, `not_submitted`, `completed`, `failed`, `retrying`, or `cancelled`. Exact progress lives in the control and external fields.

## External axes

| Axis | Representative states | Authority |
|---|---|---|
| Submission | `not_started`, `in_flight`, `submitted`, `uncertain`, `failed` | Beampipe's call record |
| Scheduler | `not_submitted`, `pending`, `running`, `succeeded`, `failed`, `cancelled`, `unknown` | Slurm observation |
| DALiuGE | `not_created`, `building`, `deploying`, `running`, `finished`, `failed`, `unreachable` | DIM/runtime observation |
| Outputs | `not_started`, `pending`, `verifying`, `verified`, `failed`, `unknown` | future verifier or external evidence |

The output axis exists in the ledger and reducer, but no worker currently performs output verification. Normal executions have `output_verification_required=false`; terminal runtime completion is therefore a control-plane result, not a native assertion about scientific products.

## Uncertainty and retry

<div class="bp-decision-table">
  <div class="bp-decision-table__head"><span>Observed state</span><span>Safe action</span></div>
  <div><span>Known pre-submit failure</span><span data-tone="green">retry from pinned artifacts</span></div>
  <div><span>submission uncertain</span><span data-tone="red">reconcile by stable identity</span></div>
  <div><span>external job or session exists</span><span data-tone="red">monitor or cancel</span></div>
  <div><span>consistent terminal failure</span><span data-tone="amber">classify, correct, then retry</span></div>
</div>

```bash
beampipe execution retry "$EXECUTION_ID" \
  --reason "Translator endpoint restored after maintenance"
```

Every retry requires a reason and increments `retry_count`. Continue with [Recovery and cancellation](../operations/recovery.md) for the operator procedure.

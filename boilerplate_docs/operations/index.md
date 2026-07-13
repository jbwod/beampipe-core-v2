---
hide:
  - toc
---

# Operator handbook

<p class="bp-lede">Operate from durable state outward: check the control plane, inspect its external axes, and act only when Beampipe can prove what the scheduler and DALiuGE have done.</p>

## Begin a shift

<div class="bp-check-grid">
  <div><span>01</span><strong>preflight</strong><code>beampipe doctor</code></div>
  <div><span>02</span><strong>queue</strong><code>beampipe status</code></div>
  <div><span>03</span><strong>workers</strong><code>beampipe worker list</code></div>
  <div><span>04</span><strong>external</strong><code>scheduler status --profile</code></div>
  <div><span>05</span><strong>console</strong><code>beampipe console</code></div>
  <div><span>06</span><strong>alerts</strong><code>/metrics + timeline</code></div>
</div>

The [daily workflow](operator-guide.md) explains the role contract and normal path. The console is an operational lens, while PostgreSQL remains authoritative.

## Triage by symptom

<div class="bp-switcher bp-terminal-frame" data-bp-switcher data-title="triage.selector">
  <div class="bp-segmented" role="tablist" aria-label="Operational symptom">
    <button type="button" role="tab" aria-selected="true" aria-controls="triage-backlog" id="tab-backlog" data-bp-target="triage-backlog">backlog grows</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="triage-submit" id="tab-submit" data-bp-target="triage-submit">submit uncertain</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="triage-daliuge" id="tab-daliuge" data-bp-target="triage-daliuge">DALiuGE unreachable</button>
    <button type="button" role="tab" aria-selected="false" aria-controls="triage-output" id="tab-output" data-bp-target="triage-output">outputs fail</button>
  </div>

  <section id="triage-backlog" role="tabpanel" aria-labelledby="tab-backlog" data-bp-panel>
    <span class="bp-status" data-tone="amber">QUEUE</span>
    <h2>Separate admission pressure from worker capacity</h2>
    <p>Check ready jobs, stale claims, pool labels, per-profile concurrency, and automation caps before adding workers.</p>
    <p><a href="workers-scheduling/">Workers and scheduling</a></p>
  </section>

  <section id="triage-submit" role="tabpanel" aria-labelledby="tab-submit" data-bp-panel hidden>
    <span class="bp-status" data-tone="red">FENCE</span>
    <h2>Reconcile before retrying</h2>
    <p>An SSH disconnect after <code>sbatch</code> is not proof of failure. Search by deterministic job identity and resolve the submission axis first.</p>
    <p><a href="recovery/#retry">Stage-aware retry procedure</a></p>
  </section>

  <section id="triage-daliuge" role="tabpanel" aria-labelledby="tab-daliuge" data-bp-panel hidden>
    <span class="bp-status" data-tone="cyan">EXTERNAL</span>
    <h2>Preserve scheduler and session facts independently</h2>
    <p>Probe translator and manager endpoints, inspect the persisted session identifier, and compare scheduler state before changing execution state.</p>
    <p><a href="daliuge-setonix/#live-inspection">DALiuGE live inspection</a></p>
  </section>

  <section id="triage-output" role="tabpanel" aria-labelledby="tab-output" data-bp-panel hidden>
    <span class="bp-status" data-tone="red">VERIFY</span>
    <h2>Do not equate scheduler success with scientific success</h2>
    <p>Inspect the output-verification axis, immutable artifacts, run record, and provenance before deciding whether work is retryable.</p>
    <p><a href="recovery/#investigate">Failure investigation</a></p>
  </section>
</div>

## Handbook map

| Operational need | Source of truth | Procedure |
|---|---|---|
| Watch active work | Execution ledger and external axes | [Live console](console.md) |
| Scale or drain workers | Claims, heartbeats, labels, pools | [Workers and scheduling](workers-scheduling.md) |
| Check translator, manager, SSH, or Slurm | Profile snapshot and adapter diagnostics | [DALiuGE and Setonix](daliuge-setonix.md) |
| Explain a failure | Structured diagnostic and provenance | [Recovery and cancellation](recovery.md) |
| Tune alerts and dashboards | Prometheus metrics | [Observability](observability.md) |
| Promote a release | Migration, backup, and preflight evidence | [Production runbook](production-runbook.md) |
| Rotate or restore | Database backup and external secret store | [Upgrades, backups, and secrets](upgrades-backups.md) |

## Operator rule

<div class="terminal-note" data-tone="amber">
<strong>When external state is uncertain, reconcile. Do not resubmit.</strong><br>
Beampipe persists intent before I/O and fences claims so that process restarts and additional workers do not turn uncertainty into duplicate scientific work.
</div>

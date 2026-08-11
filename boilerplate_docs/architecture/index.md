---
hide:
  - toc
---

# Architecture

Beampipe is the durable control plane between archive facts, project policy, worker side effects, and scientific runtimes. PostgreSQL records intent and evidence; external systems retain authority over data, allocations, sessions, and products.

## System boundary

<div class="bp-explorer bp-terminal-frame" data-bp-explorer data-title="system.boundary">
  <div class="bp-system-map" aria-label="Interactive Beampipe architecture">
    <button type="button" class="bp-system-node" data-tone="cyan" data-bp-target="arch-input" aria-pressed="false"><span>INPUT</span><strong>archive + YAML</strong><small>facts and policy</small></button>
    <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
    <button type="button" class="bp-system-node" data-tone="amber" data-bp-target="arch-control" aria-pressed="true"><span>CONTROL</span><strong>API + PostgreSQL</strong><small>intent and evidence</small></button>
    <span class="bp-flow-link" aria-hidden="true">&lt;--&gt;</span>
    <button type="button" class="bp-system-node" data-tone="green" data-bp-target="arch-worker" aria-pressed="false"><span>EFFECT</span><strong>workers</strong><small>leased and fenced</small></button>
    <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
    <button type="button" class="bp-system-node" data-tone="cyan" data-bp-target="arch-runtime" aria-pressed="false"><span>RUNTIME</span><strong>DIM or Slurm</strong><small>external authority</small></button>
  </div>

  <div class="bp-explorer-output" aria-live="polite">
    <section id="arch-input" data-bp-panel hidden><span class="bp-status" data-tone="cyan">VERSIONED</span><h2>Facts and policy enter separately</h2><p>CASDA and VizieR provide TAP rows. Project YAML provides queries, transforms, manifest shape, graph patches, and automation. Deployment profiles provide facility policy.</p></section>
    <section id="arch-control" data-bp-panel><span class="bp-status" data-tone="amber">AUTHORITATIVE</span><h2>PostgreSQL holds control-plane truth</h2><p>Sources, metadata, immutable revisions, profile snapshots, jobs, claims, executions, artifacts, observations, and provenance survive process restarts.</p></section>
    <section id="arch-worker" data-bp-panel hidden><span class="bp-status" data-tone="green">BOUNDED</span><h2>Workers perform side effects under leases</h2><p>Capability routing, renewable claims, fencing tokens, idempotency keys, and delayed retries make horizontal operation recoverable.</p></section>
    <section id="arch-runtime" data-bp-panel hidden><span class="bp-status" data-tone="cyan">RECONCILED</span><h2>DALiuGE and Slurm remain independent</h2><p>Beampipe stores external IDs and normalized observations. Scheduler state, DALiuGE state, and output facts are not collapsed into one success flag.</p></section>
  </div>
</div>

## Lifecycle

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Source lifecycle from registration through discovery preparation submission reconciliation and terminal state">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>register</strong><small>source identity</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>02</span><strong>discover</strong><small>query + sign</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>03</span><strong>prepare</strong><small>manifest + graph</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>04</span><strong>submit</strong><small>REST or Slurm</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>05</span><strong>reconcile</strong><small>poll external facts</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>06</span><strong>terminal</strong><small>durable outcome</small></div>
</div>

Discovery claims stale enabled sources, renders project-defined TAP queries, validates complete metadata snapshots, and computes canonical signatures. Changed sources become workflow-pending. Admission then applies project, environment, queue, and profile caps before an execution is created.

Preparation pins the project revision and profile snapshot, generates the manifest, resolves and patches the graph, and stores checksums. Submission intent is persisted before external I/O. Pollers update scheduler and DALiuGE observations until the reducer can choose a safe next state.

## Durable records

| Record | Why it matters |
|---|---|
| Project revision | reproduces query, metadata, manifest, graph, and automation policy |
| Deployment snapshot | reproduces translator, target, resources, and transport intent |
| Source and metadata | stable identity, readiness, discovery signature, claims |
| Execution ledger | current control phase plus independent external axes |
| Job, worker, lease | ownership, routing, retry, and recovery evidence |
| Artifact | content-addressed manifest and graph inputs |
| Observation and event | external facts and operator-readable history |

## Side-effect contract

1. Persist deterministic intent and external identity.
2. Acquire and renew a fenced lease.
3. Perform the external call.
4. Record the response or uncertainty.
5. Reconcile before retrying an ambiguous effect.

If a process stops after `sbatch` or DIM deployment but before recording the response, the execution is uncertain rather than automatically failed. A stale worker cannot commit after its lease is replaced.

## Integration boundaries

| Boundary | Rust contract | Normalized behavior |
|---|---|---|
| TAP | `TapClient` | rows, health, timeout, transient/permanent errors |
| Discovery | `DiscoveryRunner` | changed, unchanged, no datasets, error, timeout |
| Staging | `StagingClient` | usable and failed datasets without leaking credentials |
| DALiuGE | translator/manager traits | translate, deploy, inspect, cancel, typed failures |
| Scheduler | `SchedulerAdapter` | submit, batch status, accounting, queue, cancel, logs |

Vendor parsing stays behind adapters. Project policy stays in immutable YAML. Execution state receives normalized observations rather than service-specific response shapes.

## Workspace ownership

| Crate | Responsibility |
|---|---|
| `beampipe-api`, `beampipe-cli` | HTTP and operator surfaces |
| `beampipe-db`, `beampipe-domain` | durable repositories and state rules |
| `beampipe-project`, `beampipe-profiles` | typed project and backend policy |
| `beampipe-adapters`, `beampipe-orchestration` | TAP, staging, DALiuGE, SSH, Slurm |
| `beampipe-jobs` | scheduler ticks, claims, dispatch, polling |
| `beampipe-security`, `beampipe-auth` | secret policy, redaction, users, JWT |
| `beampipe-metrics`, `beampipe-alerts` | telemetry and notifications |

No Python sidecar, Redis queue, or Restate bridge is required. Redis is optional for distributed login rate limiting.

Continue with the [execution state model](state-machine.md) for exact axes or [Project YAML](../project-configs/index.md) for the dynamic survey contract.

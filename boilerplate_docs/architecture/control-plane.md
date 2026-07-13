# Control-plane boundaries

Beampipe coordinates archive discovery and workflow execution without becoming the archive, scheduler, or science runtime. Its authority is durable intent: what was requested, which immutable inputs were used, what side effect was attempted, and what each external system later reported.

## Ownership map

<div class="bp-lane-diagram" aria-label="Ownership boundaries across external systems and Beampipe">
  <div class="bp-lane-diagram__label" data-tone="cyan">OBSERVE</div>
  <div class="bp-lane-diagram__content"><strong>CASDA</strong><span>archive metadata, products, access facts</span></div>
  <div class="bp-lane-diagram__label" data-tone="amber">OWN</div>
  <div class="bp-lane-diagram__content"><strong>Beampipe</strong><span>source identity, config/profile revisions, admission, jobs, claims, artifacts, provenance</span></div>
  <div class="bp-lane-diagram__label" data-tone="green">EFFECT</div>
  <div class="bp-lane-diagram__content"><strong>workers</strong><span>discovery, graph preparation, submission, polling, verification</span></div>
  <div class="bp-lane-diagram__label" data-tone="cyan">RECONCILE</div>
  <div class="bp-lane-diagram__content"><strong>Slurm + DALiuGE</strong><span>allocations, sessions, runtime state, scientific outputs</span></div>
</div>

The [interactive architecture map](index.md) expands each boundary.

## Durable records

| Record | Why it exists | Stability |
|---|---|---|
| Project config revision | Reproduce discovery and manifest policy | Immutable after upload |
| Deployment profile snapshot | Reproduce backend and resource intent | Pinned by execution |
| Execution ledger | Hold current control and external state axes | Updated through validated transitions |
| Job and claim history | Prove worker ownership and recovery | Append-oriented audit |
| Manifest and graph artifacts | Reproduce translated inputs and mutations | Content-addressed / checksummed |
| Run record | Preserve backend identifiers and poll detail | Execution-scoped |
| Provenance event | Explain meaningful state and operator actions | Append-only narrative |

## Side-effect contract

Before a worker contacts a scheduler or DALiuGE manager, Beampipe persists intent and deterministic external identity. The worker then performs the effect under a leased claim and records the observation. This ordering makes three difficult cases recoverable:

1. The process stops before making the external call: persisted intent remains safe to resume.
2. The process stops after the call but before recording its result: submission becomes uncertain and must be reconciled.
3. A stale worker returns after another worker recovered the claim: the fencing token prevents stale state from being committed.

## Responsibilities

| Component | Responsibility |
|---|---|
| API | Authentication, source registry, operator intent, project/profile lifecycle, read models |
| PostgreSQL | Authoritative control state, queues, artifacts, claims, history, provenance |
| Worker | Typed adapter calls, deterministic preparation, bounded external effects, polling |
| Project config | Survey identity, archive queries, transforms, manifest shape, graph patches, automation |
| Deployment profile | Translator, manager, scheduler, resource, TLS, and facility configuration |
| Console | Live projection of durable and probed state; never an alternate source of truth |

## Explicit non-responsibilities

Beampipe does not evaluate the science graph, emulate Slurm, infer success from one backend response, store SSH private keys in project YAML, or reconstruct an execution from mutable active configuration.

## API boundary

The Rust API exposes `/api/v2` and generates OpenAPI from `utoipa`. Use the [API workflow](../api/index.md) for request order and the [API schema](../api/reference.md) for exact objects.

Next: follow data through [discovery and execution](lifecycle.md).

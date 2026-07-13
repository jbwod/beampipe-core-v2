# Discovery and execution

The lifecycle begins with a stable project/source identity and ends only when control state, external observations, artifacts, and provenance explain the outcome.

## End-to-end flow

<div class="bp-flow-diagram bp-flow-diagram--wrap" role="img" aria-label="Source discovery and execution lifecycle">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>register</strong><small>source identity</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>02</span><strong>discover</strong><small>archive metadata</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>03</span><strong>prepare</strong><small>manifest + graph</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>04</span><strong>submit</strong><small>scheduler intent</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>05</span><strong>reconcile</strong><small>Slurm + DALiuGE</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>06</span><strong>verify</strong><small>outputs + outcome</small></div>
</div>

## Discovery

Discovery is a repeatable metadata preparation pipeline, not an execution side effect.

| Step | Project control | Durable result |
|---|---|---|
| Build query | `discovery.queries` and source template variables | Rendered archive request context |
| Enrich | `discovery.enrichments` | Additional archive rows |
| Prepare fields | `prepare_metadata.field_map` and typed transforms | Normalized metadata |
| Derive flags | `prepare_metadata.discovery_flags` | Admission-relevant facts |
| Sign | `prepare_metadata.signature` | Stable change digest |

Exclude volatile archive fields such as access URLs, sizes, and timestamps from the signature when they should not trigger new scientific work.

## Admission and preparation

Admission checks automation policy, source readiness, queue pressure, and per-profile concurrency before creating runnable work. Preparation then pins the project revision and deployment-profile snapshot, renders the manifest, applies graph patches, and records checksums.

<div class="bp-artifact-strip" aria-label="Immutable preparation artifacts">
  <span><b>config</b><code>revision</code></span>
  <i aria-hidden="true">+</i>
  <span><b>profile</b><code>snapshot</code></span>
  <i aria-hidden="true">+</i>
  <span><b>manifest</b><code>sha256</code></span>
  <i aria-hidden="true">+</i>
  <span><b>graph</b><code>sha256</code></span>
</div>

## Submission and reconciliation

Submission intent is durable before I/O. Slurm job identity and DALiuGE session identity are persisted as soon as they are known. Pollers then update scheduler and DALiuGE axes independently; the reducer derives the next safe control action.

| Observation | Meaning |
|---|---|
| Scheduler running, DALiuGE building | Normal startup progression |
| Scheduler succeeded, DALiuGE running | Inconsistent; investigate rather than complete |
| SSH disconnected after `sbatch` | Submission uncertain; search before retry |
| DALiuGE finished, outputs unverified | Continue output verification |
| Outputs verified | Eligible for successful terminal outcome |

## Operator reading order

For a stalled execution, inspect:

1. Readiness and admission diagnostics.
2. Execution control phase and external axes.
3. Worker claim and heartbeat.
4. Scheduler job and DALiuGE session identifiers.
5. Immutable artifacts and run record.
6. Provenance timeline and metrics around the transition.

The [execution state model](state-machine.md) defines exact values and retry gates. [Deployment profiles](deployment-profiles.md) define the backend behavior pinned to each execution.

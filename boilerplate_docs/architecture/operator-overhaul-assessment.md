# Operator and DALiuGE overhaul: Phase 1 assessment

**Status:** Phase 1 complete  
**Beampipe baseline:** `b651a91`  
**DALiuGE baseline inspected:** `fbcd8c535e5069316cb72a3ecbeae6ec7135eccc`
(`v6.7.0` in the upstream `VERSION` files)

## Executive assessment

Beampipe already has the right high-level boundary: it is a Rust control plane backed by
PostgreSQL, while DALiuGE remains responsible for executing scientific graphs. The
workspace split, typed project configuration, archive adapters, deterministic manifest
construction, security helpers, metrics, and PostgreSQL claim patterns should be
retained.

The next phase should strengthen the contracts between those pieces rather than rewrite
them. Five issues block a polished operator platform today:

1. A process crash can strand a `running` queue job because expired job locks are not
   reclaimable.
2. The REST translation path depends on DALiuGE's legacy HTML-producing translator API,
   and manager responses are interpreted as loose JSON.
3. One coarse execution record mixes Beampipe phase, scheduler state, and DALiuGE
   session state; the REST path also stores a DALiuGE session ID in
   `scheduler_job_id`.
4. Configuration is mostly direct environment-variable access, so setup, doctor, API,
   workers, and future TUI views cannot share a resolved configuration model with
   source-aware diagnostics.
5. Operators cannot inspect worker ownership, leases, DALiuGE topology, scheduler
   allocation, graph changes, or durable execution artefacts from one coherent surface.

Phase 2 should therefore establish durable identities and state first. Building the TUI
or adding broad CLI commands before that would expose incomplete and sometimes
ambiguous state.

## Current architecture

```text
operator
  |-- beampipe CLI --------------------------|
  `-- Axum /api/v2 --------------------------+-- PostgreSQL
                                               |-- project configs and profiles
scheduler process -----------------------------|-- sources and metadata
  `-- enqueues recurring jobs                  |-- executions and provenance
                                               `-- PostgreSQL job queue
replicated Beampipe worker processes
  |-- claim queue jobs with SKIP LOCKED
  |-- CASDA/VizieR TAP adapters
  |-- manifest and graph preparation
  `-- execution backend
        |-- Translator Manager + DIM REST
        `-- SSH -> SLURM -> DALiuGE managers/nodes

Prometheus / OpenTelemetry <- API, scheduler, and worker instrumentation
Alertmanager / notification channels <- metrics and persisted alert rules
```

### Workspace map

| Crate | Current responsibility | Direction |
| --- | --- | --- |
| `beampipe-domain` | Execution, admission, readiness, SLURM identifiers, and run-record domain types | Retain; split external state axes and add typed failures |
| `beampipe-db` | SQLx models, repositories, migrations, claims, queue, provenance, alerts | Retain; add leases, workers, artefacts, snapshots, and observations |
| `beampipe-project` | Versioned YAML/JSON project model, validation, transforms, graph patches, WASM hooks | Retain the typed `beampipe.dev/v2` model |
| `beampipe-adapters` | TAP clients, async UWS, VOTable parsing, CASDA DataLink/staging | Retain behind explicit archive interfaces |
| `beampipe-orchestration` | Manifest/graph preparation, DALiuGE REST, SSH, SLURM, staging | Refactor into typed DALiuGE and scheduler adapter boundaries |
| `beampipe-jobs` | Queue dispatch, recurring scheduling, discovery, admission, execution, polling | Refactor around worker identity, renewable leases, and reconcilers |
| `beampipe-profiles` | Translation and REST/SLURM deployment profile types | Extend with versioned scheduler resources and capability requirements |
| `beampipe-config` | Environment-derived process settings | Replace internals with one layered, explainable settings model |
| `beampipe-api` | Authenticated `/api/v2` source, execution, project, profile, alert, and event routes | Retain Axum surface; add operator read models/actions |
| `beampipe-cli` | Server/worker, setup/doctor, project upload, status/timeline, admin tools | Retain command entry point; reorganise operator workflows |
| `beampipe-auth` | Passwords, JWTs, refresh/logout, request identity | Retain |
| `beampipe-security` | Secret references, redaction, SSH host/key policy | Retain and use from all output paths |
| `beampipe-metrics` | Prometheus, tracing context, OTLP, dependency and queue metrics | Retain; add lease/reconciliation metrics |
| `beampipe-alerts` | Webhook/email delivery and persisted rule evaluation | Retain; attach typed failure classifications |

The active product is the root Rust workspace. The untracked `beampipe-core/` tree is
not a member of that workspace and was not treated as a production implementation in
this assessment.

### Existing strengths

- Project configurations are versioned, validated, pinned to executions, and now expose
  typed v2 transforms, mappings, templates, and graph patches.
- Discovery and workflow source claims use PostgreSQL transactions,
  `FOR UPDATE SKIP LOCKED`, expiring tokens, and token-guarded updates.
- Queue jobs have idempotency keys, bounded attempts, scheduled retry times, and
  per-kind metrics.
- DALiuGE session names are derived deterministically from Beampipe execution identity.
- Graph preparation and manifest generation are isolated and testable.
- Production SSH policy includes strict known-host handling, key permission checks,
  secret references, and redaction.
- API, scheduler, and worker roles can be scaled separately from one image.
- Provenance, alerting, Prometheus, and trace-context plumbing already exist.

## Operator pain points

| Area | Current behaviour | Operator impact |
| --- | --- | --- |
| Installation | No `init` or `start`; setup writes a narrow `.env` and assumes surrounding files/services | A new operator must understand Compose, migrations, process roles, and environment names |
| Setup | Database/JWT/admin/project prompts do not model DALiuGE, SSH, SLURM, profiles, or secret providers | A successful setup does not imply a runnable scientific deployment |
| Doctor | Checks DB, migrations, JWT, optional Redis, TAP, and local SSH credential resolution | It does not prove TM/DIM, remote SSH, modules, directories, commands, SLURM, profile, graph, or worker health |
| Configuration | Settings are read directly from environment variables | There is no precedence, config provenance, profile resolution, redacted dump, or `explain` path |
| Status | CLI status/timeline are separate point queries | There is no single operational summary or stable JSON envelope for automation |
| Executions | Manifest is persisted, but prepared graph versions, checksums, remote paths, manager IDs, scheduler IDs, and phase timestamps are not first-class artefacts | Failure diagnosis and reproducibility require log archaeology |
| Processes | Beampipe worker processes are anonymous | Ownership, stale work, draining, capacity, and clock skew are invisible |
| Language | `worker`, scheduler job, and DALiuGE session can blur together | Operators cannot quickly identify which control layer is unhealthy |

The CLI and API should become two presentations of the same application services and
diagnostic records. Human output may be concise and styled; JSON output must preserve a
stable envelope, typed codes, remediation hints, and redaction.

## Verified DALiuGE contract

The following observations come from the official DALiuGE source at the revision above,
not inferred endpoint names:

| Surface | Verified contract | Beampipe today |
| --- | --- | --- |
| Translator, original API | `POST /gen_pgt` returns HTML and `GET /gen_pg` produces the physical graph | REST execution uses this path and extracts `pgtName` from HTML |
| Translator, updated API | JSON endpoints include `POST /lg_fill`, `/unroll`, `/partition`, `/unroll_and_partition`, and `/map`; `GET /api/submission_method` reports submission mode | SLURM translation already uses `/unroll_and_partition`; REST does not |
| Manager session lifecycle | `POST /api/sessions`, graph append, deploy, status, graph status, cancel, delete, graph retrieval, and logs are implemented | Create/append/deploy/poll/cancel/delete are used through loosely typed payloads |
| DIM topology | Composite managers expose `/api/nodes`, node forwarding, and past sessions | Not represented in health/status or persisted execution state |
| Session states | `PRISTINE=0`, `BUILDING=1`, `DEPLOYING=2`, `RUNNING=3`, `FINISHED=4`, `CANCELLED=5`, `FAILED=6` | Reduced immediately to a generic backend poll result |
| Scheduler deployment | Official tooling exposes `dlg remote-submit` / `create_dlg_job.py`; SLURM templates remain facility-specific | Beampipe invokes the tooling remotely and parses composite identifiers |

Primary upstream references:

- [Translator routes](https://github.com/ICRAR/daliuge/blob/fbcd8c535e5069316cb72a3ecbeae6ec7135eccc/daliuge-translator/dlg/dropmake/web/translator_rest.py)
- [Manager REST routes](https://github.com/ICRAR/daliuge/blob/fbcd8c535e5069316cb72a3ecbeae6ec7135eccc/daliuge-engine/dlg/manager/rest.py)
- [Session lifecycle](https://github.com/ICRAR/daliuge/blob/fbcd8c535e5069316cb72a3ecbeae6ec7135eccc/daliuge-engine/dlg/manager/session.py)
- [Manager OpenAPI contract](https://github.com/ICRAR/daliuge/blob/fbcd8c535e5069316cb72a3ecbeae6ec7135eccc/OpenAPI/manager_common.yaml)
- [SLURM deployment guide](https://github.com/ICRAR/daliuge/blob/fbcd8c535e5069316cb72a3ecbeae6ec7135eccc/docs/deployment/slurm_deployment.rst)

### Integration gaps

1. **Legacy translator coupling, high.** HTML parsing is a brittle control-plane
   dependency. Introduce a typed translator client using the updated JSON API, with an
   explicit compatibility adapter only where a tested deployed DALiuGE version requires
   it.
2. **No compatibility handshake, high.** Record translator and manager versions plus
   discovered capabilities. Refuse unsupported combinations with a diagnostic rather
   than guessing.
3. **Untyped status payloads, high.** A DIM can aggregate node/session status differently
   from a Node Manager. Decode known forms into typed DTOs, preserve the raw payload for
   audit, and classify unknown shapes as compatibility failures.
4. **Topology is invisible, medium.** Read DIM nodes for doctor/status, but keep normal
   deployment through the DIM. Beampipe should not become a replacement manager.
5. **Logs and graph snapshots are ephemeral, medium.** Persist references, checksums,
   retrieval timestamps, and bounded diagnostic excerpts.
6. **Translation and deployment are conflated, medium.** Separate translator,
   manager-session, and scheduler interfaces so each can be tested and reconciled
   independently.

Compatibility must be demonstrated by contract tests against pinned DALiuGE releases;
the inspected `master` revision alone is not a support range. Store the observed version
and API capabilities on every deployment attempt.

## Multi-worker risks

| Severity | Risk | Evidence and consequence | First mitigation |
| --- | --- | --- | --- |
| Critical | Queue jobs can remain `running` forever after worker loss | `claim_next_job` selects only `queued` rows, so an expired `locked_until` on a `running` row is never reclaimed | Add lease owner/token, renewal, and an atomic expired-lease recovery path |
| High | No process identity or heartbeat | Jobs and source claims cannot identify the responsible Beampipe worker process | Add `worker_instances`, heartbeat, role, pool, capabilities, started/version fields |
| High | External submission has a crash window | A worker can submit remotely and die before the scheduler/session identifier is durably acknowledged, allowing a duplicate attempt | Persist submission intent and deterministic external key before I/O; reconcile before retry |
| High | Source claims are not renewable | Long discovery/admission work can outlive a fixed expiry and be claimed twice | Renew token-guarded claims or subdivide work below the lease duration |
| High | Error classification is string-oriented | Retry policy cannot reliably distinguish transient dependency, invalid input, conflict, cancellation, or permanent remote failure | Use typed error classes and policy-driven backoff with jitter |
| Medium | No draining or capability assignment | Shutdown can interrupt owned work; any process may claim work it cannot execute safely | Claim by pool/capability and stop new claims before graceful lease handoff |
| Medium | Poison work is only an exhausted retry count | There is no explicit dead-letter state or operator resolution workflow | Persist terminal failure class, attempts, next action, and safe retry eligibility |
| Medium | Core provenance writes are best effort | State may advance without a corresponding audit event | Write state transition and event in one transaction; publish notifications afterward |

The existing source token checks and queue idempotency keys are good foundations. They
should be extended, not replaced by an external broker in this overhaul.

## Scheduler interaction risks

- A deployment profile does not model partition, QoS, CPUs/tasks, memory, GPU/generic
  resources, constraints, reservation, or a typed environment/module/container plan.
- `scheduler_job_id` contains either a DALiuGE session ID or a composite SLURM value.
  This prevents stable joins and forces parsing at multiple call sites.
- Remote submission relies on generated command strings and output parsing. The DALiuGE
  CLI should remain the implementation mechanism where appropriate, but it must sit
  behind typed request/result/error contracts with captured command version and redacted
  output.
- Scheduler states are collapsed too early. Preserve raw scheduler state/reason and map
  it to a normalized state without losing information.
- There is no durable submit intent, acknowledgement timestamp, last observation,
  scheduler allocation snapshot, or reconciliation cursor.
- Remote directories and logs are embedded in composite strings or manifests rather
  than managed as typed artefacts with retention and cleanup policy.
- The production Compose overlay requires a JWT secret but can still inherit the
  development PostgreSQL password unless the operator overrides the complete database
  configuration.

The scheduler boundary should expose operations such as validate profile, submit,
inspect, cancel, fetch logs, and reconcile. DALiuGE-on-SLURM remains one scheduler
payload; Beampipe must not reimplement DALiuGE's intra-allocation manager topology.

## Target architecture

```text
Presentation
  CLI human/JSON | TUI | Axum API/OpenAPI
                  |
Application services and read models
  setup | diagnostics | discovery | preparation | submission | reconciliation
                  |
Domain contracts
  execution reducer | diagnostics | failure classes | artefacts | capabilities
                  |
Coordination and persistence
  PostgreSQL queue + renewable leases + worker registry + transactional events
                  |
Integration adapters
  Archive/TAP | DALiuGE translator | DALiuGE manager | Scheduler | SSH | secrets
```

### Boundary decisions

**Retain**

- The modular Rust workspace and the control-plane/execution-engine separation.
- PostgreSQL as the durable coordination authority.
- The typed v2 project model, pinned project config versions, adapters, manifests,
  security policy, metrics, alerts, and existing idempotency keys.

**Refactor**

- Queue claims into renewable, owner-fenced leases.
- Execution handling into explicit preparation, submission, scheduler observation,
  DALiuGE observation, cancellation, and terminal outcome transitions.
- Process settings into one layered model used by CLI, API, scheduler, and workers.
- CLI/API handlers into shared application services and diagnostic output types.

**Wrap**

- DALiuGE translator and manager APIs with separate typed clients, capability discovery,
  bounded timeouts, raw-response capture, and contract fixtures.
- SSH/SLURM operations behind a scheduler adapter that returns typed identifiers,
  states, reasons, allocation, paths, and retry classes.
- Artefact storage behind a small interface; begin with PostgreSQL metadata and a local
  filesystem implementation, leaving object storage optional.

**Replace**

- Legacy HTML translator response parsing in the supported path.
- Multiplexed scheduler/session identifiers and repeated composite-string parsing.
- Retry decisions based on display strings.
- Direct environment reads outside the configuration crate.
- Best-effort audit writes for scientifically meaningful state transitions.

### Durable model additions

Use additive migrations and backfill current rows:

- `worker_instances`: stable process ID, role, pool, capabilities, version, host,
  start/heartbeat/drain/stop timestamps, and clock observation.
- Queue lease fields: `lease_owner`, opaque `lease_token`, `lease_expires_at`,
  `heartbeat_at`, failure class, and dead-letter metadata. Every completion/update must
  be fenced by the token.
- Separate external identifiers: `daliuge_session_id`, `scheduler_job_id`, manager URL,
  remote session directory, and submitted profile snapshot/version.
- Execution observations: normalized and raw scheduler/DALiuGE state, reason, observed
  time, source version, and reconciliation attempt.
- Execution artefacts: kind, URI/storage key, media type, SHA-256, size, creation time,
  producer phase, and optional redacted metadata. Include source graph, patched logical
  graph, translated graph, manifest, graph diff, submission request, and log references.
- Transition events written transactionally with the state they describe.

### State model

Do not force three systems into one enum. Keep independent, typed axes:

| Axis | Examples |
| --- | --- |
| Beampipe phase | preparing, ready, submitting, reconciling, cancelling, terminal |
| Submission state | not requested, intent recorded, acknowledged, uncertain, rejected |
| Scheduler state | pending, running, succeeded, failed, cancelled, timeout, unknown |
| DALiuGE session state | pristine, building, deploying, running, finished, cancelled, failed, unknown |
| Outcome | succeeded, failed, cancelled, inconsistent |

A pure reducer should derive the operator-facing execution state from persisted facts.
An `unknown` or conflicting external state remains visible and triggers reconciliation;
it must not be silently converted into success or a retry.

## Migration sequence

### Phase 2: foundations

1. Introduce shared `Diagnostic`, `FailureClass`, redaction-safe error context, and
   consistent human/JSON envelopes.
2. Add the layered settings model and config-source reporting while preserving current
   environment variable names.
3. Add worker registry and fenced queue leases, including recovery of existing expired
   `running` jobs and deterministic race tests.
4. Add separate execution identifiers, observations, profile snapshots, artefact
   metadata, and transactional transition events through additive migrations.
5. Split `beampipe-orchestration` behind typed `DaliugeTranslator`, `DaliugeManager`,
   and `Scheduler` traits. Add fixtures from verified upstream contracts before changing
   production routing.
6. Document and test the execution reducer and cancellation/retry rules.

**Gate:** existing v2 project/discovery/execution tests pass; an expired lease is
recovered exactly once; no supported DALiuGE translation path parses HTML; old database
rows remain readable.

### Phase 3: operator workflow

1. Build `init`, expanded `setup`, profile/project add/list/show/validate commands, and
   `start` on the shared services.
2. Expand `doctor --profile --json` into local config, DB/migrations, TAP, TM, DIM,
   topology, SSH, remote prerequisites, SLURM, graph, worker, and clock-skew checks.
3. Add redacted config dump/explain and secret-provider references; never echo secret
   values.
4. Add execution prepare/preview showing manifest, graph diff, checksums, resource plan,
   and validation diagnostics before submission.

**Gate:** a clean checkout can be initialised and diagnosed without reading crate or
database internals; human and JSON modes report identical codes and state.

### Phase 4: distributed execution

1. Enable pools, capabilities, renewable claims, graceful draining, and worker
   inspection/actions.
2. Record submission intent before external I/O and reconcile uncertain outcomes using
   deterministic DALiuGE session names and scheduler metadata.
3. Normalize scheduler observations without discarding raw states/reasons; implement
   bounded backoff with jitter and typed retry policy.
4. Reconcile DALiuGE session and graph status independently from scheduler allocation.

**Gate:** kill/restart and concurrent-worker tests show no duplicate scientific
submission; stale ownership is visible and recoverable.

### Phase 5: terminal console

Build the TUI from stable API/application read models: overview, sources, executions,
workers, scheduler, DALiuGE, and events/logs. Actions call the same guarded services as
the CLI/API and require confirmation for destructive or scientifically meaningful
operations.

**Gate:** every displayed value is backed by persisted or live data with an observation
time; disconnected, stale, empty, loading, partial, and error states are explicit.

### Phase 6: hardening

Add DALiuGE version-matrix tests, PostgreSQL upgrade/backfill tests, worker race and
failure-injection suites, Compose smoke tests with real DALiuGE fixtures, scheduler
contract fixtures, OpenAPI snapshots, CLI/TUI snapshots, and performance checks. Finish
operator runbooks for backup, restore, upgrades, credential rotation, incident
diagnosis, reconciliation, and artefact retention.

## Decisions to validate during Phase 2

- Establish the supported DALiuGE release matrix from deployed WALLABY/Pawsey versions,
  then pin those images or source revisions in contract tests.
- Confirm the facility-approved DALiuGE SLURM invocation and modules/container runtime;
  keep these in versioned deployment profiles rather than generic orchestration code.
- Set lease durations from measured longest non-checkpointed operations and require
  renewal well before expiry.
- Choose the initial artefact root and retention policy. Checksums and metadata remain in
  PostgreSQL regardless of storage backend.
- Decide which configuration file location is the operator default; environment
  overrides must continue to work for containers and secrets.

These are deployment inputs, not reasons to delay the additive foundations.

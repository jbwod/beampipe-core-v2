# beampipe v2 — Standalone Rust application plan

**Status:** Draft plan (generated from full codebase review, May 2026)  
**Target:** Single static binary + PostgreSQL, config-upload plugin system, no Restate/ARQ/Docker stack for local dev  
**Replaces:** Python `beampipe-core` control plane (FastAPI + ARQ + Restate + entry-point plugins)

---

## Executive summary

Today beampipe is a capable but **operationally heavy** control plane: 8+ Docker services, dual workflow engines (ARQ + Restate), two Python apps (`web` + `beamcore_rs`), and survey logic shipped as pip-installed entry points.

**beampipe v2** is a **single Rust binary** that:

1. Serves the HTTP API and runs background workers in one process (optional `beampipe worker` for scale-out).
2. Uses **PostgreSQL** as the sole durable store (ledger, registry, deployment profiles, **versioned project configs**).
3. Replaces Restate with a **Postgres-backed job scheduler** (poll loops, idempotent steps, `next_poll_at`).
4. Replaces Python plugins with **uploaded project config** (YAML/JSON) + optional **WASM extensions** for surveys that outgrow declarative limits.
5. Keeps external integrations unchanged: CASDA, Vizier TAP, TM, DIM, Slurm SSH, DALiuGE on cluster.

**Not in scope for v2 binary:** Running DALiuGE on compute nodes (still `create_dlg_job` on HPC).

---

## Goals and non-goals

### Goals

| Goal | Success metric |
|------|----------------|
| Single-binary local dev | `beampipe serve` + Postgres only |
| Config-upload surveys | No pip install; `POST /v1/project-configs` |
| Feature parity | All production workflows: discover → execute → poll → terminal |
| Smaller footprint | &lt;100MB RAM idle vs multi-container stack |
| OpenAPI parity | Compatible `/api/v1` where practical |

### Non-goals (v2.0)

- Rewriting DALiuGE / TM / DIM
- Replacing astropy-heavy science code inside cluster jobs
- Multi-tenant SaaS hardening (keep simple JWT admin auth)
- CRUDAdmin UI port (API-only or minimal static admin later)
- Read the Docs / MkDocs port in Rust (static OpenAPI + existing docs site can remain separate)

---

## Current system inventory (what we must port or drop)

### Runtime stack today → v2 mapping

| Today | v2 |
|-------|-----|
| FastAPI `web` | `beampipe serve` — axum HTTP |
| ARQ `scheduler` + `worker` | Embedded tokio scheduler in same binary |
| Restate + `beamcore_rs` | Postgres job table + in-process poll loops |
| Redis queue (×3 pools) | **Optional** Redis for rate limits only; queue in Postgres |
| Redis cache | Drop or in-memory LRU |
| PostgreSQL | PostgreSQL (required) |
| Python entry points | Project config registry + WASM |
| `setup.py` + compose variants | `beampipe setup` + minimal compose (Postgres only) |

### Feature inventory — every domain area

#### 1. Authentication & users

| Feature | Today | v2 plan |
|---------|-------|---------|
| OAuth2 password login | `POST /api/v1/login` | Keep — `axum-login` or custom JWT |
| Refresh cookie | HttpOnly refresh | Keep |
| Logout + token blacklist | Postgres table | Keep — simplify schema |
| User CRUD | register, me, patch, delete | Keep minimal (admin + API users) |
| Superuser hard delete | `DELETE /db_user/{username}` | Keep for ops |
| Rate limiting | Redis per user/IP | Optional Redis or in-process token bucket |
| CRUDAdmin `/admin` | Python | **Defer** — CLI `beampipe admin` instead |

#### 2. Health & readiness

| Endpoint | Today | v2 |
|----------|-------|-----|
| `GET /health` | Liveness | Keep |
| `GET /ready` | DB + Redis | DB only (or optional Redis) |
| `GET /health/tap` | CASDA/Vizier TAP probe | Keep — async HTTP probes |

#### 3. Project modules → **project config registry**

| Today (Python) | v2 (config) |
|----------------|-------------|
| `beampipe.projects` entry points | `project_configs` table, versioned |
| `GET /projects` | List config IDs + active version |
| `GET /projects/contracts` | Schema validation report at upload |
| `GET /projects/contracts/{id}` | Same |
| Required hooks: `discover`, `prepare_metadata`, `manifest` | Declarative pipeline + WASM fallback |
| Optional: `graph_overrides_from_sources` | Declarative `graph.patches` with expressions |
| `REQUIRED_ADAPTERS` | `adapters: [casda, vizier]` |
| `GRAPH_GITHUB_URL` / `GRAPH_PATH` | `graph.url` or `graph.path` |
| `WORKFLOW_EXECUTION_AUTOMATION` | `automation.execution` block |
| `WORKFLOW_DISCOVERY_AUTOMATION` | `automation.discovery` block |

#### 4. Archive adapters

| Adapter | Today | v2 |
|---------|-------|-----|
| CASDA TAP | Python astroquery | `reqwest` + VOTable/CSV parser or thin HTTP TAP client |
| CASDA async staging | Python client | Port staging client; same XML job protocol |
| Vizier TAP | Python | Same TAP client trait |
| `beampipe.adapters` entry points | Python | Built-in adapters + config-declared TAP endpoints |

#### 5. Source registry

| Feature | API / logic | v2 |
|---------|-------------|-----|
| Register source | `POST /sources` idempotent 200/201 | Keep |
| Bulk register | `POST /sources/bulk` | Keep |
| List/filter | `project_module`, `enabled`, pagination | Keep |
| Get/update/delete | Standard CRUD | Keep |
| Metadata list | `GET /sources/{id}/metadata` | Keep |
| Executions per source | `GET /sources/{id}/executions` JSONB query | Keep SQL |
| Discovery trigger | `POST /sources/discover` | Keep — enqueue discovery job |
| Discovery claims | `discovery_claim_token`, expiry | Keep — `FOR UPDATE SKIP LOCKED` |
| Workflow pending | `workflow_run_pending`, claims | Keep |
| Staleness | `stale_after_hours`, signatures | Keep |
| Readiness gates | `source_readiness.py` rules | Port as pure Rust functions + tests |

#### 6. Discovery pipeline

| Step | Today | v2 |
|------|-------|-----|
| Cron schedule | ARQ `discover_schedule` | tokio interval per module policy |
| Admission gates | TAP health, queue depth, in-flight caps | Port gate chain |
| Claim stale sources | Postgres | Keep |
| Tap phase | `discover()` per source | Config-driven TAP queries |
| Persist phase | `prepare_metadata()` → archive_metadata | Declarative field maps |
| Outcomes | unchanged/changed/no_datasets/error | Keep enum |
| Mark workflow pending | On successful discovery | Keep |
| Restate `DiscoveryBatchWorkflow` | Durable two-phase | Single job row with phase column |

#### 7. Execution ledger

| Feature | Today | v2 |
|---------|-------|-----|
| Create execution | `POST /executions` | Keep |
| Prepare (dry run) | `POST /executions/prepare` | Keep |
| List/filter/sort | pagination, status, module | Keep |
| Get execution | + DIM URL enrichment | Keep |
| Ledger snapshot | `GET .../ledger-snapshot` | Keep |
| Summary | `GET .../summary` slurm/dim state | Keep |
| Status | `GET .../status` | Keep |
| Patch / cancel | FSM + scancel | Keep |
| Execute | `POST .../execute` 202 | Enqueue job row (not Redis) |

**Execution statuses (must match):**  
`pending`, `running`, `awaiting_scheduler`, `not_submitted`, `completed`, `failed`, `retrying`, `cancelled`

**Execution phases:**  
`stage_and_manifest`, `submit`

**FSM transitions:** Port exactly from `ledger/service.py` including locked terminal states, retry_count, timestamp side effects.

#### 8. Orchestration pipeline

| Phase | Module today | v2 crate |
|-------|--------------|----------|
| Stage CASDA | `staging.py` | `beampipe-orchestration::staging` |
| Build manifest | `manifest_builder.py` + project hook | Config transform engine |
| Graph inject | `manifest.py` beampipe-ingest | Port graph JSON patch logic |
| Graph overrides | patches by node name | Declarative + `$count(datasets)` expr |
| Resolve graph | GitHub fetch | `reqwest` + cache |
| Translate REST | `translate.py` + TM client | `TranslatorClient` trait |
| Translate Slurm | unroll_and_partition | Same trait |
| Deploy REST | `rest.py` + DIM client | `DimDeployClient` trait |
| Deploy Slurm | `slurm.py` + SSH | `SlurmDeployClient` trait |
| Poll REST | session + graph status | Poll job loop |
| Poll Slurm | squeue/sacct | Poll job loop |
| Cancel | scancel / DIM cancel | Keep |

#### 9. Deployment profiles

| Feature | Today | v2 |
|---------|-------|-----|
| CRUD API | `/deployment-profiles` | Keep |
| Translation block | metis/mysarkar, tm_url | Serde struct |
| `rest_remote` | dim_host, deploy_host, ports | Serde enum variant |
| `slurm_remote` | SSH + INI fields + facility enum | Serde enum + INI generator |
| Default per module | `is_default`, `project_module` | Keep |
| Validation | Pydantic | `validator` + JSON Schema at API |

#### 10. `beampipe_run_record`

Embedded JSON under `workflow_manifest.beampipe_run_record`:

| Sub-record | Merge triggers | v2 |
|------------|----------------|-----|
| `requested_sources` | execution create | Typed struct + serde_json merge |
| `slurm` | submit, each poll, terminal | Port all merge functions |
| `dim` | deploy, poll, terminal | Port |
| `restate` | timeout metadata | Rename to `scheduler` or keep for compat |

Hoist to API responses: `beampipe_run_record`, derived `slurm_state`, `dim_state`.

#### 11. Admission control & shaping

| Gate | Today | v2 |
|------|-------|-----|
| Project automation enabled | module dict | config `automation.execution.enabled` |
| Tick source/run limits | | Keep |
| Project in-flight cap | | Keep |
| Global in-flight cap | excludes `awaiting_scheduler` | **Critical invariant** |
| Min sources / max wait | | Keep |
| Queue depth | ARQ zcard | `SELECT COUNT(*) FROM jobs WHERE status=queued` |
| Rate budget | stub Lua | Implement or defer |
| Enqueue pacing | sleep ms | Keep |
| Skip reason telemetry | structured logs | Keep |

#### 12. Slurm client

| Capability | Today | v2 |
|------------|-------|-----|
| asyncssh SSH/SFTP | `slurm_client/client.py` | `russh` or `async-ssh2-lite` |
| State normalization | `state.py` | Port + same test vectors |
| Scheduler job ID codec | `session:job\|dir` | Port exactly (512 char max) |
| INI generation | ConfigParser | `ini` crate or template |
| create_dlg_job argv | remote bash | Same command construction |
| manual_ssh CLI | dev tool | `beampipe slurm ping` subcommand |

#### 13. REST / DIM client

| Endpoint | v2 |
|----------|-----|
| TM: gen_pgt, gen_pg, unroll_and_partition | `TranslatorClient` |
| DIM: sessions, append, deploy, status, graph/status | `DimDeployClient` |
| DIM status classifier | Port `classify_dim_session_status` |

#### 14. Automation schedulers

| Job | Today | v2 |
|-----|-------|-----|
| Discovery cron | per module | tokio task |
| Execution cron | claim pending → create executions | tokio task |
| Slurm completion poll | Restate workflow | Job type `slurm_poll` on job table |

#### 15. API surface (full route list)

**Port all v1 routes unless marked defer:**

```
GET  /health, /ready, /health/tap
POST /login, /refresh, /logout
POST /user, GET /users, GET /user/me/, GET /user/{username}
PATCH /user/{username}, DELETE /user/{username}, DELETE /db_user/{username}
GET  /projects, /projects/contracts, /projects/contracts/{id}
POST /project-configs          # NEW — upload/replace survey config
GET  /project-configs/{id}     # NEW
GET  /project-configs/{id}/versions  # NEW
POST /sources, /sources/bulk, /sources/discover
GET  /sources, /sources/{id}, /sources/{id}/metadata, /sources/{id}/executions
PATCH /sources/{id}, DELETE /sources/{id}
POST /executions/prepare, /executions, /executions/{id}/execute
GET  /executions, /executions/{id}, .../ledger-snapshot, .../status, .../summary
PATCH /executions/{id}
GET  /deployment-profiles, POST /deployment-profiles
GET  /deployment-profiles/{id}, PATCH, DELETE
GET  /openapi.json, /docs      # utoipa + swagger-ui embed optional
```

**Defer:** `POST /tasks/task` (sample ARQ demo)

#### 16. HTML / misc

| Feature | v2 |
|---------|-----|
| `/sources` HTML view | Defer or static SPA later |
| OpenAPI export script | `beampipe openapi export` |

#### 17. Tests to port as golden fixtures

33 Python test modules → Rust integration tests prioritized:

1. FSM transitions + run_record merges  
2. Slurm client (mock SSH)  
3. Slurm state normalization  
4. Admission control / in-flight caps  
5. Source readiness  
6. Deployment profile validation  
7. Graph overrides  
8. DIM status classification  
9. Orchestration e2e (mocked HTTP/SSH)  
10. OpenAPI schema snapshot tests  

---

## Plugin system design (config + WASM)

### Design principles

1. **Config-first** — 80% of surveys never upload code.  
2. **Schema-validated at upload** — reject bad configs before they hit production.  
3. **Versioned** — immutable config versions; executions pin `project_config_version`.  
4. **WASM optional** — one module export: `transform_manifest` or full hook bundle.  
5. **No pip, no Docker rebuild** for survey changes.

### ProjectConfig schema (conceptual)

```yaml
apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata:
  id: wallaby_hires
  description: WALLABY HiRes CASDA pipeline

adapters:
  required: [casda, vizier]

graph:
  url: https://raw.githubusercontent.com/.../wallaby-hires.graph
  # or path: /data/graphs/wallaby.graph

discovery:
  queries:
    - name: visibility
      adapter: casda
      template: |
        SELECT o.* FROM ivoa.obscore o
        WHERE filename LIKE '{source_identifier}%'
        AND obs_collection IN ('ASKAP Pilot Survey for WALLABY', 'WALLABY')
    - name: ra_dec_vsys
      adapter: vizier
      template: |
        SELECT HIPASS, RAJ2000, DEJ2000, RV50max FROM "VIII/73/hicat"
        WHERE HIPASS = '{source_name}'
      source_id_transform: strip_hipass_prefix  # built-in

  enrichments:
    - name: sbid_to_eval_file
      adapter: casda
      for_each: sbid_from_visibility
      template: |
        SELECT * FROM casda.observation_evaluation_file WHERE sbid = '{sbid}'

  prepare_metadata:
    # Declarative mapping from TAP rows → archive_metadata rows
    field_map:
      sbid: { from: obs_id, transform: extract_askap_sbid }
      dataset_id: { from: filename }
      scan_id: { from: obs_publisher_did, transform: extract_scan_id }
    discovery_flags:
      requires: [has_staged_url, metadata_valid]

manifest:
  # Template for sources[] list in DALiuGE manifest
  group_by: [source_identifier, sbid]
  source_template:
    source_identifier: "{source_identifier}"
    ra_string: "{flags.ra_string}"
    dec_string: "{flags.dec_string}"
    vsys: "{flags.vsys}"
    sbids:
      - sbid: "{sbid}"
        evaluation_file_url: "{eval_urls[sbid]}"
        datasets:
          - name: "{dataset.name}"
            staged_url: "{staged_urls[scan_id]}"
            checksum_url: "{checksum_urls[scan_id]}"

graph_patches:
  - match: { node_name: "Scatter/GenericScatterApp/Beam" }
    set:
      num_of_copies: "$count(datasets)"   # expression language

automation:
  execution:
    enabled: true
    archive_name: casda
    deployment_profile: slurm-remote
    max_sources_per_execution: 1
    tick_execution_run_limit: 5
    concurrent_execution_run_limit: 5
    slurm_poll_max_rounds: 20
  discovery:
    enabled: true
    batch_size: 5
    tick_discovery_source_limit: 200
    stale_after_hours: 24

extension:
  wasm_sha256: null   # optional WASM module bytes stored separately
  hooks: []           # e.g. [custom_prepare_metadata] if wasm set
```

### Config engine (Rust)

| Component | Responsibility |
|-----------|----------------|
| `ProjectConfig` | Serde + JSON Schema generation |
| `TemplateEngine` | ADQL/query templates with `{source_identifier}` binding |
| `FieldMapper` | TAP row → metadata JSON (JMESPath / custom transforms) |
| `ManifestBuilder` | Apply `manifest` template to staged URL maps |
| `ExpressionEval` | `$count(datasets)`, `$sum(...)`, etc. for graph patches |
| `ConfigValidator` | Dry-run validate on upload; report like `/projects/contracts` |

### WASM extension (optional, Phase 4+)

| Aspect | Choice |
|--------|--------|
| Runtime | `wasmtime` |
| Interface | WIT (`beampipe-hooks.wit`) — discover, prepare, manifest as optional exports |
| Upload | `POST /project-configs/{id}/wasm` multipart |
| Sandbox | No network from WASM; host provides TAP results as JSON input |
| Fallback | If `extension.wasm_sha256` set, host calls WASM after declarative steps or replaces them |

**When WASM is needed:** wallaby-level `prepare_metadata` loops, custom enrichment logic, non-standard manifest shapes.

### Migration from Python modules

| Python hook | v1 path | Config equivalent |
|-------------|---------|-------------------|
| wallaby `discover` | ADQL + eval loop | `discovery.queries` + `enrichments` |
| wallaby `prepare_metadata` | astropy Table parsing | `prepare_metadata.field_map` + WASM if complex |
| wallaby `manifest` | sbid grouping | `manifest.group_by` + templates |
| wallaby `graph_overrides_from_sources` | count datasets | `graph_patches` + `$count(datasets)` |
| wallaby automation dicts | module constants | `automation.*` blocks |

Ship **reference config** `wallaby_hires.v2.yaml` validated against production behavior using existing pytest fixtures as golden JSON.

---

## Rust application architecture

### Binary layout

```
beampipe/
├── Cargo.toml                 # workspace
├── crates/
│   ├── beampipe-cli/          # clap subcommands
│   ├── beampipe-api/          # axum routes, middleware, OpenAPI
│   ├── beampipe-domain/       # FSM, readiness, admission gates, enums
│   ├── beampipe-db/           # sqlx, migrations, repositories
│   ├── beampipe-jobs/         # job queue, scheduler ticks, poll loops
│   ├── beampipe-orchestration/# stage, manifest, translate, backends
│   ├── beampipe-adapters/     # casda, vizier TAP + staging
│   ├── beampipe-profiles/     # deployment profile types
│   ├── beampipe-project/      # config schema, engine, WASM host
│   ├── beampipe-auth/         # JWT, bcrypt, blacklist
│   └── beampipe-config/       # env settings (figment/envy)
├── migrations/                # sqlx migrate
└── wit/                       # WASM interface definitions
```

### Subcommands

```bash
beampipe serve              # API + embedded scheduler + job worker
beampipe worker             # optional: worker-only replica
beampipe migrate            # sqlx migrations
beampipe setup              # interactive first-run (admin, .env, postgres DSN)
beampipe openapi export     # write openapi.json
beampipe project validate -f wallaby.yaml
beampipe slurm ping --host ...  # SSH smoke test
```

### Process model

```
┌─────────────────────────────────────────────────────────┐
│                     beampipe serve                       │
├─────────────────────────────────────────────────────────┤
│  axum HTTP (8080)                                        │
│  tokio runtime                                           │
│    ├── JobWorker: claim jobs FROM jobs WHERE ...         │
│    ├── SchedulerTick: discovery + execution automation   │
│    ├── PollLoop: slurm/dim sessions (next_poll_at)      │
│    └── Admission: gate chain before enqueue              │
│  sqlx → PostgreSQL                                       │
│  optional: redis for rate limits                         │
└─────────────────────────────────────────────────────────┘
```

### Job table (replaces ARQ + Restate)

```sql
CREATE TABLE jobs (
  id UUID PRIMARY KEY,
  kind TEXT NOT NULL,  -- discover_batch | execute | slurm_poll | dim_poll
  payload JSONB NOT NULL,
  status TEXT NOT NULL,  -- queued | running | completed | failed
  execution_id UUID REFERENCES batch_execution_record(uuid),
  phase TEXT,            -- for multi-step discover: tap | persist
  attempts INT DEFAULT 0,
  next_run_at TIMESTAMPTZ,
  locked_until TIMESTAMPTZ,
  created_at TIMESTAMPTZ DEFAULT now()
);
```

Executions remain source of truth for business state; jobs are **implementation** for async work.

### Key Rust dependencies

| Concern | Crate |
|---------|-------|
| HTTP server | `axum`, `tower`, `tower-http` |
| HTTP client | `reqwest` |
| Postgres | `sqlx` with migrate |
| SSH | `russh` |
| JWT | `jsonwebtoken`, `argon2` or `bcrypt` |
| Config | `figment`, `serde`, `serde_yaml`, `schemars`, `jsonschema` |
| WASM | `wasmtime`, `wit-bindgen` |
| UUID | `uuid` (v7 feature) |
| Time | `time` |
| OpenAPI | `utoipa` |
| CLI | `clap` |
| Logging | `tracing`, `tracing-subscriber` |
| TAP/VOTable | `votable` or custom minimal parser |

---

## Database schema (port from Python)

### Tables to migrate (Alembic → sqlx)

| Table | Notes |
|-------|-------|
| `batch_execution_record` | JSONB sources, workflow_manifest; uuid7 |
| `source_registry` | claims, workflow pending |
| `archive_metadata` | unique (module, source, sbid) |
| `daliuge_deployment_profile` | JSONB translation + deployment |
| `user` | auth |
| `token_blacklist` | JWT revocation |
| **NEW** `project_configs` | id, version, spec JSONB, active flag, uploaded_at |
| **NEW** `project_config_wasm` | optional bytes per version |
| **NEW** `jobs` | async work queue |

### Critical SQL to preserve

- Source→execution linkage via `jsonb_array_elements(sources)` 
- `FOR UPDATE SKIP LOCKED` on discovery/workflow claims
- In-flight count queries excluding `awaiting_scheduler`

---

## Phased implementation plan

### Phase 0 — Foundation (4–6 weeks)

- [ ] Cargo workspace + CI (fmt, clippy, test, sqlx check)
- [ ] Postgres migrations: core tables + `project_configs` + `jobs`
- [ ] `beampipe-domain`: `ExecutionStatus`, `ExecutionPhase`, FSM with Python parity tests
- [ ] `beampipe-db`: repositories for executions, sources, profiles
- [ ] `beampipe-config`: env loading from template equivalent
- [ ] `beampipe serve` skeleton: health, ready

**Exit criteria:** FSM tests pass; migrations run; health endpoints work.

### Phase 1 — Project config system (3–4 weeks)

- [ ] Define `ProjectConfig` JSON Schema + Rust types
- [ ] `POST /project-configs` upload, validate, version
- [ ] `GET /projects/contracts/{id}` validation report
- [ ] Template engine for discovery queries (no execution yet)
- [ ] Port wallaby config as reference YAML
- [ ] Expression evaluator for `$count(datasets)`

**Exit criteria:** Upload wallaby YAML; validation passes; contracts API matches Python shape.

### Phase 2 — Read API + auth (3–4 weeks)

- [ ] JWT auth (login, refresh, logout, blacklist)
- [ ] GET sources, executions, deployment profiles, projects
- [ ] Pagination helper matching fastcrud response shape
- [ ] OpenAPI via utoipa; snapshot test vs current spec

**Exit criteria:** Bruno collection read-only requests work against Rust server.

### Phase 3 — Discovery pipeline (4–6 weeks)

- [ ] CASDA + Vizier TAP clients
- [ ] Config-driven discover + prepare_metadata (field maps)
- [ ] Discovery job worker; claims; archive_metadata persist
- [ ] Scheduler tick with admission gates
- [ ] `POST /sources`, `/sources/discover`, registry CRUD

**Exit criteria:** Register source → discover → metadata in DB using config-only wallaby.

### Phase 4 — Staging + manifest (4–5 weeks)

- [ ] CASDA async staging port
- [ ] Config manifest builder
- [ ] Graph fetch + beampipe-ingest injection + graph patches
- [ ] `prepare` + `create execution` API

**Exit criteria:** Prepare + create execution with manifest JSON equivalent to Python golden fixtures.

### Phase 5 — Orchestration backends (6–8 weeks)

- [ ] TM translator client (REST + Slurm paths)
- [ ] `rest_remote`: DIM deploy + poll
- [ ] `slurm_remote`: SSH, INI, create_dlg_job, sbatch
- [ ] `beampipe_run_record` merge logic
- [ ] Execute job: full pipeline
- [ ] Slurm/DIM poll jobs replacing Restate workflows
- [ ] Cancel + scancel

**Exit criteria:** `test_orchestration_slurm_e2e` and REST translate equivalents pass (mocked).

### Phase 6 — Automation + admission (3–4 weeks)

- [ ] Execution scheduler (claim pending → create → execute)
- [ ] Discovery scheduler cron
- [ ] Full admission gate chain + skip reasons
- [ ] Shaping / in-flight caps

**Exit criteria:** Admission control tests ported.

### Phase 7 — WASM extensions (optional, 3–4 weeks)

- [ ] WIT interface for custom prepare/manifest
- [ ] wasmtime host; upload API
- [ ] wallaby WASM only if declarative config insufficient

### Phase 8 — Hardening + cutover (4+ weeks)

- [ ] `beampipe setup` wizard
- [ ] Docker: Postgres-only compose
- [ ] Migration tool: export Python Postgres → import (if needed)
- [ ] Load testing Slurm poll loops
- [ ] Documentation update
- [ ] Parallel run Python vs Rust; diff ledger snapshots

**Exit criteria:** Production trial on Setonix path; operator sign-off.

---

## What we deliberately remove

| Component | Reason |
|-----------|--------|
| Restate | Postgres jobs + idempotent steps sufficient |
| ARQ | Job table |
| `beamcore_rs` second app | Merged into binary |
| Redis queue | Postgres job queue |
| Redis cache | Not used on hot path |
| Triple Redis pools | Simplify to 0–1 Redis |
| CRUDAdmin | CLI/API sufficient for v2 |
| FastAPI boilerplate user tiers | Out of scope |
| Sample background task API | Demo only |
| `navigation.instant` docs issues | N/A for Rust |

---

## Risk register

| Risk | Mitigation |
|------|------------|
| Config DSL too weak for wallaby | WASM escape hatch; reference wallaby YAML drives DSL features |
| Config DSL too powerful (bad DSL) | Strict schema; limit expressions; good error messages on upload |
| TAP/VOTable parsing without astropy | Golden fixtures; integration tests against CASDA dev |
| Slurm SSH edge cases | Port existing test vectors; manual_ssh → `beampipe slurm ping` |
| uuid7 compatibility | Use same uuid7 crate/format as Python |
| API breaking changes | Version `/api/v1`; OpenAPI diff in CI |
| Rewrite takes too long | Phase 2 read API enables incremental Bruno testing; run Python alongside |
| Astronomers resist YAML | Web UI for config upload later; start with wallaby example |

---

## Success metrics

| Metric | Target |
|--------|--------|
| Local dev services | 1 binary + Postgres |
| Cold start RAM | &lt; 150 MB |
| Time to first execution (dev) | ≤ current after `beampipe setup` |
| Survey onboarding | Upload YAML, no pip install |
| Test parity | ≥ 90% of orchestration/admission tests ported |
| Slurm E2E | Pass with mocked SSH |

---

## Open decisions

1. **Rust vs Go** — This plan assumes Rust (WASM, footprint). Go remains valid if velocity beats WASM (see prior discussion).
2. **SQLite lite mode** — Optional embedded DB for demos without Docker Postgres?
3. **API compatibility** — Strict v1 parity vs clean `/api/v2`?
4. **Auth** — Keep JWT+cookie or switch to API keys for machine-to-machine?
5. **Rate limiting** — Require Redis or pure in-process?
6. **Python sidecar** — Temporary `beampipe-py-runner` for manifest during Phase 4–5?

---

## Appendix A — File mapping (Python → Rust)

| Python | Rust target |
|--------|-------------|
| `src/app/models/ledger.py` | `beampipe-domain::execution` |
| `src/app/core/ledger/service.py` | `beampipe-db::execution_repo` + FSM |
| `src/app/core/ledger/run_record.py` | `beampipe-domain::run_record` |
| `src/app/core/ledger/source_readiness.py` | `beampipe-domain::readiness` |
| `src/app/core/orchestration/service.py` | `beampipe-orchestration::pipeline` |
| `src/app/core/orchestration/staging.py` | `beampipe-orchestration::staging` |
| `src/app/core/orchestration/manifest*.py` | `beampipe-orchestration::manifest` |
| `src/app/core/orchestration/rest.py` | `beampipe-orchestration::backends::rest` |
| `src/app/core/orchestration/slurm.py` | `beampipe-orchestration::backends::slurm` |
| `src/app/core/orchestration/slurm_client/*` | `beampipe-orchestration::slurm_client` |
| `src/app/core/archive/adapters/casda/*` | `beampipe-adapters::casda` |
| `src/app/core/projects/*` | `beampipe-project::*` |
| `src/app/schemas/daliuge.py` | `beampipe-profiles::*` |
| `src/app/restate_workflows/*` | `beampipe-jobs::workflow_steps` |
| `src/app/core/worker/*` | `beampipe-jobs::*` |
| `src/app/core/shaping/*` | `beampipe-domain::admission` |
| `src/app/api/v1/*` | `beampipe-api::routes` |

---

## Appendix B — Invariants checklist (must not break)

- [ ] `execution_phase=submit` skips re-staging when manifest present
- [ ] `awaiting_scheduler` excluded from in-flight automation counts
- [ ] Composite `scheduler_job_id` format and 512-char limit
- [ ] Slurm PGT filename uses `BeampipeExecution_{uuid}.pgt.graph`
- [ ] Terminal poll clears `workflow_run_pending` on all execution sources
- [ ] `failed→retrying→running` increments `retry_count`
- [ ] `completed`/`cancelled` status locked
- [ ] CASDA staging failures → `exclude_sbids`, not fatal if other sources ready
- [ ] Restate 409 equivalent: idempotent job enqueue (dedupe by execution_id + kind)
- [ ] Default deployment profile resolution order: explicit → module default → global default

---

## Appendix C — Related documents

- [`dev/combined-pr-issue-11-12.md`](combined-pr-issue-11-12.md) — Current feature inventory (Python)
- [`boilerplate_docs/project-modules/lifecycle.md`](../boilerplate_docs/project-modules/lifecycle.md) — Hook lifecycle
- [`boilerplate_docs/deployment-profiles/index.md`](../boilerplate_docs/deployment-profiles/index.md) — Profile shapes

---

*Generated from agent review of the full beampipe-core codebase. Update this plan as phases complete or decisions are made.*

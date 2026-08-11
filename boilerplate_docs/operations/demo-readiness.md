# Demo readiness assessment

This assessment turns the [current system audit](../architecture/current-system-audit.md)
into a completion plan for a repeatable end-to-end demonstration. It was
prepared on 2026-08-11 from Git commit `d908efe` and direct inspection of the
tracked repository.

## What "end to end" means

Beampipe has several legitimate demonstration boundaries. They should be named
explicitly because passing one does not prove the next.

| Level | Demonstration | Required evidence | Current verdict |
|---|---|---|---|
| L0 | Build and contract | Workspace tests, migrations, config validation, OpenAPI | Implemented; local library tests passed, DB/container checks rely on CI |
| L1 | Deterministic local control plane | Register -> fixture discovery -> prepare -> manifest/graph -> mock backend -> terminal record | **Blocked** by the lack of a runtime fixture discovery adapter |
| L2 | Live archive preparation | Real CASDA/VizieR discovery, staging, manifest, graph patch and checksums | Implemented in code; needs a controlled integration proof |
| L3 | Live facility execution | TM translation, Setonix submission, Slurm and DALiuGE reconciliation | Implemented in code; needs a controlled facility proof and hardened harness |
| L4 | Scientific completion | Expected outputs independently located and validated before success | **Blocked** because output verification is modeled but not executed |

The target "full demo" in this document is L4. An L1 demo should be completed
first because it is the fast, repeatable regression path. L2 and L3 then prove
the real external contracts without conflating network availability with core
control-plane behavior.

## P0 completion gaps

### DEMO-001: runtime fixture discovery

**Finding.** `BEAMPIPE_USE_REAL_BACKENDS=false` selects mock execution clients,
but every production worker still constructs `ConfigDiscoveryRunner` and calls
configured TAP services. `DeterministicDiscoveryRunner` is available only to
tests and returns `no_datasets`, which cannot make a source execution-ready.

**Impact.** The documented five-minute environment can start safely, but it
cannot perform the first-workflow sequence without live CASDA and VizieR. There
is no deterministic, offline L1 demonstration.

**Required change.** Add an explicit development-only fixture TAP/discovery
adapter selected by project config or a tightly scoped setting. It must return
versioned fixture rows for a known source, exercise the same transforms and
persistence path as live TAP, and be rejected in production. Do not bypass the
discovery claim, signature, metadata, or readiness repositories.

**Acceptance.** From an empty database and with network disabled, one command
registers the fixture source and the normal scheduler produces metadata,
discovery flags, a stable signature, and `workflow_run_pending=true`. A second
discovery is unchanged. The resulting source passes `/executions/prepare`.

### DEMO-002: output-verification worker path

**Finding.** `OutputState`, `output_verification_required`,
`ControlPhase::OutputVerification`, and `ReconciliationAction::VerifyOutputs`
exist. Worker capability lists include `output-verification`. No job kind,
adapter, project schema, or handler verifies expected outputs or advances the
output axis to `verified` or `failed`. The WALLABY config does not define an
output contract.

**Impact.** A scheduler/DALiuGE completion can become successful when output
verification is not required, but that proves control-plane completion rather
than the expected scientific products. Current architecture prose overstates
this boundary.

**Required change.** Define a typed project-level output contract and an
`OutputVerifier` trait, enqueue a fenced `verify_outputs` job after external
completion, persist a verification report as an immutable artifact, and feed
its result through the existing reducer. Start with a deterministic filesystem
or object-store listing verifier; add content/schema checks required by the
WALLABY product contract.

**Acceptance.** Missing products end in `output_state=failed`; present and
valid products end in `output_state=verified`, `control_phase=terminal`, and
`terminal_outcome=succeeded`. Duplicate jobs and stale leases cannot rewrite a
terminal result. The artifact and provenance stream explain every checked
path, checksum, size, and failure without exposing signed credentials.

### DEMO-003: immutable graph input

**Finding.** Project `GraphConfig` accepts only `url` or `path`. The reference
WALLABY project points to a mutable GitHub branch URL, fetched when execution
preparation runs. A local path must exist independently on every eligible
worker. Beampipe checksums and stores the graph after resolution, but config
upload does not pin or prefetch the source bytes.

**Impact.** Identical active project revisions can prepare different graphs if
the remote branch changes. A network failure can also break an otherwise local
demo after discovery succeeds.

**Required change.** Support an uploaded/content-addressed graph artifact or a
required expected SHA-256 for URL/path sources. Validate graph availability and
checksum at project activation. Bundle a small fixture graph for L1 and pin the
production WALLABY graph to immutable content.

**Acceptance.** Project activation fails on a missing or mismatched graph.
Workers resolve the same bytes on separate hosts. The execution's
`source_graph_sha256` equals the activated project contract, and the L1 demo
does not require internet access.

### DEMO-004: one tracked, fail-fast demo harness

**Finding.** Two substantial E2E shell scripts exist in the working tree but
are untracked and therefore absent from releases and CI. They overlap, mutate a
shared database, use different process models, manually enqueue poll ticks,
manually create executions while automation is enabled, and declare success
before asserting DALiuGE and scientific output completion.
One host SSH probe uses `StrictHostKeyChecking=accept-new`, which is weaker than
the application's production policy and does not prove the Rust SSH path.

**Impact.** There is no versioned command that produces a trustworthy demo
result or a machine-readable evidence bundle.

**Required change.** Replace the drafts with one tracked harness supporting
explicit `fixture`, `live-prepare`, and `live-setonix` modes. Use only Beampipe
preflight commands and API state, create a dedicated database or Compose
project, use bounded polls based on profile wall time, trap cleanup, and write a
redacted evidence bundle under an operator-selected output directory.

**Acceptance.** CI runs fixture mode from a clean checkout. Live modes refuse
to start unless their exact preconditions pass. Exit zero requires all
mode-specific assertions; timeout, failed, cancelled, inconsistent, or
unverified states exit non-zero.

### DEMO-005: bootstrap and profile contract alignment

**Finding.** Non-interactive `beampipe setup --yes` defaults to a project-scoped
`local-daliuge` REST profile. The WALLABY automation config and first-workflow
example request `slurm-remote`. The example does not install that profile.
When an explicit profile name is not found, execution creation currently stores
no profile ID instead of rejecting the request; the worker can then fall back
to a different project default.
Additionally, setup calls `doctor` without the newly installed profile name,
while doctor without `--profile` selects only a global default. It therefore
does not perform the profile-specific SSH/DIM checks for the project-scoped
profile setup just installed.

**Impact.** A fresh operator can follow the documented path and silently route
an execution through a profile other than the one requested, or receive a
successful setup report that did not test the selected facility profile.

**Required change.** Reject an unknown explicit profile name during execution
creation. Give setup an explicit demo mode/profile contract, invoke doctor with
the installed profile, honor exported secret-bearing settings before prompting
or accepting command-line values, and make project activation verify that an
automation profile exists and belongs to the same project or is global. Update
the first-run guide to create services in migration-safe order and use the
profile actually installed.

**Acceptance.** A clean setup either installs every referenced profile and
passes profile-specific checks or exits non-zero with one remediation command.
An unknown explicit profile returns a client error and creates no ledger row.
No first-run command names a profile that bootstrap did not create.

### DEMO-006: controlled live contract proof

**Finding.** The repository has mocked adapter tests and production client
implementations, but no checked-in result proving the exact deployed CASDA,
VizieR, TM, DALiuGE, Setonix, graph, and product-store versions together. This
audit environment also lacked Docker/PostgreSQL and facility access.

**Impact.** L2/L3 behavior is code-complete but operationally unqualified. API
shape drift, remote module changes, account policy, VPN routing, or a TAP schema
change could still stop the demonstration.

**Required change.** Establish a named demo source with known datasets and
expected products, a facility-approved profile template, a compatibility
matrix, and an expiring integration environment. Run the tracked harness once
per release candidate and retain only redacted reports and immutable IDs.

**Acceptance.** A release-candidate report records component versions, config
and profile hashes, discovery signature, artifact hashes, TM capability,
scheduler job ID, DALiuGE session ID, output-verification report, terminal
outcome, timings, and operator identity.

## P1 operational gaps

| ID | Gap | Completion test |
|---|---|---|
| DEMO-101 | Grafana is not packaged: no service, provisioning, or dashboard JSON is tracked, although Prometheus and alerts are. | `docker compose --profile observability up` starts Grafana with a provisioned Beampipe overview; API traffic, queue, workers, dependencies, discovery, and execution are visible without manual import. |
| DEMO-102 | Production Compose mounts the Slurm key but does not define/mount a CASDA password file secret. | Scheduler and worker resolve `CASDA_PASSWORD_FILE` from a read-only Compose secret and production security checks pass with no inline password. |
| DEMO-103 | No isolated/resettable demo database convention is documented or enforced. | Harness creates a uniquely named database/Compose project, records it, and refuses to delete a non-demo target. |
| DEMO-104 | Draft live polling is capped at roughly four minutes despite a 50-minute sample Slurm wall time and bypasses normal cadence with manual tick jobs. | Poll deadline derives from profile wall time plus startup allowance; recurring pollers are observed rather than manually injected. |
| DEMO-105 | The checked-in sample Slurm profile contains one operator's user, account, paths, and mutable TM hostname. | Repository provides a non-runnable template with placeholders; generated operator profiles stay outside version control. |
| DEMO-106 | Doctor checks reachability but not a known-source TAP query, CASDA stage/unstage permission, TM translation of the selected graph, or expected output-store write/read. | A separate destructive `demo preflight --live` performs opt-in contract probes and records redacted results. |

## Documentation defects found

These should be corrected with the implementation they describe:

- The five-minute page says it does not contact external systems, while the
  linked first-workflow page immediately triggers live TAP discovery.
- The first-workflow Compose sequence starts API and workers before applying
  migrations; the installation page has the safer order.
- The first workflow selects `slurm-remote` even though default local setup
  creates `local-daliuge`.
- CLI reference says `beampipe profile validate <file>`, but the command accepts
  an installed profile name. File validation occurs as part of `profile add`.
- Architecture pages imply output verification is active. They must distinguish
  a modeled future action from a persisted verification result.
- Observability docs mention dashboards generically, but the repository ships
  Prometheus and Alertmanager only; no Grafana runtime or dashboard is tracked.
- `setup` prints `deploy/ssh/README.md` as the SSH next step, but that file does
  not exist. SSH guidance currently lives under deployment profiles.

## Recommended implementation sequence

Keep each change independently reviewable and locally committed:

| Order | Proposed commit | Delivers |
|---|---|---|
| 1 | `feat(demo): add fixture discovery and graph artifacts` | DEMO-001 and local half of DEMO-003 |
| 2 | `fix(setup): align project and deployment profile preflight` | DEMO-005 plus first-run corrections |
| 3 | `test(e2e): add isolated fixture demo harness` | Fixture mode of DEMO-004 and CI coverage |
| 4 | `feat(outputs): verify project output contracts` | DEMO-002 |
| 5 | `feat(project): pin graph source artifacts` | Production half of DEMO-003 |
| 6 | `ops(observability): provision the demo monitoring stack` | DEMO-101 and DEMO-102 |
| 7 | `test(e2e): qualify live archive and Setonix modes` | DEMO-004, DEMO-006, and live preflight |
| 8 | `docs(demo): publish verified evidence and support matrix` | Final L4 qualification |

Do not combine facility credentials, generated profiles, live response payloads,
or scientific data in these commits. Store only templates, hashes, redacted
evidence, and reproducible assertions.

## Release gate

Call the demonstration complete only when all of the following are true:

- fixture mode passes from an empty database with network disabled;
- live discovery returns the expected source, SBIDs, flags, and stable signature;
- CASDA staging leaves at least one usable dataset and records excluded failures;
- graph and profile hashes match the activated contracts;
- TM and Slurm/DALiuGE identifiers are durable and independently inspectable;
- the normal pollers reach consistent external terminal states;
- the output-verification artifact passes and `output_state=verified`;
- the execution ends `terminal_outcome=succeeded` with no active claims;
- Prometheus has scraped API, scheduler, and worker targets for the run;
- the redacted evidence bundle contains no credentials or signed access URLs.

Until those gates pass, use the precise level label L0, L1, L2, or L3 rather
than describing the result as a full end-to-end scientific demonstration.
The exact gated procedure is [End-to-end scientific demo](end-to-end-demo.md).

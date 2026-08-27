# Changelog

All notable changes to Beampipe Core are documented here. This project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Corrected the standalone DALiuGE package, repository, and Python namespace
  spelling to `beampipe-palette` / `beampipe_palette`.
- Re-vendored the Wallaby graphs from `wallaby-hires` 0.1.19 with immutable
  `beampipe-palette` 0.5.1 component metadata and refreshed graph digests.

## [0.2.0] - 2026-08-27

Version 0.2.0 is a project-neutral control-plane release. It replaces the
remaining survey-specific and callback-based contracts with generic
groups/records, typed deployment policy, native DALiuGE components, and a
durable output-inventory handoff that Core reconciles over its existing SSH
transport.

### Upgrade notes

- This is a clean break for project configs and API clients. Use
  `beampipe.dev/v2`; replace SBID/dataset fields with `group_key`/`record_id`,
  and replace scalar worker requirements with `required_capabilities` arrays.
- Project configs must declare their adapters, endpoint modes, source identity,
  staging provider, graph patches, and output-verification policy explicitly.
  The old v1 compatibility parser and `source_id_transform` are removed.
- Remote Slurm publication no longer calls back to Core and no publisher token
  is injected into a graph. Install the public `beampipe-palette` 0.5.1 wheel
  in the remote DALiuGE runtime. The terminal publisher writes a canonical
  inventory handoff inside the execution session; Core retrieves and verifies
  it through the existing SFTP connection after scheduler completion.
- The publisher-token and output-callback API routes are removed. Regenerate
  API clients from the v0.2.0 OpenAPI document.
- Worker routing is all-of and namespaced. Workers must advertise every
  required capability, including the selected deployment and staging provider.
- Neutral setup installs no WALLABY, CASDA, Slurm, or DALiuGE deployment policy
  implicitly. Select `--sample wallaby-hires` or provide project/profile files
  explicitly.
- Apply all forward SQL migrations. Historical migrations remain immutable;
  the v0.2.0 migrations rename generic metadata fields, migrate routing
  capabilities, and remove the retired publisher-credential table.

### Added

- Project-scoped TAP adapters with arbitrary names, explicit sync/direct or
  async-job modes, bounded health probes, per-project caching, and real UWS
  create/start/poll/result/cleanup behavior.
- Generic project policy for source identity, query iteration and cardinality,
  metadata templates, staging providers, graph patches, manifests, and output
  patterns.
- Native `BeampipeIngestApp` and `BeampipePublishApp` DALiuGE components in the
  separately versioned `beampipe-palette` package, with EAGLE-compatible ports
  and project-owned pattern configuration.
- Required output-verification policy using
  `beampipe-output-inventory/v1`, exact expected-pattern counts, canonical
  product hashes, durable destination verification, and attempt/execution
  binding.
- Pull-only remote receipt reconciliation over bounded, policy-checked SFTP.
  Output verification remains pending and retryable until Core retrieves a
  valid attempt-scoped inventory.
- Typed project runtime contracts and provider-specific worker capabilities for
  REST DALiuGE, remote Slurm, generic manifest preparation, and optional CASDA
  staging.
- All-of worker routing with bounded unroutable-job diagnostics in the API,
  doctor output, metrics, alerts, and operator dashboard assets.
- A guided setup experience with review-before-write behavior, neutral defaults,
  secure generated credentials, explicit unattended mode, clearer progress,
  and project/profile-aware next actions.
- Complete OpenAPI response, path, query, binary-download, adapter-health, and
  routing schemas. Every live operation is covered by contract invariants.
- Project-neutral offline acceptance coverage and a real local DALiuGE 6.6
  no-download handoff qualification path.

### Changed

- Discovery and preparation now use `groups` and `records`, with canonical
  non-empty string identities for `source_identifier`, `group_key`, and
  `record_id`.
- Adapter readiness, doctor output, scheduler gating, and metrics are dynamic
  per project revision rather than fixed to CASDA and VizieR.
- Slurm submission artifacts, graphs, INI files, scripts, and receipt reads use
  the maintained `openssh-sftp-client` transport with atomic create/rename,
  containment, size, type, and mode checks. Shell `tee` uploads are gone.
- XML, VOTable, UWS, URL/form, known-host, and glob handling now use maintained
  standards libraries instead of local protocol parsers.
- Remote output roots, runtime environment, walltime, node/island resources,
  translation, credential slots, and publisher destination are pinned by the
  selected deployment profile.
- Scheduler and execution reconciliation now preserve submission uncertainty,
  recover exact remote jobs, fence lease owners, and keep terminal failure
  authoritative over late observations.
- Setup, installation, operator recipes, and documentation now use explicit
  installation homes, safely quoted commands, private atomic environment files,
  and provider-neutral guidance.
- OpenAPI generation is deterministic. The two published specifications are
  byte-identical and contain one canonical schema name per type.
- Core internals were streamlined for release: dead scheduler/client surfaces,
  legacy config conversion, obsolete CLI aliases, test-only production helpers,
  unused dependencies, and redundant wrapper layers were removed.

### Fixed

- False-green TAP health checks, missing required endpoint handling, and
  cross-project cache serialization.
- Async TAP jobs that previously remained pending because `PHASE=RUN` was not
  submitted, including relative UWS result locations and terminal cleanup.
- Numeric TAP identities that passed discovery but failed during execution.
- Historical migration checksum drift by restoring issued migrations and using
  forward-only compatibility migrations.
- Workers over-advertising unavailable Slurm/DALiuGE providers and jobs that
  could be leased without all required staging/deployment capabilities.
- Slurm upload stalls, ambiguous submission recovery, scheduler polling,
  cancellation races, output-path lifetimes, and run/session isolation.
- Output completion races so durable inventory and backend completion may arrive
  in either order while backend failure still wins.
- Setup path resolution, rerun state, secret-file permissions, host-key consent,
  shell quoting, headless behavior, and truthful start/reporting output.
- Metrics that hid legitimate project modules whose names resembled integration
  test fixtures.

### Security

- Removed Core superuser credentials and scoped callback tokens from DALiuGE
  graphs. Remote publication is callback-free and Core initiates the trusted
  SFTP read.
- Enforced strict known-host verification, explicit credential slots, owner-only
  secret files, path containment, symlink/type rejection, bounded reads, and
  redacted transport diagnostics.
- Added explicit SSH host-key consent before keyscan/import and fail-closed
  handling for unsafe or ambiguous credential state.
- Kept real backends disabled until their exact project/profile capabilities and
  credentials pass validation and doctor checks.

### Removed

- Project v1 compatibility, survey-specific SBID/dataset API aliases, fixed
  CASDA/VizieR health fields, and `source_id_transform`.
- Publisher credential issuance/verification callbacks and the callback runtime
  contract.
- Pre-lease job mutation APIs, duplicate scheduler polling/cancellation clients,
  unused graph/manifest abstractions, no-op profile fields, and test fakes
  exported from production crates.
- Production filtering for integration-test project-name prefixes.
- Thirty obsolete or duplicate tests across admission, staging, scheduler,
  graph injection, recurring jobs, and DIM classification. Release-critical
  lease, security, migration, output-verification, SFTP, OpenAPI, and the opt-in
  live DALiuGE coverage remain.

[Unreleased]: https://github.com/jbwod/beampipe-core-v2/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/jbwod/beampipe-core-v2/compare/v0.1.6...v0.2.0

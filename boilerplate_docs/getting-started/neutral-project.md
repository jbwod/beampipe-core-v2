# Project-neutral acceptance

Use this path to prove the Core contract without a live archive, provider
credentials, SSH, Slurm, or DALiuGE service. It exercises the checked-in
`minimal_survey` fixture against PostgreSQL with mock catalog and DALiuGE
adapters.

The fixture deliberately contains none of the provider or survey names used by
the first-party WALLABY sample. Its endpoint is under `.invalid`, so an
accidental live request cannot reach a real service.

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Offline neutral acceptance flows from a mock catalog through PostgreSQL discovery, admission, graph preparation, mock DALiuGE and generic output verification">
  <div class="bp-flow-node" data-tone="cyan"><span>01 / DISCOVER</span><strong>mock catalog</strong><small>arbitrary adapter name</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>02 / PERSIST</span><strong>PostgreSQL</strong><small>group + records</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>03 / PREPARE</span><strong>admission + graph</strong><small>pinned artifacts</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>04 / EXECUTE</span><strong>mock DALiuGE</strong><small>submit + poll</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>05 / VERIFY</span><strong>generic inventory</strong><small>terminal success</small></div>
</div>

## Inspect and validate the fixture

```bash
beampipe project validate -f config/examples/minimal_survey.v2.yaml
beampipe project explain -f config/examples/minimal_survey.v2.yaml
```

The fixture demonstrates:

- a project-owned adapter named `catalog` with `sync_post` transport;
- `staging.provider: none`;
- primary-row mapping to `group_key` and `record_id`;
- the neutral `groups` and `records` manifest shape;
- a local graph pinned by SHA-256; and
- required `beampipe-output-inventory/v1` verification.

## Run the database-backed acceptance test

Point `DATABASE_URL` at a fresh, disposable PostgreSQL database. The test runs
migrations itself and generates a unique project ID, but a dedicated database
makes the acceptance boundary and cleanup unambiguous.

```bash
export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/beampipe_neutral_test
cargo test -p beampipe-jobs \
  tests::neutral_project_runs_offline_end_to_end \
  -- --exact --nocapture
```

When `DATABASE_URL` is absent, the test skips outside CI and fails in CI. It
does not perform network, SSH, staging, or remote scheduler operations.

The test proves all of these transitions in one run:

1. Load and validate `minimal_survey.v2.yaml`.
2. Discover one mock catalog row and persist `group-1` with `record-1`.
3. Pass source readiness and prepare the neutral manifest and pinned graph.
4. Create an execution selecting `groups: ["group-1"]`.
5. Submit and poll the mock DALiuGE backend, retaining the four graph artifacts.
6. Hold at required output verification after backend completion.
7. Accept a generic v1 inventory and reach terminal success.
8. Finalize the source discovery signature so unchanged input is not queued again.

## API migration check

Clients built against an older project-shaped API must make the same clean
break as project YAML:

| Old project-specific concept | Core v2 contract |
|---|---|
| execution source filter | `sources[].groups` |
| metadata grouping field | `group_key` |
| prepared item identity | `record_id` |
| stored group items | `metadata_json.records` |
| default manifest collections | `groups[].records[]` |

There are no compatibility aliases. See the [API workflow](../api/index.md)
for request examples and [Project YAML](../project-configs/index.md) for
manifest-only field renaming.

## Move from neutral acceptance to a real project

Replace the `.invalid` endpoint and example query with project-owned values,
provide a real graph digest, and install a typed deployment profile. Keep real
backends disabled until `beampipe doctor --profile PROFILE_NAME` passes.

To evaluate the provider-specific first-party sample instead, opt in
explicitly:

```bash
beampipe setup --yes --runtime docker --sample wallaby-hires
```

That command installs WALLABY policy and provider requirements; normal setup
does not.

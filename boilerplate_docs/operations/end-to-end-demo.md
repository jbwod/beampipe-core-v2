# Qualification run

Use this run to prove one release across discovery, admission, graph preparation, and a real DALiuGE backend. It is a control-plane qualification, not yet scientific-output certification: Beampipe models output verification but does not currently execute an output-verification job.

## Current evidence

The latest local qualification on 2026-08-11 proved:

- live project-defined CASDA and VizieR TAP discovery for `HIPASSJ1313-15`;
- three visibility datasets under SBID `72962` at observation time;
- RA, DEC, VSys enrichment and a stable discovery signature;
- automatic source admission and durable job claims;
- manifest and graph artifact persistence;
- DALiuGE TM translation and REST/DIM deployment to a local cluster.

It also exposed two runtime issues:

1. Manifest templates initially read nested discovery flags while persisted datasets carried the values flat. This is fixed by commit `839e5f1`.
2. The deployed WALLABY application returned a list where the supplied graph expected raw pickled bytes, causing the scatter node to fail. A config-driven `output_parser: pickle` compatibility patch was prepared, but the cluster became unavailable before a successful rerun.

CASDA staging and Slurm submission remain unqualified. Do not claim a complete production run until the remaining gates below pass.

## Qualification levels

| Level | Boundary | Required result |
|---|---|---|
| Q0 | Local control plane | setup, migrations, auth, jobs, console, metrics |
| Q1 | Live discovery | TAP rows, enrichment, flags, stable signature, ready source |
| Q2 | Preparation | manifest and source/patched graph artifacts with hashes |
| Q3 | Real execution | TM plus REST/DIM or Slurm reaches a consistent terminal state |
| Q4 | Scientific result | independently defined products are present and valid |

The code can currently establish Q0-Q3. Q4 needs an external/manual product assertion until a typed output verifier exists.

## Before touching a backend

```bash
cargo fmt --all -- --check
cargo test --workspace
beampipe project validate -f config/wallaby_hires.v2.yaml
beampipe security check
beampipe doctor --json
```

Use a dedicated project ID, database, profile, and approved source. Record the Git commit, project hash, profile revision, graph SHA-256, and runtime package versions.

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="Qualification gates from source discovery through graph preparation backend execution and manual output validation">
  <div class="bp-flow-node" data-tone="cyan"><span>Q1</span><strong>discover</strong><small>real TAP</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>Q2</span><strong>prepare</strong><small>hashed artifacts</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>Q3</span><strong>execute</strong><small>REST or Slurm</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>Q4</span><strong>validate</strong><small>manual output contract</small></div>
</div>

## 1. Isolate inputs

Copy the reference project outside the repository and make these explicit qualification changes:

- set `metadata.id` to a unique project ID;
- point `graph.url` at an immutable URL or use a worker-visible local `graph.path`;
- use the no-download test graph when CASDA staging is outside the test boundary;
- set `automation.execution.archive_name` to a non-CASDA name for the no-download run;
- set `automation.execution.deployment_profile_name` to the qualification profile;
- keep source and run limits at `1`.

For the current local test graph, use:

```text
https://raw.githubusercontent.com/jbwod/wallaby-hires-beampipe/main/dlg-graphs/wallaby-hires_test-pipeline-nodownloads-beampipe.graph
```

Record its expected SHA-256 separately. A mutable branch URL is acceptable only for a rehearsal when the fetched bytes are hashed and retained; release qualification should pin content.

If runtime inspection shows `wallaby_hires.process_CSV_str` returns a Python list, add this project-config patch:

```yaml
graph_patches:
  - match:
      kind: node_name
      equals: process_CSV_str
    set:
      output_parser: pickle
```

Do not apply it blindly. Pin the DALiuGE and WALLABY package versions and document why the graph contract requires it.

## 2. Install a backend profile

=== "Existing local DIM"

    ```bash
    beampipe setup \
      --deployment rest_remote \
      --profile-name local-daliuge-e2e \
      --tm-url http://TRANSLATOR_HOST \
      --dim-url http://DIM_HOST:PORT
    beampipe doctor --profile local-daliuge-e2e
    beampipe daliuge inspect --profile local-daliuge-e2e
    ```

=== "Slurm"

    ```bash
    beampipe setup \
      --deployment slurm_remote \
      --profile-name setonix-e2e \
      --facility setonix \
      --ssh-host setonix.pawsey.org.au \
      --ssh-user "$USER" \
      --slurm-account PROJECT \
      --slurm-partition work \
      --remote-home /scratch/PROJECT \
      --dlg-root /scratch/PROJECT/$USER/dlg \
      --remote-logs /scratch/PROJECT/$USER/dlg/log

    beampipe security check
    beampipe doctor --profile setonix-e2e
    beampipe slurm ping --profile setonix-e2e
    beampipe profile render setonix-e2e
    ```

For Slurm, verify the graph application packages and DALiuGE CLI inside the same modules, virtual environment, or container that the batch job will use. A successful SSH probe does not prove runtime compatibility.

## 3. Start controlled roles

Use exactly one scheduler and low admission limits:

```bash
export BEAMPIPE_USE_REAL_BACKENDS=true
export BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS=1
export BEAMPIPE_WORKER_CONCURRENCY=1

beampipe serve --worker false
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true beampipe serve --worker true
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false beampipe worker
```

Give each host process a unique `BEAMPIPE_BIND_ADDR` and `BEAMPIPE_METRICS_BIND_ADDR`, or run the roles in separate containers.

## 4. Discover one source

Upload the isolated project with `beampipe project add -f PROJECT.yaml`, then authenticate and register the source:

```bash
BASE=http://127.0.0.1:8080
TOKEN=$(curl -fsS -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"YOUR_PASSWORD"}' | jq -er .access_token)
AUTH="Authorization: Bearer $TOKEN"

SOURCE=$(curl -fsS -X POST "$BASE/api/v2/sources" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"project_module":"PROJECT_ID","source_identifier":"HIPASSJ1313-15","enabled":true}')
SOURCE_ID=$(jq -r .uuid <<<"$SOURCE")

curl -fsS -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"project_module":"PROJECT_ID","source_identifier":"HIPASSJ1313-15"}' | jq .
```

Wait for readiness, then capture source status, metadata, and events. Stop if discovery is incomplete, a claim remains unexpectedly active, or the signature changes across identical consecutive discovery runs.

## 5. Preview before admission

```bash
beampipe graph prepare --project PROJECT_ID --source HIPASSJ1313-15
beampipe profile validate PROFILE
beampipe profile render PROFILE
```

Confirm graph-patch targets, manifest values, artifact hashes, TM reachability, profile caps, resource directives, and runtime package contracts. This is the go/no-go point.

## 6. Observe execution

Let project automation admit the workflow-pending source. Do not also create a manual execution for the same source.

```bash
curl -fsS "$BASE/api/v2/executions?project_module=PROJECT_ID" -H "$AUTH" | jq .
beampipe status
beampipe console
```

Follow the execution until a consistent terminal state:

```bash
beampipe timeline execution "$EXEC_ID" --table
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/observations" -H "$AUTH" | jq .
```

For REST, inspect the persisted DIM session and graph-status URLs. For Slurm, confirm the scheduler ID in both `squeue`/`sacct` and the execution ledger. Treat HTTP failures, SSH disconnects after submission, and unknown scheduler states as reconciliation events, not permission to resubmit.

## 7. Capture evidence

Retain a redacted bundle containing:

- Beampipe commit/version and UTC run window;
- project revision and SHA-256;
- profile ID, revision, and redacted snapshot;
- source status, metadata summary, discovery signature, and events;
- execution status, ledger snapshot, observations, and timeline;
- manifest, source graph, patched graph, and physical graph hashes;
- TM/DIM versions or Slurm job ID and runtime environment versions;
- Prometheus target health and relevant metric window;
- manual expected-output checks for Q4.

Exit criteria for Q3 are a consistent terminal control state, no active claims, durable external identifiers, and no unresolved submission uncertainty. Q4 additionally requires a separately approved output contract; until that is automated, label it manual evidence.

## Remaining release gates

- Complete a terminal local REST rerun with pinned graph/runtime packages.
- Exercise CASDA authenticated staging, including partial-SBID failure behavior.
- Exercise Slurm submission, uncertain-response reconciliation, batched polling, cancellation, and remote cleanup.
- Add an automated typed output verifier before presenting scientific completion as a native Beampipe result.
- Package or explicitly integrate the operator Grafana dashboard and retain its run window.

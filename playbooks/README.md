# Beampipe demonstration playbooks

`rest_remote_no_downloads.ipynb` is an operator-led, binary-driven demonstration of:

1. PostgreSQL migration and project/profile installation;
2. API and worker startup from the compiled `beampipe` binary;
3. bulk registration of multiple independent sources;
4. scheduler-backed CASDA/VizieR discovery;
5. deterministic manifest and no-download graph preparation;
6. one-source-per-execution admission;
7. real TM translation, REST DIM deployment, polling, and ledger evidence.

## Start

Build the binary and make PostgreSQL available from the repository root:

```bash
cargo build --locked --release -p beampipe-cli --bin beampipe
docker compose up -d postgres
```

Docker is optional when `DATABASE_URL` already points at a running PostgreSQL server. The notebook detects a missing Docker CLI and proceeds to its migration check.

Launch Jupyter from the repository root so the notebook can find the binary and config files:

```bash
jupyter lab playbooks/rest_remote_no_downloads.ipynb
```

The notebook uses only Python's standard library. Any Jupyter Python kernel is sufficient.

## Configure before running

Edit the first configuration cell:

- set a fresh `RUN_TAG`, or export `BEAMPIPE_PLAYBOOK_RUN_TAG` before launching Jupyter;
- set `TM_URL`, `DIM_DEPLOY_HOST`, and `DIM_HOST_FOR_TM` for your local DALiuGE services;
- override `BEAMPIPE_CASDA_TAP_URL` or `BEAMPIPE_VIZIER_TAP_URL` when using an institutional TAP proxy or mirror;
- use `LIVE_SUBMIT = False` for a mock-backend rehearsal;
- change to `LIVE_SUBMIT = True` only after `beampipe profile test` passes;
- replace `SOURCES` when demonstrating a different set of known archive sources.

The tracked profile is a readable starting point. The notebook writes a runtime copy under `playbook-runs/<RUN_TAG>/`, so endpoint edits do not modify the repository example.

## Safety and repeatability

- The project uses an immutable graph URL and records its expected SHA-256.
- `archive_name: no_downloads` prevents CASDA staging in the execution path.
- `max_sources_per_execution: 1` creates a separate run for each source.
- Source-level TAP discovery is serialized in the demo to avoid overloading public VizieR; production concurrency is a separate shaping decision.
- Transient TAP overloads use the retry policy in project YAML. The notebook performs up to three explicit rediscovery attempts before stopping with source blockers.
- The scheduler role starts only after discovery, profile checks, and graph previews pass.
- Global `beampipe doctor` is not a run gate because stale heartbeats from unrelated historical workers can make it fail. Use `beampipe worker list` separately when cleaning a shared database.
- Existing profiles are not overwritten. Use a new profile name when changing endpoints.
- The final cell stops only processes started by the notebook; PostgreSQL is left running.

This proves control-plane and DALiuGE runtime behavior. It does not verify scientific products, and current archive contents may change between demonstrations.

If TAP health is green but a source remains blocked on `ra_dec_vsys_complete`, inspect `playbook-runs/<RUN_TAG>/discovery-worker.log`. Reachability does not guarantee public query capacity. Let the notebook retry, then use an approved mirror/proxy or rerun later; keep the readiness flag strict for live DALiuGE work.

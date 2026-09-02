# WALLABY local DALiuGE qualification

This qualification runs the current WALLABY no-download graph directly through
real DALiuGE REST services. It is a component and graph smoke test, not a
Core-managed execution: the runner starts DALiuGE itself, supplies the publisher
handoff path, and verifies the resulting inventory without contacting Core.

Use it to check a WALLABY checkout before a facility run. It requires no CASDA,
Setonix, Core, SSH, S3, NGAS, or other external service.

## Proven topology

The runner creates one disposable workspace and starts actual DALiuGE 6.6 Node
Manager, Data Island Manager, and Translator Manager processes on separate,
dynamically selected loopback ports:

| Caller | Target | Purpose |
|---|---|---|
| smoke runner | TM `/unroll` | translate the patched logical graph |
| smoke runner | DIM session REST API | create, append, deploy, poll, and destroy the session |
| DIM | NM event, RPC, and REST ports | schedule and execute the physical graph |
| native publisher | temporary `file://` destination | publish and re-read the synthetic products |
| native publisher | attempt-scoped handoff file | atomically return the canonical inventory |

All listeners bind to `127.0.0.1`. This topology therefore does not exercise
the three caller-specific addresses in a Core `rest_remote` profile, container
DNS, Traefik, or any remote network path.

## Run the qualification

Use the same Python environment for the DALiuGE Engine and Translator, the
WALLABY package, and `beampipe-palette`. The current path is qualified with
DALiuGE 6.6 and `beampipe-palette` 0.5.1.

From the `wallaby-hires-beampipe` checkout:

```bash
python scripts/local_nodownload_rest_smoke.py
```

The command needs no credentials or service configuration. It:

1. loads `wallaby-hires_test-pipeline-nodownloads-beampipe.graph` and injects
   the repository's synthetic manifest fixture;
2. starts disposable NM, DIM, and TM processes on unique loopback ports;
3. calls the real TM `/unroll` route and the real DIM session REST API;
4. executes the native ingest and publisher applications;
5. requires one product for each configured pattern:
   `**/image*.10arc.final_mosaic.fits` and
   `**/weights*.10arc.final_mosaic.fits`;
6. verifies product bytes and SHA-256 digests after publication; and
7. requires the durable inventory, DALiuGE inventory FileDROP, and atomic
   handoff file to contain identical canonical bytes.

The recorded DALiuGE 6.6.0 qualification completed all 100 DROPs from five
triggered roots and verified byte-identical canonical inventory bytes across
the durable copy, FileDROP, and handoff.

Success prints a JSON summary containing the DROP and root counts, session ID,
two pattern counts, product paths, durable URI, handoff path, and inventory
SHA-256. The runner then destroys the session, stops all three services, and
removes the workspace. To retain the workspace and service logs for diagnosis:

```bash
python scripts/local_nodownload_rest_smoke.py --keep
```

## Evidence boundary

This smoke proves that the current no-download graph can translate, deploy,
execute its synthetic stubs, publish both expected synthetic products, and
produce the receipt handoff. It does not prove Core project/profile pinning,
admission, backend reconciliation, trusted receipt retrieval, or terminal
ledger state. It also does not qualify CASDA staging, ASKAPsoft, Setonix, or
science products.

The bundled Core no-download project deliberately has
`output_verification.required: true`, and its graph contains a mandatory native
publisher. For a Core-managed execution, trusted receipt retrieval is currently
implemented only for `slurm_remote`: Core owns the attempt-scoped handoff path
and pulls the inventory through its authenticated SSH/SFTP session after Slurm
completion. A `rest_remote` execution that requires publication has no trusted
handoff retrieval contract and fails closed; a finished DIM session cannot be
promoted to Core success.

Do not weaken the no-download project to `output_verification.required: false`
to make a REST run terminal. Use this direct runner for local DALiuGE graph
qualification only; qualify facility execution separately under the target
site's approval and operations controls.

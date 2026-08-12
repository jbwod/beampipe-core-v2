# REST demonstration playbook

Use the repository notebook to present multiple WALLABY sources from registration through independent terminal DALiuGE executions. It runs the compiled `beampipe` binary, calls `/api/v2` for workflow intent, and retains a redacted evidence bundle.

## Demonstration boundary

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="No-download REST demonstration from project configuration through source discovery and separate DALiuGE executions">
  <div class="bp-flow-node" data-tone="cyan"><span>CONFIG</span><strong>project + profile</strong><small>validated revisions</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>DISCOVER</span><strong>multiple sources</strong><small>CASDA + VizieR TAP</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>PREVIEW</span><strong>manifest + graph</strong><small>no submission</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>ADMIT</span><strong>one run per source</strong><small>durable jobs</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>REST</span><strong>TM + DIM</strong><small>deploy + poll</small></div>
</div>

The project uses the no-download graph and `archive_name: no_downloads`. Discovery still queries live public TAP services, but execution does not request CASDA staging. This proves control-plane and DALiuGE runtime behavior, not scientific products.

The notebook serializes source-level TAP requests for a reliable public-service demonstration. Do not treat its discovery concurrency as a production throughput recommendation.

## Launch

From the repository root:

```bash
cargo build --locked --release -p beampipe-cli --bin beampipe
docker compose up -d postgres
jupyter lab playbooks/rest_remote_no_downloads.ipynb
```

Docker is optional when `DATABASE_URL` points at an existing PostgreSQL server. The notebook uses only the Python standard library. Open `playbooks/README.md` when Jupyter is unavailable; its commands and configuration files can be followed from any notebook-capable editor.

## Configure the run

In the first notebook cell:

1. set a fresh `RUN_TAG`, or export `BEAMPIPE_PLAYBOOK_RUN_TAG` before launching Jupyter;
2. review the source list and keep at least two distinct identifiers;
3. set the TM URL and both DIM address perspectives;
4. leave `LIVE_SUBMIT=False` for a mock rehearsal;
5. set `LIVE_SUBMIT=True` only after the live profile test passes.

The public TAP defaults may be replaced with `BEAMPIPE_CASDA_TAP_URL` and `BEAMPIPE_VIZIER_TAP_URL`. Project YAML controls request timeout and retry count; the notebook also retries a blocked source discovery up to three times before stopping.

The worker-facing DIM host can differ from the host visible to Translator Manager. Treat that as an expected topology choice, not a duplicated setting.

## Operator pause

The notebook starts an API and discovery worker first, leaving execution scheduling off. It then waits for every discovery claim, summarizes metadata, and previews each source-specific graph. Only the next explicit cell starts the scheduler and permits one-source-per-run admission.

This ordering gives the presenter a clean go/no-go point:

- project and profile revisions validate;
- the graph input matches its expected SHA-256;
- at least two sources are ready;
- manifests and patched graphs have durable hashes;
- the live deployment profile test has passed when `LIVE_SUBMIT=True`.

## Configuration walkthroughs

The notebook ends with two presenter-ready appendices:

- **Deployment profile, start to finish:** address perspectives, translation, TLS, concurrency, installation, validation, rendering, live testing, and revision pinning.
- **Project YAML, start to finish:** identity, transforms, TAP queries, enrichments, metadata, signatures, manifests, graph patches, automation, validation, activation, and qualification.

The reviewed inputs are:

```text
playbooks/config/rest_remote.local.json
playbooks/config/wallaby_hires_no_downloads_rest.v2.yaml
```

For deeper field reference, continue with [Deployment profiles and SSH](../architecture/deployment-profiles.md) and [Project YAML](../project-configs/index.md).

## Common stops

| Stop | Meaning | Action |
|---|---|---|
| Profile already exists | `RUN_TAG` was reused against the same database | Choose a fresh tag; profiles and projects are intentionally immutable |
| `ra_dec_vsys_complete` blocks a source | VizieR returned no enrichment row or rejected work while saturated | Let the notebook retry, then use an approved TAP mirror/proxy or rerun later; do not bypass the flag for a live run |
| TAP health is `ok`, but discovery fails | Health proves reachability, not query capacity | Inspect `discovery-worker.log` for the bounded external error |
| Profile test cannot reach DIM | The TM-visible and worker-visible DIM addresses are wrong for the current network | Correct both address perspectives in the first cell and use a fresh tag |
| Fewer executions than sources | Admission limits, pending state, or readiness blocked a source | Inspect the per-source status table before starting the scheduler |

Every stop occurs before or alongside durable evidence. Do not delete the run directory until the presenter has captured the relevant logs and status output.

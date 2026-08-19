# First workflow

This walkthrough uses one known WALLABY source to prove live public CASDA/VizieR discovery and deterministic graph preparation. It does not request CASDA staging or submit work to DALiuGE.

Complete the [quick start](index.md), leave `BEAMPIPE_USE_REAL_BACKENDS=false`, and keep `beampipe start` running.

## 1. Authenticate

The password is the value setup printed once (`Generated admin password...`). It is not stored in `.env`.

```bash
export BASE=http://127.0.0.1:18080
export ADMIN_USER="${ADMIN_USER:-admin}"
export ADMIN_PASSWORD="${ADMIN_PASSWORD:?set to the password setup printed}"
export TOKEN=$(curl -fsS -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"${ADMIN_USER}\",\"password\":\"${ADMIN_PASSWORD}\"}" \
  | jq -er .access_token)
export AUTH="Authorization: Bearer $TOKEN"
```

## 2. Register and discover

```bash
SOURCE=$(curl -fsS -X POST "$BASE/api/v2/sources" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{
    "project_module":"wallaby_hires",
    "source_identifier":"HIPASSJ1313-15",
    "enabled":true
  }')
SOURCE_ID=$(jq -r .uuid <<<"$SOURCE")

curl -fsS -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires","source_identifier":"HIPASSJ1313-15"}' \
  | jq .
```

The endpoint marks the source stale. The scheduler claims it and a worker executes the project-defined TAP queries. Poll until `ready` is true and the discovery claim is empty:

```bash
watch -n 2 "curl -fsS '$BASE/api/v2/sources/$SOURCE_ID/status' -H '$AUTH' | jq ."
```

Expected for the reference source at the time of qualification: visibility datasets from SBID `72962`, populated RA/DEC/VSys values, and `ra_dec_vsys_complete=true`. Archive results can change; readiness and a stable signature are the contract, not a permanent row count.

## 3. Inspect preparation

Ask the API whether the source can form an execution:

```bash
curl -fsS -X POST "$BASE/api/v2/executions/prepare" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{
    "project_module":"wallaby_hires",
    "sources":[{"source_identifier":"HIPASSJ1313-15"}],
    "archive_name":"casda",
    "deployment_profile_name":"slurm-remote"
  }' | jq .
```

Build the manifest and patched graph without external submission:

```bash
beampipe graph prepare \
  --project wallaby_hires \
  --source HIPASSJ1313-15
```

Confirm the output includes the active project revision, manifest checksum, source graph checksum, patched graph checksum, and graph-patch summary.

## 4. Inspect automatic execution

The reference project has execution automation enabled. With mock backends, the scheduler may admit the ready source automatically. Find and inspect the resulting execution:

```bash
curl -fsS "$BASE/api/v2/executions?project_module=wallaby_hires" \
  -H "$AUTH" | jq '.items[0] | {uuid,status,control_phase,submission_state}'
```

Set `EXEC_ID` to that UUID, then inspect durable evidence:

```bash
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/ledger-snapshot" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/artifacts" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/events" -H "$AUTH" | jq .
```

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="First workflow from source registration through discovery and graph preparation to mock execution evidence">
  <div class="bp-flow-node" data-tone="cyan"><span>01</span><strong>source</strong><small>stable identity</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>02</span><strong>TAP</strong><small>query + enrich</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>03</span><strong>metadata</strong><small>normalize + sign</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>04</span><strong>artifacts</strong><small>manifest + graph</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>05</span><strong>ledger</strong><small>mock outcome</small></div>
</div>

## Next boundary

Interactive setup already offers these as Next actions. To submit real work later, install a validated `rest_remote` or `slurm_remote` profile, set CASDA credentials for staging, run `beampipe doctor --profile NAME`, then set `BEAMPIPE_USE_REAL_BACKENDS=true` and `beampipe restart`. Follow [Deployment profiles and SSH](../architecture/deployment-profiles.md); do not reuse a mock profile for live submission.

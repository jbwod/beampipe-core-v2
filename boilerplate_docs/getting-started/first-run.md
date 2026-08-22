# First workflow

This walkthrough uses one known WALLABY source to prove live public CASDA/VizieR discovery and deterministic graph preparation. It does not request CASDA staging or submit work to DALiuGE.

Complete the [quick start](index.md) and leave
`BEAMPIPE_USE_REAL_BACKENDS=false`. A Docker `beampipe start` returns after the
services start. Native host mode runs in the foreground, so keep it in a
dedicated terminal and use a second terminal for this walkthrough.

## 1. Authenticate

The password is the value setup printed once (`Generated admin password...`). It is not stored in `.env`.

```bash
export BASE=http://127.0.0.1:18080
export ADMIN_USER="${ADMIN_USER:-admin}"
export ADMIN_PASSWORD="${ADMIN_PASSWORD:?set to the password setup printed}"
LOGIN_BODY=$(jq -n \
  --arg username "$ADMIN_USER" \
  --arg password "$ADMIN_PASSWORD" \
  '{username:$username,password:$password}')
export TOKEN=$(curl -fsS -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d "$LOGIN_BODY" \
  | jq -er .access_token)
export AUTH="Authorization: Bearer $TOKEN"
```

## 2. Safely select mock admission policy

The reference project's automation names `slurm-remote`. First prove the active
installation resolves mock mode. Then create the bundled profile only when that
name is absent; `profile add` is an upsert and must not replace an existing
operator profile.

```bash
export BEAMPIPE_HOME="${BEAMPIPE_HOME:-$HOME/beampipe}"
beampipe config explain | jq -e '
  .settings[]
  | select(.key == "use_real_backends" and .value == "false")
' >/dev/null

PROFILE_NAME=slurm-remote
EXISTING_PROFILE=$(beampipe profile list | \
  jq -c --arg name "$PROFILE_NAME" \
    'first(.[] | select(.name == $name)) // empty')
if [ -n "$EXISTING_PROFILE" ]; then
  jq -e '
    .project_module == null or .project_module == "wallaby_hires"
  ' <<<"$EXISTING_PROFILE" >/dev/null || {
    echo 'Existing slurm-remote belongs to another project; stop.' >&2
    exit 1
  }
else
  beampipe profile add \
    -f "$BEAMPIPE_HOME/config/deployment_profile.slurm-remote.json"
fi
beampipe profile validate "$PROFILE_NAME"
```

The existing profile is left unchanged. In mock mode it supplies typed policy
but cannot submit to Slurm. If you changed `.env` to reach this state, restart
the running services before continuing. Edit and qualify account, paths,
credential slot, and runtime inputs before ever enabling real backends.

## 3. Register and discover

```bash
SOURCE=$(curl -fsS -X POST "$BASE/api/v2/sources" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{
    "project_module":"wallaby_hires",
    "source_identifier":"HIPASSJ1318-21",
    "enabled":true
  }')
SOURCE_ID=$(jq -r .uuid <<<"$SOURCE")

curl -fsS -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires","source_identifier":"HIPASSJ1318-21"}' \
  | jq .
```

The endpoint marks the source stale. The scheduler claims it and a worker
executes the project-defined TAP queries. Poll for completed discovery and a
non-empty signature. Automatic mock admission may consume that signature before
you observe `ready_for_execution=true`; that is one valid branch, not a failed
discovery.

```bash
for attempt in $(seq 1 120); do
  SOURCE_STATUS=$(curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH")
  jq '{ready_for_execution,discovery_complete,discovery_signature,blockers}' \
    <<<"$SOURCE_STATUS"
  jq -e '.discovery_complete == true and
         ((.discovery_signature // "") | length > 0)' \
    <<<"$SOURCE_STATUS" >/dev/null && break
  sleep 5
done
jq -e '.discovery_complete == true and
       ((.discovery_signature // "") | length > 0)' \
  <<<"$SOURCE_STATUS" >/dev/null
DISCOVERY_SIGNATURE=$(jq -er .discovery_signature <<<"$SOURCE_STATUS")
```

Expected for the reference source at the time of qualification: visibility datasets from SBID `72962`, populated RA/DEC/VSys values, and `ra_dec_vsys_complete=true`. Archive results can change; completed discovery and a non-empty signature are the contract, not a permanent row count.

## 4. Materialize and inspect admission

Build the graph locally, then ask whether the source can form an execution. No
command in this step submits work externally.

```bash
beampipe graph prepare \
  --project wallaby_hires \
  --source HIPASSJ1318-21

PREPARE_RESPONSE=$(curl -fsS -X POST "$BASE/api/v2/executions/prepare" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{
    "project_module":"wallaby_hires",
    "sources":[{"source_identifier":"HIPASSJ1318-21"}],
    "archive_name":"casda",
    "deployment_profile_name":"slurm-remote"
  }')
jq . <<<"$PREPARE_RESPONSE"
```

Confirm the output includes the active project revision, manifest checksum, source graph checksum, patched graph checksum, and graph-patch summary.

Preparation normally reports `valid=true`. If it does not, accept the result
only when automatic admission already created an execution for the exact
discovery signature; otherwise stop and resolve the returned errors:

```bash
if ! jq -e '.valid == true' <<<"$PREPARE_RESPONSE" >/dev/null; then
  EXEC_ID=$(curl -fsS \
    "$BASE/api/v2/executions?project_module=wallaby_hires&items_per_page=100" \
    -H "$AUTH" | jq -r --arg signature "$DISCOVERY_SIGNATURE" '
      [.items[] | select(.discovery_signature == $signature)][0].uuid // empty
    ')
  test -n "$EXEC_ID" || {
    echo 'Preparation failed without a matching automatic execution.' >&2
    exit 1
  }
fi
```

## 5. Inspect automatic execution

The reference project has execution automation enabled. With mock backends, the
scheduler admits the discovered signature automatically. If the branch above
did not already find it, poll for that exact execution instead of selecting an
older run:

```bash
for attempt in $(seq 1 60); do
  EXECUTION_LIST=$(curl -fsS \
    "$BASE/api/v2/executions?project_module=wallaby_hires&items_per_page=100" \
    -H "$AUTH")
  EXEC_ID=$(jq -r --arg signature "$DISCOVERY_SIGNATURE" '
    [.items[] | select(.discovery_signature == $signature)][0].uuid // empty
  ' <<<"$EXECUTION_LIST")
  [ -n "$EXEC_ID" ] && break
  sleep 2
done
test -n "$EXEC_ID"
jq --arg id "$EXEC_ID" \
  '.items[] | select(.uuid == $id) | {uuid,status,control_phase,submission_state}' \
  <<<"$EXECUTION_LIST"
```

Inspect durable evidence:

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

For a reproducible live submission without CASDA downloads, continue with
[Local DALiuGE end to end](local-daliuge.md). It uses the explicit no-download
project, exercises creation and start idempotency, and proves terminal DIM and
artifact evidence.

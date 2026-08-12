# REST no-downloads demonstration

This playbook drives the compiled `beampipe` binary and `/api/v2` with shell commands. It discovers three independent WALLABY sources, previews each manifest and graph, then admits one `rest_remote` execution per source.

```mermaid
flowchart LR
    C["Project + profile"] --> S["Register sources"]
    S --> D["CASDA + VizieR discovery"]
    D --> P["Manifest + graph preview"]
    P --> A["One run per source"]
    A --> R["TM + REST DIM"]
    R --> E["Terminal evidence"]
```

`LIVE_SUBMIT=false` uses Beampipe's mock backend after live TAP discovery. Set it to `true` only when Translator Manager and DIM are running. The project uses `archive_name: no_downloads`, so execution does not request CASDA staging.

CASDA credentials are not required for this playbook. The discovery TAP queries are public, and the no-download execution path skips authenticated staging.

## 1. Build and choose the run

Use a Bash terminal at the repository root:

```bash
set -euo pipefail

export BEAMPIPE="$PWD/target/release/beampipe"
cargo build --locked --release -p beampipe-cli --bin beampipe

export RUN_TAG="demo-$(date +%m%d-%H%M)"
export RUN_DIR="$PWD/playbook-runs/$RUN_TAG"
export PROJECT_ID="wallaby_hires_nodownloads_$RUN_TAG"
export PROFILE_NAME="rest-demo-$RUN_TAG"
export API_URL="http://127.0.0.1:18080"
export LIVE_SUBMIT=false

mkdir -p "$RUN_DIR/graphs" "$RUN_DIR/evidence"
```

Choose a fresh `RUN_TAG` for every demonstration. Project and profile revisions are intentionally immutable.

## 2. Configure the deployment profile

These are two different address perspectives:

```mermaid
flowchart LR
    B["Beampipe worker"] -->|"translation.tm_url"| T["Translator Manager"]
    T -->|"dim_host_for_tm"| D["DIM"]
    B -->|"deploy_host"| D
```

Set the addresses for the local DALiuGE installation:

```bash
export TM_URL="http://dlg-tm.desk"
export DIM_HOST_FOR_TM="dlg-dim.desk"
export DIM_PORT_FOR_TM=80
export DIM_DEPLOY_HOST="dlg-dim.desk"
export DIM_DEPLOY_PORT=80
```

Generate a run-specific profile from the tracked example:

```bash
jq \
  --arg name "$PROFILE_NAME" \
  --arg project "$PROJECT_ID" \
  --arg tm "$TM_URL" \
  --arg dim_tm "$DIM_HOST_FOR_TM" \
  --arg dim_worker "$DIM_DEPLOY_HOST" \
  --argjson dim_tm_port "$DIM_PORT_FOR_TM" \
  --argjson dim_worker_port "$DIM_DEPLOY_PORT" \
  '.name = $name
   | .project_module = $project
   | .translation.tm_url = $tm
   | .deployment.dim_host_for_tm = $dim_tm
   | .deployment.dim_port_for_tm = $dim_tm_port
   | .deployment.deploy_host = $dim_worker
   | .deployment.deploy_port = $dim_worker_port' \
  playbooks/config/rest_remote.local.json \
  > "$RUN_DIR/rest-profile.json"

jq . "$RUN_DIR/rest-profile.json"
```

Profile field checklist:

| Field | Purpose |
|---|---|
| `project_module` | Prevents another project selecting this profile |
| `translation.tm_url` | Translator Manager endpoint visible to Beampipe |
| `algo`, `num_par`, `num_islands` | DALiuGE partitioning policy |
| `dim_host_for_tm` | DIM address Translator Manager embeds in translated work |
| `deploy_host` | DIM address Beampipe uses for deploy, poll, and cancel |
| `use_https`, `verify_ssl` | Transport and certificate policy |
| `max_concurrent_executions` | Profile-level admission cap |

Profiles contain infrastructure policy, not credentials.

## 3. Configure the project YAML

Create a run-specific project document while leaving every TAP query in project configuration:

```bash
sed \
  -e "s/wallaby_hires_nodownloads_rest_demo/$PROJECT_ID/g" \
  -e "s/local-rest-no-downloads/$PROFILE_NAME/g" \
  playbooks/config/wallaby_hires_no_downloads_rest.v2.yaml \
  > "$RUN_DIR/project.v2.yaml"

sed -n '1,240p' "$RUN_DIR/project.v2.yaml"
```

Read it in data-flow order:

1. `source_identity` and `definitions.transforms` create safe template values.
2. `adapters.tap` controls timeout, retries, and fail policy.
3. `discovery.queries` contains CASDA and VizieR ADQL.
4. `discovery.enrichments` contains per-SBID evaluation-file ADQL.
5. `prepare_metadata` maps rows, calculates readiness flags, and defines signatures.
6. `manifest` groups source, SBID, and datasets.
7. `graph` pins the no-download logical graph.
8. `graph_patches` sets scatter copies and the CSV output contract.
9. `automation.discovery` shapes durable discovery jobs.
10. `automation.execution` selects the profile and enforces one source per run.

Validate both documents before touching the database:

```bash
"$BEAMPIPE" project validate -f "$RUN_DIR/project.v2.yaml"
"$BEAMPIPE" project explain -f "$RUN_DIR/project.v2.yaml" | tee "$RUN_DIR/project-explain.json"
```

## 4. Configure the process environment

The runtime file is local, ignored by Git, and mode `0600`. Bash `%q` escaping keeps URLs and generated secrets safe when the file is sourced:

```bash
export DATABASE_URL="${DATABASE_URL:-postgres://postgres:postgres@127.0.0.1:5432/beampipe}"
export BEAMPIPE_JWT_SECRET="${BEAMPIPE_JWT_SECRET:-$(openssl rand -hex 32)}"
export BEAMPIPE_CASDA_TAP_URL="${BEAMPIPE_CASDA_TAP_URL:-https://casda.csiro.au/casda_vo_tools/tap/sync}"
export BEAMPIPE_VIZIER_TAP_URL="${BEAMPIPE_VIZIER_TAP_URL:-https://tapvizier.cds.unistra.fr/TAPVizieR/tap/sync}"

{
  printf 'export BEAMPIPE=%q\n' "$BEAMPIPE"
  printf 'export RUN_TAG=%q\n' "$RUN_TAG"
  printf 'export RUN_DIR=%q\n' "$RUN_DIR"
  printf 'export PROJECT_ID=%q\n' "$PROJECT_ID"
  printf 'export PROFILE_NAME=%q\n' "$PROFILE_NAME"
  printf 'export API_URL=%q\n' "$API_URL"
  printf 'export LIVE_SUBMIT=%q\n' "$LIVE_SUBMIT"
  printf 'export DATABASE_URL=%q\n' "$DATABASE_URL"
  printf 'export BEAMPIPE_JWT_SECRET=%q\n' "$BEAMPIPE_JWT_SECRET"
  printf 'export BEAMPIPE_ENV=development\n'
  printf 'export BEAMPIPE_USE_REAL_BACKENDS=%q\n' "$LIVE_SUBMIT"
  printf 'export BEAMPIPE_CASDA_TAP_URL=%q\n' "$BEAMPIPE_CASDA_TAP_URL"
  printf 'export BEAMPIPE_VIZIER_TAP_URL=%q\n' "$BEAMPIPE_VIZIER_TAP_URL"
  printf 'export BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY=1\n'
  printf 'export BEAMPIPE_WORKER_CONCURRENCY=1\n'
  printf 'export BEAMPIPE_WORKER_POLL_INTERVAL_MS=200\n'
  printf 'export BEAMPIPE_SCHEDULER_INTERVAL_SECONDS=5\n'
  printf 'export BEAMPIPE_DIM_POLL_INTERVAL_SECONDS=3\n'
  printf 'export BEAMPIPE_RATE_LIMIT_REQUESTS=200\n'
} > "$RUN_DIR/runtime.env"
chmod 600 "$RUN_DIR/runtime.env"
```

For production, inject JWT and database credentials from mounted files or the service manager instead of writing them to a shell environment file.

## 5. Bootstrap and install immutable configuration

Start PostgreSQL if needed, migrate, and create a temporary local administrator:

```bash
docker compose up -d postgres       # Skip when DATABASE_URL is already reachable.
source "$RUN_DIR/runtime.env"
"$BEAMPIPE" migrate

read -r -s -p "Demo administrator password: " DEMO_PASSWORD
echo
"$BEAMPIPE" admin create-user \
  --username "demo-$RUN_TAG" \
  --password "$DEMO_PASSWORD" \
  --email "demo-$RUN_TAG@example.test" \
  --name "Demo Operator" \
  --superuser
```

Install, validate, and render the profile:

```bash
"$BEAMPIPE" profile add -f "$RUN_DIR/rest-profile.json"
"$BEAMPIPE" profile validate "$PROFILE_NAME"
"$BEAMPIPE" profile render "$PROFILE_NAME" | tee "$RUN_DIR/profile-render.json"
```

Install the project revision:

```bash
"$BEAMPIPE" project add -f "$RUN_DIR/project.v2.yaml"
"$BEAMPIPE" project render -f "$RUN_DIR/project.v2.yaml" | tee "$RUN_DIR/project-render.json"
```

For a real submission, require live TM and DIM checks to pass now:

```bash
if [ "$LIVE_SUBMIT" = true ]; then
  "$BEAMPIPE" profile test "$PROFILE_NAME"
else
  echo "Mock rehearsal: profile connectivity test skipped."
fi
```

Do not continue in live mode when `profile test` fails.

## 6. Start Beampipe in three terminals

### Terminal A: API

```bash
export RUN_TAG="demo-MMDD-HHMM"   # Use the exact tag from section 1.
source "playbook-runs/$RUN_TAG/runtime.env"
export BEAMPIPE_BIND_ADDR="127.0.0.1:18080"
export BEAMPIPE_METRICS_BIND_ADDR="127.0.0.1:19090"
"$BEAMPIPE" serve --worker false 2>&1 | tee "$RUN_DIR/api.log"
```

### Terminal B: discovery worker

Keep execution scheduling disabled until graph review:

```bash
export RUN_TAG="demo-MMDD-HHMM"   # Use the exact tag from section 1.
source "playbook-runs/$RUN_TAG/runtime.env"
export BEAMPIPE_WORKER_SCHEDULER_ENABLED=false
export BEAMPIPE_WORKER_INSTANCE_NAME="demo-discovery-$RUN_TAG"
"$BEAMPIPE" worker 2>&1 | tee "$RUN_DIR/discovery-worker.log"
```

### Terminal C: operator commands

```bash
export RUN_TAG="demo-MMDD-HHMM"   # Use the exact tag from section 1.
source "playbook-runs/$RUN_TAG/runtime.env"
curl -fsS "$API_URL/api/v2/health" | jq
curl -fsS "$API_URL/api/v2/ready" | jq

read -r -s -p "Demo administrator password: " DEMO_PASSWORD
echo
LOGIN_BODY=$(jq -n \
  --arg username "demo-$RUN_TAG" \
  --arg password "$DEMO_PASSWORD" \
  '{username: $username, password: $password}')
TOKEN=$(curl -fsS -X POST "$API_URL/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d "$LOGIN_BODY" \
  | jq -r .access_token)
test -n "$TOKEN" && test "$TOKEN" != null

auth=(-H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json')
curl -fsS "${auth[@]}" "$API_URL/api/v2/user/me" | jq
```

The token remains only in Terminal C.

## 7. Register three separate sources

```bash
curl -fsS -X POST "$API_URL/api/v2/sources/bulk" "${auth[@]}" \
  -d "{
    \"items\": [
      {\"project_module\": \"$PROJECT_ID\", \"source_identifier\": \"HIPASSJ1313-15\", \"enabled\": true},
      {\"project_module\": \"$PROJECT_ID\", \"source_identifier\": \"HIPASSJ1317-16\", \"enabled\": true},
      {\"project_module\": \"$PROJECT_ID\", \"source_identifier\": \"HIPASSJ1318-21\", \"enabled\": true}
    ]
  }" | tee "$RUN_DIR/sources.json" | jq
```

Trigger one durable discovery tick for the explicit set:

```bash
curl -fsS -X POST "$API_URL/api/v2/sources/discover" "${auth[@]}" \
  -d "{
    \"project_module\": \"$PROJECT_ID\",
    \"source_identifiers\": [\"HIPASSJ1313-15\", \"HIPASSJ1317-16\", \"HIPASSJ1318-21\"]
  }" | jq
```

## 8. Watch discovery

Run this loop and press `Ctrl-C` when all three show `discovery_complete: true`:

```bash
while true; do
  date -u
  for SOURCE in HIPASSJ1313-15 HIPASSJ1317-16 HIPASSJ1318-21; do
    SOURCE_ID=$(jq -r --arg source "$SOURCE" \
      '.items[] | select(.source_identifier == $source) | .uuid' \
      "$RUN_DIR/sources.json")
    curl -fsS "${auth[@]}" "$API_URL/api/v2/sources/$SOURCE_ID/status" \
      | jq -c --arg source "$SOURCE" \
        '{source: $source, discovery_complete, ready_for_execution, blockers}'
  done
  sleep 5
done
```

Inspect metadata and signatures:

```bash
for SOURCE in HIPASSJ1313-15 HIPASSJ1317-16 HIPASSJ1318-21; do
  SOURCE_ID=$(jq -r --arg source "$SOURCE" \
    '.items[] | select(.source_identifier == $source) | .uuid' \
    "$RUN_DIR/sources.json")
  curl -fsS "${auth[@]}" "$API_URL/api/v2/sources/$SOURCE_ID/metadata" \
    > "$RUN_DIR/evidence/metadata-$SOURCE.json"
  jq '{source: .source.source_identifier, metadata_count}' \
    "$RUN_DIR/evidence/metadata-$SOURCE.json"
done
```

All sources must show `ready_for_execution: true`. If `ra_dec_vsys_complete` blocks a source, inspect `discovery-worker.log`; public VizieR may be reachable but too busy to accept synchronous work. Rerun the discovery trigger from section 7 later or configure an approved TAP mirror. Do not bypass the flag for a live run.

## 9. Go/no-go: preview every graph

No external DALiuGE work is submitted here. Use the binary's graph-preparation command:

```bash
for SOURCE in HIPASSJ1313-15 HIPASSJ1317-16 HIPASSJ1318-21; do
  "$BEAMPIPE" graph prepare --project "$PROJECT_ID" --source "$SOURCE" \
    > "$RUN_DIR/graphs/$SOURCE.json"

  jq '{
    source_identifiers,
    manifest_sha256,
    source_graph_sha256,
    patched_graph_sha256,
    patch_summary
  }' "$RUN_DIR/graphs/$SOURCE.json"
done
```

Stop if any preview fails. Confirm:

- each manifest is source-specific;
- source and patched graph hashes differ;
- `Scatter/GenericScatterApp.num_of_copies` changed;
- `process_CSV_str.output_parser` changed;
- `LIVE_SUBMIT` still reflects the intended mode.

## 10. Enable execution scheduling

In Terminal B, press `Ctrl-C`, then restart the worker with scheduling enabled:

```bash
export RUN_TAG="demo-MMDD-HHMM"   # Use the exact tag from section 1.
source "playbook-runs/$RUN_TAG/runtime.env"
export BEAMPIPE_WORKER_SCHEDULER_ENABLED=true
export BEAMPIPE_WORKER_INSTANCE_NAME="demo-scheduler-$RUN_TAG"
"$BEAMPIPE" worker 2>&1 | tee "$RUN_DIR/scheduler-worker.log"
```

This bootstraps recurring execution admission and DIM polling. The project sets `max_sources_per_execution: 1`, so three ready sources produce three independent executions.

## 11. Watch executions reach terminal state

In Terminal C:

```bash
while true; do
  curl -fsS "${auth[@]}" \
    "$API_URL/api/v2/executions?project_module=$PROJECT_ID&items_per_page=100" \
    > "$RUN_DIR/executions.json"

  jq '[.items[] | {
    uuid,
    source: .sources[0].source_identifier,
    status,
    control_phase,
    daliuge_state,
    last_error
  }]' "$RUN_DIR/executions.json"

  RUNS=$(jq '.items | length' "$RUN_DIR/executions.json")
  TERMINAL=$(jq '[.items[] | select(.status == "completed" or .status == "failed" or .status == "cancelled")] | length' "$RUN_DIR/executions.json")
  [ "$RUNS" -ge 3 ] && [ "$TERMINAL" -eq "$RUNS" ] && break
  sleep 3
done
```

Verify one source per execution and capture redacted evidence:

```bash
jq -e \
  '(.items | length == 3) and ([.items[].sources | length] | all(. == 1))' \
  "$RUN_DIR/executions.json"

for ID in $(jq -r '.items[].uuid' "$RUN_DIR/executions.json"); do
  curl -fsS "${auth[@]}" "$API_URL/api/v2/executions/$ID/status" \
    > "$RUN_DIR/evidence/$ID-status.json"
  curl -fsS "${auth[@]}" "$API_URL/api/v2/executions/$ID/summary" \
    > "$RUN_DIR/evidence/$ID-summary.json"
  curl -fsS "${auth[@]}" "$API_URL/api/v2/executions/$ID/artifacts" \
    > "$RUN_DIR/evidence/$ID-artifacts.json"
  curl -fsS "${auth[@]}" \
    "$API_URL/api/v2/executions/$ID/ledger-snapshot?include_manifest=false" \
    > "$RUN_DIR/evidence/$ID-ledger.json"
done

jq -s '[.[] | {uuid, status, daliuge_session_id, terminal_outcome, last_error}]' \
  "$RUN_DIR"/evidence/*-status.json
```

The demonstration passes when all three runs are `completed`. In live mode, this proves Beampipe control-plane and DALiuGE runtime behavior; it does not verify scientific products.

## 12. Stop the demonstration

Press `Ctrl-C` in Terminals A and B. PostgreSQL is deliberately left running so evidence remains queryable:

```bash
"$BEAMPIPE" status
"$BEAMPIPE" worker list --include-stopped

# Optional after review:
docker compose stop postgres
```

Do not commit `runtime.env`; it contains local development secrets. The entire `playbook-runs/` directory is ignored by Git.

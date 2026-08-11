# End-to-end scientific demo

This is the exact operator procedure for the eventual L4 demonstration:
WALLABY source registration, live CASDA/VizieR discovery, CASDA staging,
manifest and graph preparation, DALiuGE translation, Setonix Slurm execution,
external-state reconciliation, and scientific output verification.

!!! danger "Not yet an L4-ready procedure"
    Do not present a run as a full scientific demonstration until every P0 item
    in [Demo readiness](demo-readiness.md) is closed. The current code can be
    rehearsed through L2/L3, but the final output-verification assertion cannot
    pass until `DEMO-002` is implemented. The graph and harness gates must also
    be closed before this becomes the release qualification procedure.

## Demo contract

The run uses one approved source and one project/profile pair:

| Item | Required value |
|---|---|
| Project | `wallaby_hires` active immutable revision |
| Deployment profile | `slurm-remote`, project `wallaby_hires`, kind `slurm_remote` |
| Source | Set in `DEMO_SOURCE`; confirmed in advance to have usable datasets and expected outputs |
| Database | New dedicated database named `beampipe_demo_*` |
| API | Bound to loopback or a controlled operator network |
| Scheduler | Exactly one scheduler-enabled worker |
| Submission | Held until the explicit go/no-go checkpoint |
| Success | All external axes consistent, outputs verified, immutable evidence captured |

The demo operator, CASDA data owner, DALiuGE owner, and Setonix account owner
must agree on the source, expected products, maintenance window, resource
request, and cancellation contact before the day of the run.

## 1. Close the readiness gates

Record evidence for these checks in the release ticket:

```text
DEMO-001 fixture discovery merged and passing in CI
DEMO-002 output verifier merged and WALLABY output contract active
DEMO-003 project graph content-addressed or checksum-pinned
DEMO-004 canonical harness tracked and passing fixture mode
DEMO-005 setup/profile validation fixed
DEMO-006 selected live integration versions qualified
```

Stop if any line is false. An L2/L3 rehearsal may proceed only when it is
labelled as such and cannot be mistaken for scientific completion.

## 2. Prepare the operator host

Install `beampipe`, PostgreSQL client tools, `curl`, `jq`, and Prometheus. Join
the facility VPN and obtain a released Beampipe binary or build the audited
commit. From the repository checkout:

```bash
export REPO_ROOT=$(pwd)
export BEAMPIPE_BIN=${BEAMPIPE_BIN:-$REPO_ROOT/target/release/beampipe}

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
cargo build --locked --release -p beampipe-cli --bin beampipe
"$BEAMPIPE_BIN" project validate -f config/wallaby_hires.v2.yaml
git rev-parse HEAD
git status --short
```

Use a release artifact instead of the Cargo build when qualifying a published
release. The commit must be approved, and unexpected tracked changes stop the
run. Untracked credentials or generated profiles must never live in the
checkout.

Create a private operator directory:

```bash
export DEMO_ID=$(date -u +%Y%m%dT%H%M%SZ)
export DEMO_HOME=$HOME/.local/state/beampipe-demo/$DEMO_ID
umask 077
mkdir -p "$DEMO_HOME"/{logs,evidence,prometheus}
"$BEAMPIPE_BIN" init --production --directory "$DEMO_HOME"
cd "$DEMO_HOME"
```

## 3. Inject configuration and secrets

Provision a new PostgreSQL database. This sample assumes the database
administrator has already exposed a secure connection through
`DATABASE_ADMIN_URL`:

```bash
export DEMO_DB=beampipe_demo_${DEMO_ID//[-:TZ]/_}
createdb --maintenance-db="$DATABASE_ADMIN_URL" "$DEMO_DB"
export DATABASE_URL="${DATABASE_ADMIN_URL%/*}/$DEMO_DB"

export BEAMPIPE_ENV=production
export BEAMPIPE_CONFIG=$DEMO_HOME/beampipe.yaml
export BEAMPIPE_BIND_ADDR=127.0.0.1:8080
export BEAMPIPE_CORS_ALLOW_ORIGINS=http://127.0.0.1:8080
export BEAMPIPE_METRICS_PUBLIC=false
export BEAMPIPE_USE_REAL_BACKENDS=true
export BEAMPIPE_CASDA_TAP_URL=https://casda.csiro.au/casda_vo_tools/tap/sync
export BEAMPIPE_VIZIER_TAP_URL=https://tapvizier.cds.unistra.fr/TAPVizieR/tap/sync

read -rsp 'JWT signing secret (32+ characters): ' BEAMPIPE_JWT_SECRET; echo
export BEAMPIPE_JWT_SECRET
```

Supply secrets as files owned by the service user or root and readable by the
Beampipe process:

```bash
export CASDA_USERNAME='replace-with-casda-user'
export CASDA_PASSWORD_FILE=/run/credentials/beampipe/casda_password
export CASDA_LOGIN_URL=https://data.csiro.au/casda_vo_proxy/vo/tap/availability

export SLURM_SSH_PRIVATE_KEY_FILE=/run/credentials/beampipe/setonix_key
export SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE=/run/credentials/beampipe/setonix_passphrase
export SLURM_SSH_KNOWN_HOSTS_SOURCE=/run/credentials/beampipe/known_hosts
export BEAMPIPE_SLURM_SSH_STRICT_KNOWN_HOSTS=true
export BEAMPIPE_SLURM_SSH_ALLOW_HOME_FALLBACK=false
export BEAMPIPE_ALLOW_INLINE_SECRETS=false
export BEAMPIPE_ALLOW_INSECURE_SSH_HOST_KEYS=false
```

Omit `SLURM_SSH_PRIVATE_KEY_PASSPHRASE_FILE` only for a key that genuinely has
no passphrase. Obtain the known-hosts line through a trusted facility channel;
do not establish trust with `StrictHostKeyChecking=accept-new` during the demo.

Check local file policy without printing contents:

```bash
test -s "$CASDA_PASSWORD_FILE"
test -f "$SLURM_SSH_PRIVATE_KEY_FILE"
test -s "$SLURM_SSH_KNOWN_HOSTS_SOURCE"
stat -f '%Sp %Su:%Sg %N' "$SLURM_SSH_PRIVATE_KEY_FILE" 2>/dev/null || \
  stat -c '%A %U:%G %n' "$SLURM_SSH_PRIVATE_KEY_FILE"
```

## 4. Bootstrap the exact project and profile

Set facility-owned, non-secret values. Do not use the personal example profile
from the repository unchanged.

```bash
export DEMO_ADMIN_USER=demo-admin
export DEMO_SOURCE='replace-with-approved-wallaby-source'
export SETONIX_USER='replace-with-setonix-user'
export SETONIX_ACCOUNT='replace-with-setonix-project'
export SETONIX_PARTITION=work
export SETONIX_HOME=/scratch/$SETONIX_ACCOUNT
export SETONIX_DLG_ROOT=$SETONIX_HOME/$SETONIX_USER/dlg
export SETONIX_LOGS=$SETONIX_DLG_ROOT/log
export BEAMPIPE_TM_URL='https://replace-with-approved-translator'

"$BEAMPIPE_BIN" setup \
  --admin-user "$DEMO_ADMIN_USER" \
  --project-config "$REPO_ROOT/config/wallaby_hires.v2.yaml" \
  --casda-tap-url "$BEAMPIPE_CASDA_TAP_URL" \
  --tm-url "$BEAMPIPE_TM_URL" \
  --deployment slurm_remote \
  --profile-name slurm-remote \
  --facility setonix \
  --ssh-host setonix.pawsey.org.au \
  --ssh-user "$SETONIX_USER" \
  --slurm-account "$SETONIX_ACCOUNT" \
  --slurm-partition "$SETONIX_PARTITION" \
  --remote-home "$SETONIX_HOME" \
  --dlg-root "$SETONIX_DLG_ROOT" \
  --remote-logs "$SETONIX_LOGS" \
  --use-real-backends
```

`setup` consumes the exported JWT secret and, after `DEMO-005`, the exported
database URL. During a rehearsal on the current CLI, enter the dedicated
`DATABASE_URL` when prompted. It prompts for the administrator password without
placing either secret in shell history. After `DEMO-005`, it also runs doctor
against `slurm-remote` and fails if the project references a missing or
incompatible profile.

Run the complete preflight again and retain redacted output:

```bash
"$BEAMPIPE_BIN" migrate
"$BEAMPIPE_BIN" security check | tee "$DEMO_HOME/evidence/security-check.txt"
"$BEAMPIPE_BIN" project validate -f "$REPO_ROOT/config/wallaby_hires.v2.yaml" \
  | tee "$DEMO_HOME/evidence/project-validation.json"
"$BEAMPIPE_BIN" profile validate slurm-remote \
  | tee "$DEMO_HOME/evidence/profile-validation.json"
"$BEAMPIPE_BIN" profile render slurm-remote \
  | tee "$DEMO_HOME/evidence/profile-render.json"
"$BEAMPIPE_BIN" doctor --profile slurm-remote --json \
  | tee "$DEMO_HOME/evidence/doctor.json"
"$BEAMPIPE_BIN" slurm ping --profile slurm-remote \
  | tee "$DEMO_HOME/evidence/slurm-ping.json"
"$BEAMPIPE_BIN" daliuge inspect --profile slurm-remote \
  | tee "$DEMO_HOME/evidence/daliuge-inspect.json"
"$BEAMPIPE_BIN" bench tap --source "$DEMO_SOURCE" \
  --config "$REPO_ROOT/config/wallaby_hires.v2.yaml" --runs 1 \
  | tee "$DEMO_HOME/evidence/tap-bench.json"
```

Every command must exit zero. A warning about missing workers is acceptable
before services start; dependency, security, project, profile, graph, TM, SSH,
remote-directory, or TAP failures are not.

## 5. Start with submission fenced

Start the API and a scheduler worker that can discover and prepare metadata but
cannot claim deployment jobs. This provides a deliberate inspection window
before external execution.

```bash
BEAMPIPE_PROCESS_ROLE=api \
BEAMPIPE_METRICS_BIND_ADDR=127.0.0.1:9090 \
"$BEAMPIPE_BIN" serve --worker false \
  >"$DEMO_HOME/logs/api.log" 2>&1 &
export API_PID=$!

BEAMPIPE_PROCESS_ROLE=scheduler \
BEAMPIPE_WORKER_INSTANCE_NAME=demo-scheduler \
BEAMPIPE_WORKER_SCHEDULER_ENABLED=true \
BEAMPIPE_WORKER_CAPABILITIES=casda-discovery,manifest-generation \
BEAMPIPE_METRICS_BIND_ADDR=127.0.0.1:9091 \
"$BEAMPIPE_BIN" worker \
  >"$DEMO_HOME/logs/scheduler.log" 2>&1 &
export SCHEDULER_PID=$!

for attempt in $(seq 1 60); do
  curl -fsS http://127.0.0.1:8080/api/v2/health >/dev/null && break
  sleep 1
done
curl -fsS http://127.0.0.1:8080/api/v2/health | jq -e .
```

Keep these PIDs in the same shell. If either process exits, stop the demo and
inspect its redacted log before retrying.

## 6. Start Prometheus

Create a host scrape configuration for the three demo roles:

```bash
cat >"$DEMO_HOME/prometheus/prometheus.yml" <<'YAML'
global:
  scrape_interval: 5s
scrape_configs:
  - job_name: beampipe-api
    static_configs:
      - targets: [127.0.0.1:9090]
  - job_name: beampipe-scheduler
    static_configs:
      - targets: [127.0.0.1:9091]
  - job_name: beampipe-deployment-worker
    static_configs:
      - targets: [127.0.0.1:9092]
YAML

prometheus \
  --config.file="$DEMO_HOME/prometheus/prometheus.yml" \
  --storage.tsdb.path="$DEMO_HOME/prometheus/data" \
  --web.listen-address=127.0.0.1:9099 \
  >"$DEMO_HOME/logs/prometheus.log" 2>&1 &
export PROMETHEUS_PID=$!
```

After `DEMO-101`, use the provisioned Grafana overview for the operator display.
API traffic belongs on that overview alongside queue, workers, dependencies,
discovery, and execution state.

## 7. Authenticate and verify active contracts

Read the password used during setup and create a short-lived access token:

```bash
export BASE=http://127.0.0.1:8080
read -rsp 'Demo administrator password: ' DEMO_ADMIN_PASSWORD; echo
export TOKEN=$(jq -n \
  --arg username "$DEMO_ADMIN_USER" \
  --arg password "$DEMO_ADMIN_PASSWORD" \
  '{username:$username,password:$password}' \
  | curl -fsS -X POST "$BASE/api/v2/login" \
      -H 'Content-Type: application/json' --data-binary @- \
  | jq -er .access_token)
unset DEMO_ADMIN_PASSWORD
export AUTH="Authorization: Bearer $TOKEN"
```

Assert the active project and installed profile before creating work:

```bash
curl -fsS "$BASE/api/v2/projects" -H "$AUTH" \
  | tee "$DEMO_HOME/evidence/projects.json" \
  | jq -e 'any(.[]; .project_id == "wallaby_hires" and .active == true)'

curl -fsS "$BASE/api/v2/deployment-profiles?project_module=wallaby_hires" -H "$AUTH" \
  | tee "$DEMO_HOME/evidence/profiles.json" \
  | jq -e 'any(.[];
      .name == "slurm-remote" and
      .project_module == "wallaby_hires" and
      .is_default == true and
      .deployment.kind == "slurm_remote")'
```

## 8. Register and discover the approved source

Register idempotently and trigger only that source:

```bash
export SOURCE_ID=$(jq -n \
  --arg project wallaby_hires --arg source "$DEMO_SOURCE" \
  '{project_module:$project,source_identifier:$source,enabled:true}' \
  | curl -fsS -X POST "$BASE/api/v2/sources" \
      -H "$AUTH" -H 'Content-Type: application/json' --data-binary @- \
  | tee "$DEMO_HOME/evidence/source-created.json" \
  | jq -er .uuid)

jq -n --arg project wallaby_hires --arg source "$DEMO_SOURCE" \
  '{project_module:$project,source_identifier:$source}' \
  | curl -fsS -X POST "$BASE/api/v2/sources/discover" \
      -H "$AUTH" -H 'Content-Type: application/json' --data-binary @- \
  | tee "$DEMO_HOME/evidence/discovery-trigger.json" \
  | jq -e '.marked_count == 1'
```

Wait up to 15 minutes for the normal scheduler and TAP path:

```bash
for attempt in $(seq 1 180); do
  curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH" \
    >"$DEMO_HOME/evidence/source-status.json"
  jq -e '.ready_for_execution == true' \
    "$DEMO_HOME/evidence/source-status.json" >/dev/null && break
  sleep 5
done
jq -e '.ready_for_execution == true and .discovery_signature != null' \
  "$DEMO_HOME/evidence/source-status.json"
curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/metadata" -H "$AUTH" \
  | tee "$DEMO_HOME/evidence/source-metadata.json" \
  | jq -e '.metadata_count > 0 and (.metadata | length > 0)'
curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/events" -H "$AUTH" \
  >"$DEMO_HOME/evidence/source-events.json"
```

## 9. Inspect preparation and automatic admission

Confirm the source passes execution preparation and build the graph without
submitting:

```bash
jq -n --arg source "$DEMO_SOURCE" '{
  project_module:"wallaby_hires",
  sources:[{source_identifier:$source}],
  archive_name:"casda",
  deployment_profile_name:"slurm-remote"
}' | curl -fsS -X POST "$BASE/api/v2/executions/prepare" \
      -H "$AUTH" -H 'Content-Type: application/json' --data-binary @- \
  | tee "$DEMO_HOME/evidence/execution-prepare.json" \
  | jq -e '.valid == true and .total_datasets > 0'

"$BEAMPIPE_BIN" graph prepare --project wallaby_hires --source "$DEMO_SOURCE" \
  | tee "$DEMO_HOME/evidence/graph-prepare.json" \
  | jq -e '.source_graph_sha256 != null and .patched_graph_sha256 != null'
```

The project automation scheduler should create an execution and queue its
deployment job. The restricted worker cannot claim that job:

```bash
for attempt in $(seq 1 60); do
  export EXEC_ID=$(curl -fsS \
    "$BASE/api/v2/executions?project_module=wallaby_hires&items_per_page=50" \
    -H "$AUTH" | jq -r --arg source "$DEMO_SOURCE" '
      [.items[] | select(any(.sources[]?; (.source_identifier? // .) == $source))]
      | first | .uuid // empty')
  test -n "$EXEC_ID" && break
  sleep 5
done
test -n "$EXEC_ID"

curl -fsS "$BASE/api/v2/executions/$EXEC_ID" -H "$AUTH" \
  | tee "$DEMO_HOME/evidence/execution-before-submit.json" \
  | jq -e '
      .deployment_profile_id != null and
      .deployment_profile_revision != null and
      .project_config_id != null and
      .project_config_version != null and
      .scheduler_job_id == null and
      .daliuge_session_id == null'
```

If automation has not created an execution, stop and inspect scheduler events.
Do not manually create a second path during the qualification run.

## 10. Go/no-go checkpoint

Display and verbally confirm all of the following:

```bash
jq '{valid,total_datasets,sources_preview}' "$DEMO_HOME/evidence/execution-prepare.json"
jq '{source_graph_sha256,patched_graph_sha256}' "$DEMO_HOME/evidence/graph-prepare.json"
jq '{uuid,project_config_id,project_config_version,deployment_profile_id,deployment_profile_revision}' \
  "$DEMO_HOME/evidence/execution-before-submit.json"
"$BEAMPIPE_BIN" profile render slurm-remote | jq '{profile:.profile.name,resource_request,rendered}'
```

The operator and facility owner must confirm the source, dataset count, graph
hash, profile revision, account, partition, nodes, tasks, wall time, modules,
DALiuGE paths, TM endpoint, and cancellation route.

Require an exact acknowledgement:

```bash
read -r -p "Type SUBMIT $EXEC_ID to start CASDA staging and Setonix work: " ACK
test "$ACK" = "SUBMIT $EXEC_ID"
```

Any mismatch is a no-go. Correct it as a new config/profile revision and start
a new execution; do not mutate the prepared record.

## 11. Release the deployment job

Start one worker with the capabilities required to claim execution and polling
jobs. It is not scheduler-enabled, so recurring ticks remain single-owner.

```bash
BEAMPIPE_PROCESS_ROLE=worker \
BEAMPIPE_WORKER_INSTANCE_NAME=demo-deployment-worker \
BEAMPIPE_WORKER_SCHEDULER_ENABLED=false \
BEAMPIPE_WORKER_CAPABILITIES=daliuge-translation,daliuge-deployment,slurm-remote,output-verification \
BEAMPIPE_METRICS_BIND_ADDR=127.0.0.1:9092 \
"$BEAMPIPE_BIN" worker \
  >"$DEMO_HOME/logs/deployment-worker.log" 2>&1 &
export DEPLOYMENT_WORKER_PID=$!
```

Watch the normal control-plane state; do not enqueue manual poll ticks:

```bash
"$BEAMPIPE_BIN" console
```

In another shell with the same environment, use:

```bash
"$BEAMPIPE_BIN" timeline execution "$EXEC_ID" --table
"$BEAMPIPE_BIN" scheduler jobs --limit 20
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH" | jq .
```

## 12. Wait for verified terminal success

Derive the operational deadline from the approved profile wall time plus
staging, queue, and verification allowance. This example allows 90 minutes:

```bash
for attempt in $(seq 1 360); do
  curl -fsS "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH" \
    >"$DEMO_HOME/evidence/execution-status.json"
  STATUS=$(jq -r .status "$DEMO_HOME/evidence/execution-status.json")
  case "$STATUS" in
    completed|failed|cancelled) break ;;
  esac
  sleep 15
done

jq -e '
  .status == "completed" and
  .control_phase == "terminal" and
  .submission_state == "submitted" and
  .scheduler_state == "succeeded" and
  .daliuge_state == "finished" and
  .output_state == "verified" and
  .terminal_outcome == "succeeded" and
  .scheduler_job_id != null and
  .daliuge_session_id != null and
  .last_error == null
' "$DEMO_HOME/evidence/execution-status.json"
```

Any other terminal state fails the demo. A timeout fails the demo without
implying that cancellation is safe; inspect the durable axes first.

## 13. Capture and validate evidence

Fetch only redacted API views and deterministic summaries:

```bash
curl -fsS "$BASE/api/v2/executions/$EXEC_ID" -H "$AUTH" \
  >"$DEMO_HOME/evidence/execution.json"
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/ledger-snapshot?include_manifest=true" -H "$AUTH" \
  >"$DEMO_HOME/evidence/ledger-snapshot.json"
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/observations?limit=500" -H "$AUTH" \
  >"$DEMO_HOME/evidence/observations.json"
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/artifacts" -H "$AUTH" \
  >"$DEMO_HOME/evidence/artifacts.json"
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/events" -H "$AUTH" \
  >"$DEMO_HOME/evidence/execution-events.json"
"$BEAMPIPE_BIN" graph diff --execution "$EXEC_ID" \
  >"$DEMO_HOME/evidence/graph-diff.json"
"$BEAMPIPE_BIN" timeline execution "$EXEC_ID" \
  >"$DEMO_HOME/evidence/timeline.json"
"$BEAMPIPE_BIN" status >"$DEMO_HOME/evidence/control-plane-status.json"
curl -fsS http://127.0.0.1:9099/api/v1/targets \
  >"$DEMO_HOME/evidence/prometheus-targets.json"
git -C "$REPO_ROOT" rev-parse HEAD >"$DEMO_HOME/evidence/git-commit.txt"
sha256sum "$REPO_ROOT/config/wallaby_hires.v2.yaml" \
  "$DEMO_HOME/config/deployment_profile.slurm-remote.json" \
  >"$DEMO_HOME/evidence/input-sha256.txt"
```

Assert the artifact and monitoring contract after `DEMO-002` and `DEMO-101`:

```bash
jq -e '
  ([.[].kind] | index("manifest") != null) and
  ([.[].kind] | index("source_graph") != null) and
  ([.[].kind] | index("patched_graph") != null) and
  ([.[].kind] | index("physical_graph") != null) and
  ([.[].kind] | index("output_verification_report") != null)
' "$DEMO_HOME/evidence/artifacts.json"

jq -e '[.data.activeTargets[] | select(.labels.job | startswith("beampipe-"))]
  | length == 3 and all(.[]; .health == "up")' \
  "$DEMO_HOME/evidence/prometheus-targets.json"
```

Review the bundle with the shared redaction scanner before distribution. It
must not contain passwords, private keys, bearer tokens, authorization headers,
or signed dataset URLs. Add a small `README` outside the bundle recording demo
level, release, UTC start/end, operator, source, execution ID, scheduler job ID,
DALiuGE session ID, terminal outcome, and approval ticket.

## 14. Stop services and retain state

Stop local processes cleanly, but keep the dedicated database until the result
has been reviewed:

```bash
kill "$DEPLOYMENT_WORKER_PID" "$SCHEDULER_PID" "$API_PID" "$PROMETHEUS_PID"
wait "$DEPLOYMENT_WORKER_PID" "$SCHEDULER_PID" "$API_PID" "$PROMETHEUS_PID" 2>/dev/null || true
unset TOKEN AUTH BEAMPIPE_JWT_SECRET
```

After the review and retention window, remove only the database whose generated
name is recorded in the evidence:

```bash
case "$DEMO_DB" in
  beampipe_demo_*) dropdb --maintenance-db="$DATABASE_ADMIN_URL" "$DEMO_DB" ;;
  *) echo "refusing to drop non-demo database: $DEMO_DB"; false ;;
esac
```

Archive the evidence through the approved secure operational store, not Git.

## Failure and cancellation

On any failed assertion:

1. Stop new admissions by stopping the scheduler worker.
2. Read the execution axes, observations, events, and active lease.
3. If an external ID exists and cancellation is approved, run:

```bash
"$BEAMPIPE_BIN" execution cancel "$EXEC_ID"
```

4. Confirm Slurm and DALiuGE independently before treating cancellation as
   complete.
5. Preserve the failed evidence bundle. Do not retry an uncertain submission.
6. Use a new execution for corrected config/profile inputs; use stage-aware
   retry only for a classified safe recovery.

The [Recovery and cancellation](recovery.md) runbook governs ambiguous external
state. Receiving a Slurm job ID, seeing `COMPLETED` in Slurm, or seeing a
DALiuGE finished state is never sufficient by itself to declare the L4 demo a
success.

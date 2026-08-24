# Local DALiuGE end-to-end qualification

This runbook proves the complete live path on one Docker host: public
CASDA/VizieR discovery, immutable project and profile pinning, manifest and
graph materialization, Translator Manager translation, REST deployment to a
DALiuGE Data Island Manager, polling, and terminal evidence. It uses the
WALLABY no-download graph so no CASDA staging credential or durable product
destination is required.

This is a qualification topology, not a production deployment. Use a dedicated
Compose project and database. The cleanup command at the end deletes that test
database.

## Proven topology

There are three different network viewpoints. Do not replace these names with
`127.0.0.1`: inside a container, loopback refers to that container.

| Caller | Target | Qualification value |
|---|---|---|
| Beampipe worker | Translator Manager | `http://dlg-tm.desk` |
| Translator Manager | DIM | `dlg-dim:8001` |
| Beampipe worker | DIM REST API | `dlg-dim.desk:80` through Traefik |

<div class="bp-terminal-frame bp-topology-explorer" role="group" aria-label="DALiuGE topology address explorer" data-title="network viewpoints" data-bp-explorer>
  <div class="bp-segmented" role="tablist" aria-label="DALiuGE network mode">
    <button type="button" role="tab" id="bp-topology-qualified-tab" aria-controls="bp-topology-qualified" aria-selected="true" tabindex="0" data-bp-target="bp-topology-qualified">Qualified .desk / Traefik</button>
    <button type="button" role="tab" id="bp-topology-direct-tab" aria-controls="bp-topology-direct" aria-selected="false" tabindex="-1" data-bp-target="bp-topology-direct">Direct shared network</button>
  </div>
  <p class="bp-interactive-fallback">Both valid address sets are listed when scripting is unavailable and in print.</p>
  <div class="bp-explorer-output">
    <section id="bp-topology-qualified" role="tabpanel" aria-labelledby="bp-topology-qualified-tab" data-bp-panel tabindex="0">
      <h3>Qualified host-routed mode</h3>
      <ol class="bp-topology-routes" aria-label="Qualified DALiuGE request routes">
        <li><span class="bp-topology-route__index">01</span><strong>Beampipe worker</strong><span class="bp-topology-route__arrow" aria-hidden="true">--&gt;</span><code>http://dlg-tm.desk</code><small>logical graph translation</small></li>
        <li><span class="bp-topology-route__index">02</span><strong>Translator Manager</strong><span class="bp-topology-route__arrow" aria-hidden="true">--&gt;</span><code>dlg-dim:8001</code><small>DIM address embedded in the translation request</small></li>
        <li><span class="bp-topology-route__index">03</span><strong>Beampipe worker</strong><span class="bp-topology-route__arrow" aria-hidden="true">--&gt;</span><code>dlg-dim.desk:80</code><small>deploy and poll through Traefik</small></li>
      </ol>
    </section>
    <section id="bp-topology-direct" role="tabpanel" aria-labelledby="bp-topology-direct-tab" data-bp-panel tabindex="0" hidden>
      <h3>Direct shared-Docker-network mode</h3>
      <ol class="bp-topology-routes" aria-label="Direct DALiuGE request routes">
        <li><span class="bp-topology-route__index">01</span><strong>Beampipe worker</strong><span class="bp-topology-route__arrow" aria-hidden="true">--&gt;</span><code>http://dlg-tm:8084</code><small>logical graph translation</small></li>
        <li><span class="bp-topology-route__index">02</span><strong>Translator Manager</strong><span class="bp-topology-route__arrow" aria-hidden="true">--&gt;</span><code>dlg-dim:8001</code><small>DIM address embedded in the translation request</small></li>
        <li><span class="bp-topology-route__index">03</span><strong>Beampipe worker</strong><span class="bp-topology-route__arrow" aria-hidden="true">--&gt;</span><code>dlg-dim:8001</code><small>direct deploy and poll on the shared network</small></li>
      </ol>
    </section>
  </div>
</div>

`translation.tm_url` is resolved by Beampipe. `dim_host_for_tm` is embedded in
the translation request and must be resolvable by TM. `deploy_host` and
`deploy_port` are used later by the Beampipe worker. A deployment can therefore
pass the TM probe and still fail if either DIM path is wrong.

The repository overlay also supports direct Docker service names when Core is
attached to `docker_dlg-local`. The values above document the topology that was
qualified end to end, including the Traefik `*.desk` route.

## Prerequisites and checkout layout

- Docker Engine and Compose v2
- `curl` and `jq`
- a DALiuGE checkout with its local Docker stack
- sibling Core and WALLABY checkouts, because `compose.dlg-local.yml` mounts
  `../wallaby-hires-beampipe/dlg-graphs`

```text
workspace/
|-- beampipe-core-v2/
`-- wallaby-hires-beampipe/

daliuge/
`-- docker/dlg/workspace/wallaby-hires-beampipe/
```

The WALLABY checkout inside `/dlg/workspace` is the runtime copy seen by all
DALiuGE containers. Keep it at the same tested commit as the source checkout.
Prove that before installation:

```bash
export DALIUGE_ROOT=/path/to/daliuge
export WALLABY_SOURCE_ROOT=/path/to/workspace/wallaby-hires-beampipe
export WALLABY_RUNTIME_ROOT="$DALIUGE_ROOT/docker/dlg/workspace/wallaby-hires-beampipe"
test "$(git -C "$WALLABY_SOURCE_ROOT" rev-parse HEAD)" = \
  "$(git -C "$WALLABY_RUNTIME_ROOT" rev-parse HEAD)"
```

## 1. Start DALiuGE and install the graph applications

From the DALiuGE checkout:

```bash
cd "$DALIUGE_ROOT"

make docker-install
make docker-run
docker compose -f docker/docker-compose.yaml ps
```

`make docker-install` is needed for the first build and after DALiuGE image
changes. Reinstall the checked-out WALLABY package into every process image so
TM, DIM, and both node managers cannot retain an older wheel or editable
checkout:

```bash
for service in dlg-tm dlg-dim dlg-nm1 dlg-nm2; do
  docker exec --workdir /dlg/workspace/wallaby-hires-beampipe "$service" \
    make install PYTHON=/daliuge/.venv/bin/python
done

# Restart, do not recreate: restart keeps the package installed in each
# container's writable layer and forces long-running Python processes to reload.
docker compose -f docker/docker-compose.yaml \
  restart dlg-tm dlg-dim dlg-nm1 dlg-nm2

for service in dlg-tm dlg-dim dlg-nm1 dlg-nm2; do
  docker exec "$service" /daliuge/.venv/bin/python -c \
    'import importlib.metadata; print(importlib.metadata.version("wallaby-hires"))'
done
```

All four versions must match. These host probes verify Traefik routing, even
when host DNS does not resolve `.desk`; they do not prove that a Core container
can resolve the same names:

```bash
curl -fsS -H 'Host: dlg-tm.desk' http://127.0.0.1/ >/dev/null
curl -fsS -H 'Host: dlg-dim.desk' http://127.0.0.1/api >/dev/null
docker network inspect docker_dlg-local >/dev/null
```

## 2. Start an isolated Core stack

From the Core checkout, use a distinct project name. The API applies migrations
on startup; scheduler and worker services wait for its health check, so a fresh
database needs no separate manual migration command.

```bash
export BEAMPIPE_ROOT=/path/to/workspace/beampipe-core-v2
export COMPOSE_PROJECT_NAME=beampipe-qualification
cd "$BEAMPIPE_ROOT"

BEAMPIPE_BUILD=1 ./deploy/setup-docker.sh \
  --yes --skip-admin --skip-upload

docker compose -f docker-compose.yml -f compose.dlg-local.yml up -d --wait
docker compose -f docker-compose.yml -f compose.dlg-local.yml ps
curl -fsS http://127.0.0.1:18080/api/v2/health | jq .
```

Now probe from the callers that will use each address. The TM-to-DIM route must
work in both modes. The loop selects `.desk` only when API, scheduler, and
worker containers can all resolve and reach both Traefik routes; otherwise it
selects the direct shared-network fallback used by the profile step below.

```bash
docker exec dlg-tm /daliuge/.venv/bin/python -c \
  'import urllib.request; urllib.request.urlopen("http://dlg-dim:8001/api", timeout=10).read(1)'

if for service in api scheduler worker; do
  docker compose -f docker-compose.yml -f compose.dlg-local.yml \
    exec -T "$service" sh -ec '
      getent hosts dlg-tm.desk >/dev/null
      getent hosts dlg-dim.desk >/dev/null
      curl -fsS http://dlg-tm.desk/ >/dev/null
      curl -fsS http://dlg-dim.desk/api >/dev/null
    '
done; then
  export DALIUGE_ADDRESS_MODE=desk
else
  export DALIUGE_ADDRESS_MODE=direct
  echo 'Using direct dlg-tm/dlg-dim service names on docker_dlg-local'
fi
```

Do not treat the earlier forced `Host` header as a substitute for these
caller-context checks. The direct mode is valid because the overlay attaches
all three Core roles to `docker_dlg-local`.

The overlay publishes qualification PostgreSQL on `15432`, the API on `18080`,
and API metrics on `19090`. Stop or re-port any service already using those
ports.

Create the first superuser. The command prompts for a password instead of
placing it in shell history:

```bash
docker compose -f docker-compose.yml -f compose.dlg-local.yml run --rm api \
  admin create-user \
  --username operator \
  --email operator@example.test \
  --name 'Qualification operator'
```

## 3. Install immutable project and profile policy

The no-download project points at the graph bundled with Core and verifies its
SHA-256 before materialization. Validate it before uploading:

```bash
docker compose -f docker-compose.yml -f compose.dlg-local.yml run --rm api \
  project validate \
  -f /var/lib/beampipe/config/wallaby_hires_nodownloads.v2.yaml
```

Authenticate and upload it:

```bash
export BASE=http://127.0.0.1:18080
export ADMIN_USER=operator
export ADMIN_PASSWORD="${ADMIN_PASSWORD:?export the password you entered}"
LOGIN_BODY=$(jq -n \
  --arg username "$ADMIN_USER" \
  --arg password "$ADMIN_PASSWORD" \
  '{username:$username,password:$password}')
export TOKEN=$(curl -fsS -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d "$LOGIN_BODY" \
  | jq -er .access_token)
export AUTH="Authorization: Bearer $TOKEN"

curl -fsS -X POST "$BASE/api/v2/project-configs" \
  -H "$AUTH" -H 'Content-Type: application/x-yaml' \
  --data-binary @config/wallaby_hires_nodownloads.v2.yaml | jq .
```

Create the profile with each address expressed from the caller's viewpoint:

```bash
cat > /tmp/beampipe-dlg-desk-profile.json <<'JSON'
{
  "name": "dlg-desk",
  "description": "Local DALiuGE qualification through Traefik",
  "project_module": "wallaby_hires",
  "is_default": true,
  "max_concurrent_executions": 1,
  "translation": {
    "algo": "metis",
    "num_par": 1,
    "num_islands": 0,
    "tm_url": "http://dlg-tm.desk"
  },
  "deployment": {
    "kind": "rest_remote",
    "dim_host_for_tm": "dlg-dim",
    "dim_port_for_tm": 8001,
    "deploy_host": "dlg-dim.desk",
    "deploy_port": 80,
    "use_https": false,
    "verify_ssl": true
  }
}
JSON

jq -e . /tmp/beampipe-dlg-desk-profile.json >/dev/null

export DALIUGE_PROFILE_NAME=dlg-desk
export DALIUGE_PROFILE_FILE=/tmp/beampipe-dlg-desk-profile.json
if [ "${DALIUGE_ADDRESS_MODE:-desk}" = direct ]; then
  export DALIUGE_PROFILE_NAME=dlg-direct
  export DALIUGE_PROFILE_FILE=/tmp/beampipe-dlg-direct-profile.json
  jq '
    .name = "dlg-direct"
    | .description = "Local DALiuGE qualification on the shared Docker network"
    | .translation.tm_url = "http://dlg-tm:8084"
    | .deployment.deploy_host = "dlg-dim"
    | .deployment.deploy_port = 8001
  ' /tmp/beampipe-dlg-desk-profile.json >"$DALIUGE_PROFILE_FILE"
fi

jq -e . "$DALIUGE_PROFILE_FILE" >/dev/null
curl -fsS -X POST "$BASE/api/v2/deployment-profiles" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d @"$DALIUGE_PROFILE_FILE" | jq .

docker compose -f docker-compose.yml -f compose.dlg-local.yml run --rm api \
  doctor --profile "$DALIUGE_PROFILE_NAME"
```

Do not enable submission until the profile doctor can reach both TM and DIM.
The setup above deliberately leaves `BEAMPIPE_USE_REAL_BACKENDS=false`. After
the doctor reports successful translator and manager checks (not merely an
overall zero exit while live checks are optional), edit the isolated checkout's
`.env` and set:

```text
BEAMPIPE_USE_REAL_BACKENDS=true
```

Recreate only the Core roles with the same overlay, then rerun the doctor:

```bash
docker compose -f docker-compose.yml -f compose.dlg-local.yml \
  up -d --wait --force-recreate api scheduler worker
docker compose -f docker-compose.yml -f compose.dlg-local.yml run --rm api \
  doctor --profile "$DALIUGE_PROFILE_NAME"
```

No execution exists yet and the no-download project's automatic execution
policy is disabled, so this enablement cannot race an admission created by the
steps above.

## 4. Discover one real source

`HIPASSJ1318-21` produced three CASDA datasets during the qualification run.
Archive holdings can change, so readiness and a stable discovery signature are
the acceptance criteria, not that historical count.

```bash
SOURCE=$(curl -fsS -X POST "$BASE/api/v2/sources" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{
    "project_module":"wallaby_hires",
    "source_identifier":"HIPASSJ1318-21",
    "enabled":true
  }')
SOURCE_ID=$(jq -er .uuid <<<"$SOURCE")

curl -fsS -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{
    "project_module":"wallaby_hires",
    "source_identifier":"HIPASSJ1318-21"
  }' | jq .
```

Discovery is asynchronous. Poll for up to ten minutes because public TAP
latency varies:

```bash
for attempt in $(seq 1 120); do
  SOURCE_STATUS=$(curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH")
  jq '{ready_for_execution,discovery_complete,discovery_signature,blockers}' \
    <<<"$SOURCE_STATUS"
  jq -e '.ready_for_execution == true' <<<"$SOURCE_STATUS" >/dev/null && break
  sleep 5
done
jq -e '.ready_for_execution == true and .discovery_complete == true and
       ((.discovery_signature // "") | length > 0)' \
  <<<"$SOURCE_STATUS" >/dev/null
FIRST_DISCOVERY_SIGNATURE=$(jq -er '.discovery_signature' <<<"$SOURCE_STATUS")
```

If the loop expires, inspect the source events, worker queue, and TAP health
instead of creating an execution with incomplete metadata.

Repeat discovery once and require the same non-empty signature. This proves
that deterministic normalization, rather than the historical dataset count,
is the acceptance signal:

```bash
curl -fsS -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{
    "project_module":"wallaby_hires",
    "source_identifier":"HIPASSJ1318-21"
  }' | jq .

for attempt in $(seq 1 120); do
  SOURCE_STATUS=$(curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH")
  jq '{ready_for_execution,discovery_complete,discovery_signature,blockers}' \
    <<<"$SOURCE_STATUS"
  jq -e '.discovery_complete == true and
         ((.discovery_signature // "") | length > 0)' \
    <<<"$SOURCE_STATUS" >/dev/null && break
  sleep 5
done
SECOND_DISCOVERY_SIGNATURE=$(jq -er '.discovery_signature' <<<"$SOURCE_STATUS")
test "$SECOND_DISCOVERY_SIGNATURE" = "$FIRST_DISCOVERY_SIGNATURE"
```

## 5. Preflight and materialize the graph

The CLI materialization is local control-plane work: it generates the manifest,
resolves the bundled graph, applies deterministic patches, and reports hashes.
It does not deploy to DALiuGE.

```bash
docker compose -f docker-compose.yml -f compose.dlg-local.yml run --rm api \
  graph prepare --project wallaby_hires --source HIPASSJ1318-21

jq -n --arg profile "$DALIUGE_PROFILE_NAME" '{
  project_module: "wallaby_hires",
  sources: [{source_identifier: "HIPASSJ1318-21"}],
  archive_name: "casda",
  deployment_profile_name: $profile
}' >/tmp/beampipe-execution.json

curl -fsS -X POST "$BASE/api/v2/executions/prepare" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d @/tmp/beampipe-execution.json \
  | tee /tmp/beampipe-preflight.json | jq .
jq -e '.valid == true and .total_datasets > 0' \
  /tmp/beampipe-preflight.json >/dev/null
```

## 6. Create and execute idempotently

Use a stable creation key for one operator intent. The first request returns
`201`; an exact replay returns the same execution with `200`. Reusing the key
for a different body returns `409`.

```bash
export CREATE_KEY="dlg-qualification-HIPASSJ1318-21-$(date -u +%Y%m%dT%H%M%SZ)"

CREATE_CODE=$(curl -sS -o /tmp/beampipe-created.json -w '%{http_code}' \
  -X POST "$BASE/api/v2/executions" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -H "Idempotency-Key: $CREATE_KEY" \
  -d @/tmp/beampipe-execution.json)
test "$CREATE_CODE" = 201
EXEC_ID=$(jq -er .uuid /tmp/beampipe-created.json)

REPLAY_CODE=$(curl -sS -o /tmp/beampipe-replayed.json -w '%{http_code}' \
  -X POST "$BASE/api/v2/executions" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -H "Idempotency-Key: $CREATE_KEY" \
  -d @/tmp/beampipe-execution.json)
test "$REPLAY_CODE" = 200
test "$EXEC_ID" = "$(jq -er .uuid /tmp/beampipe-replayed.json)"
```

The no-download graph does not stage CASDA files, but it does translate and
submit the graph to DALiuGE:

```bash
START_ONE=$(curl -fsS -X POST "$BASE/api/v2/executions/$EXEC_ID/execute" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"do_stage":false,"do_submit":true}')
START_TWO=$(curl -fsS -X POST "$BASE/api/v2/executions/$EXEC_ID/execute" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"do_stage":false,"do_submit":true}')

jq . <<<"$START_ONE"
test "$(jq -er .job_id <<<"$START_ONE")" = \
  "$(jq -er .job_id <<<"$START_TWO")"
```

Execution start is intrinsically idempotent per execution. Exact queued,
running, and completed replays return the same job. A replay with different
`do_stage` or `do_submit` flags returns `409` rather than changing the original
intent.

## 7. Prove terminal state and durable evidence

```bash
for attempt in $(seq 1 120); do
  EXEC_STATUS=$(curl -fsS "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH")
  jq '{status,control_phase,submission_state,scheduler_state,daliuge_state,terminal_outcome,last_error}' \
    <<<"$EXEC_STATUS"
  case "$(jq -r .status <<<"$EXEC_STATUS")" in
    completed|failed|cancelled|not_submitted) break ;;
  esac
  sleep 2
done

test "$(jq -r .status <<<"$EXEC_STATUS")" = completed
test "$(jq -r .terminal_outcome <<<"$EXEC_STATUS")" = succeeded
test "$(jq -r .daliuge_state <<<"$EXEC_STATUS")" = finished
```

Read the full ledger and evidence surfaces:

```bash
curl -fsS "$BASE/api/v2/executions/$EXEC_ID" -H "$AUTH" \
  | jq '{status,control_phase,submission_state,scheduler_state,daliuge_state,
         output_verification_required,output_state,terminal_outcome,
         project_config_version,deployment_profile_revision,
         discovery_signature,last_error}'

curl -fsS "$BASE/api/v2/executions/$EXEC_ID/artifacts" -H "$AUTH" \
  | jq 'map({kind,sha256,size_bytes,producer_phase})'
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/events" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/observations" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/ledger-snapshot?include_manifest=false" \
  -H "$AUTH" | jq .
```

A successful REST run should read as:

- control terminal and submission submitted;
- scheduler not submitted, because REST DIM did not create a Slurm job;
- DALiuGE finished and terminal outcome succeeded;
- output verification **not required**, because this exact project revision
  explicitly sets `output_verification.required: false`;
- four immutable artifacts: `manifest`, `source_graph`, `patched_graph`, and
  `physical_graph`, each with a SHA-256 and size.

“Not required” is not the same as “verified”. The production WALLABY project
keeps output verification required and cannot reach terminal success until a
trusted publisher supplies the durable inventory evidence described in
[Output verification](../project-configs/output-verification.md).

## 8. Observe and troubleshoot

```bash
docker compose -f docker-compose.yml -f compose.dlg-local.yml logs \
  --tail 200 api scheduler worker
curl -fsS http://127.0.0.1:19090/metrics | \
  grep -E 'beampipe_(worker|jobs|queue|dependency)'

docker compose -f docker-compose.yml -f compose.dlg-local.yml \
  --profile observability up -d
curl -fsS http://127.0.0.1:9099/-/ready
```

| Symptom | Check |
|---|---|
| TM healthy, DIM deploy fails | Re-check `dlg-dim:8001` from TM, then the selected worker route: `dlg-dim.desk:80` in `.desk` mode or `dlg-dim:8001` in direct mode |
| `localhost` connection refused | Replace container-local loopback with a name reachable from the actual caller |
| Discovery does not finish | Source events, `discover_batch` job, worker capability, CASDA/VizieR health, and public TAP latency |
| Prepare returns blockers | Wait for the active discovery lease and require a non-empty discovery signature and metadata |
| Execution stays pending | Queue depth, active worker heartbeat, global shaping caps, and profile concurrency |
| DIM state appears stale | Compare observations and the direct DIM session before intervening; reconciliation is asynchronous |
| Output remains pending | The pinned project requires publication evidence; do not treat graph completion as output verification |
| Metrics bind reports “address in use” | On a current build, one scheduler process starts one listener. Check for a mixed/old binary or two host processes sharing `BEAMPIPE_METRICS_BIND_ADDR` |

In production, startup additionally requires a reachable Redis limiter through
`BEAMPIPE_REDIS_URL`; `BEAMPIPE_REQUIRE_RATE_LIMITER=false` cannot weaken that
rule. This development qualification does not prove Setonix/Slurm. The bundled
Slurm profile still requires a real account, qualified SSH slot, remote paths,
and `BEAMPIPE_ASKAPSOFT_SIF` before submission.

## 9. Back up or remove the qualification environment

If the ledger is evidence you need to retain, perform the
[backup and restore drill](../operations/recovery.md#postgresql-backup-and-restore-drills)
before cleanup.

The following removes only the Compose project named above, including its test
PostgreSQL and observability volumes. It does not stop DALiuGE:

```bash
test "$COMPOSE_PROJECT_NAME" = beampipe-qualification
docker compose -f docker-compose.yml -f compose.dlg-local.yml \
  down --volumes --remove-orphans
rm -f /tmp/beampipe-dlg-desk-profile.json \
  /tmp/beampipe-dlg-direct-profile.json \
  /tmp/beampipe-execution.json \
  /tmp/beampipe-preflight.json \
  /tmp/beampipe-created.json \
  /tmp/beampipe-replayed.json
```

Stop the separate DALiuGE development stack only when no other local workflow
uses it:

```bash
cd "$DALIUGE_ROOT"
make docker-stop
```

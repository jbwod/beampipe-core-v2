# API workflow

The Axum API is mounted at `/api/v2`. Health is public; operational resources require a bearer token, and mutating administrative surfaces require a superuser.

## Authenticate

Use the password setup printed once. It is not stored in `.env`.

```bash
export BASE=http://127.0.0.1:18080
export ADMIN_USER="${ADMIN_USER:-admin}"
export ADMIN_PASSWORD="${ADMIN_PASSWORD:?set to the password setup printed}"
export TOKEN=$(curl -fsS -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"${ADMIN_USER}\",\"password\":\"${ADMIN_PASSWORD}\"}" \
  | jq -er .access_token)
export AUTH="Authorization: Bearer $TOKEN"

curl -fsS "$BASE/api/v2/user/me" -H "$AUTH" | jq .
```

Access and refresh tokens carry `jti` claims. Refresh rotates the refresh token; logout blacklists token hashes. Public user responses never include password hashes.

## Resource order

<div class="bp-flow-diagram bp-flow-diagram--wide bp-flow-diagram--animated" role="img" aria-label="API resource order from project and profile through source discovery to execution evidence">
  <div class="bp-flow-node" data-tone="cyan"><span>CONFIG</span><strong>project + profile</strong><small>immutable policy</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="cyan"><span>SOURCE</span><strong>register</strong><small>stable identity</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>DISCOVERY</span><strong>mark + schedule</strong><small>metadata signature</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="green"><span>EXECUTION</span><strong>prepare + execute</strong><small>pinned artifacts</small></div>
  <span class="bp-flow-link" aria-hidden="true">--&gt;</span>
  <div class="bp-flow-node" data-tone="amber"><span>EVIDENCE</span><strong>status + events</strong><small>observations</small></div>
</div>

## Project and profile

```bash
curl -fsS -X POST "$BASE/api/v2/project-configs" \
  -H "$AUTH" -H 'Content-Type: application/x-yaml' \
  --data-binary @config/wallaby_hires.v2.yaml | jq .

curl -fsS -X POST "$BASE/api/v2/deployment-profiles" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d @profile.json | jq .
```

Project uploads create immutable versions. Profile responses are redacted and future executions pin the selected revision.

List installed Slurm SSH credential slots (names and file presence only; never key material) before choosing `deployment.ssh_credential`:

```bash
curl -fsS "$BASE/api/v2/slurm/credentials" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/slurm/credentials/hpc" -H "$AUTH" | jq .
```

Init, import, and `copy-id` remain CLI. Empty credential roots return `{ "slots": [] }`.

## Source and discovery

```bash
SOURCE=$(curl -fsS -X POST "$BASE/api/v2/sources" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires","source_identifier":"HIPASSJ1313-15","enabled":true}')
SOURCE_ID=$(jq -r .uuid <<<"$SOURCE")

curl -fsS -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires","source_identifier":"HIPASSJ1313-15"}' | jq .

curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/metadata" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/sources/$SOURCE_ID/events" -H "$AUTH" | jq .
```

`sources/discover` marks matching enabled sources for rediscovery. The scheduler and workers perform the durable claim/query/persistence path asynchronously.

## Prepare and execute

Use the same body for preflight and creation:

```bash
cat > /tmp/execution.json <<'JSON'
{
  "project_module": "wallaby_hires",
  "sources": [{"source_identifier": "HIPASSJ1313-15"}],
  "archive_name": "casda",
  "deployment_profile_name": "slurm-remote"
}
JSON

curl -fsS -X POST "$BASE/api/v2/executions/prepare" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d @/tmp/execution.json | jq .

EXEC=$(curl -fsS -X POST "$BASE/api/v2/executions" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: operator-intent-20260822-001' \
  -d @/tmp/execution.json)
EXEC_ID=$(jq -r .uuid <<<"$EXEC")

curl -fsS -X POST "$BASE/api/v2/executions/$EXEC_ID/execute" \
  -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"do_stage":false,"do_submit":false}' | jq .
```

Creation keys are scoped to the authenticated user and one request body. The
first request returns `201`, an exact retry returns the same execution with
`200`, and reuse with different content returns `409`. Persist the key before
sending the request so a lost response can be resumed safely.

Execution start is intrinsically idempotent for the execution UUID. Exact
queued, running, and completed retries return `202` with the same job ID;
different `do_stage`/`do_submit` flags return `409`. `do_submit:false` is a
preparation-only boundary. Use `do_submit:true` only after the pinned profile
doctor passes and real backends are deliberately enabled.

Inspect exact state instead of polling only the compact status:

```bash
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/ledger-snapshot" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/observations" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/artifacts" -H "$AUTH" | jq .
curl -fsS "$BASE/api/v2/executions/$EXEC_ID/events" -H "$AUTH" | jq .
```

## Contract

The [generated API schema](reference.md) is the field-level source of truth. Export it after Rust request/response changes:

```bash
beampipe openapi export > openapi.json
cp openapi.json boilerplate_docs/openapi.json
```

Swagger UI and JSON are also served by the running API at `/api/v2/docs` and `/api/v2/openapi.json` when documentation is enabled.

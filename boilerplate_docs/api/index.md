# API workflow guide

The Rust API is mounted at `/api/v2`. Use this page for workflow order; use [Redoc reference](reference.md) for request and response schemas.

## Auth

```bash
BASE=http://127.0.0.1:8080
TOKEN=$(curl -s -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"change-me"}' | jq -r .access_token)
AUTH="Authorization: Bearer $TOKEN"
```

## Project config

```bash
curl -s -X POST "$BASE/api/v2/project-configs" \
  -H "$AUTH" \
  -H 'Content-Type: application/x-yaml' \
  --data-binary @config/wallaby_hires.v1.yaml | jq .

curl -s "$BASE/api/v2/project-configs/wallaby_hires" -H "$AUTH" | jq .
curl -s "$BASE/api/v2/project-configs/wallaby_hires/versions" -H "$AUTH" | jq .
```

## Sources

```bash
SOURCE=$(curl -s -X POST "$BASE/api/v2/sources" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires","source_identifier":"HIPASSJ1313-15","enabled":true}')
SOURCE_ID=$(echo "$SOURCE" | jq -r .uuid)

curl -s "$BASE/api/v2/sources?project_module=wallaby_hires" -H "$AUTH" | jq .
curl -s -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires","source_identifiers":["HIPASSJ1313-15"]}' | jq .
curl -s "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH" | jq .
```

## Executions

```bash
EXEC=$(curl -s -X POST "$BASE/api/v2/executions" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{
    "project_module": "wallaby_hires",
    "sources": [{"source_identifier": "HIPASSJ1313-15"}],
    "archive_name": "casda",
    "deployment_profile_name": "slurm-remote"
  }')
EXEC_ID=$(echo "$EXEC" | jq -r .uuid)

curl -s -X POST "$BASE/api/v2/executions/$EXEC_ID/execute" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"do_stage":false,"do_submit":false}' | jq .

curl -s "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH" | jq .
curl -s "$BASE/api/v2/executions/$EXEC_ID/summary" -H "$AUTH" | jq .
curl -s "$BASE/api/v2/executions/$EXEC_ID/ledger-snapshot" -H "$AUTH" | jq .
```

## Deployment profiles

```bash
curl -s -X POST "$BASE/api/v2/deployment-profiles" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d @profile.json | jq .

curl -s "$BASE/api/v2/deployment-profiles?project_module=wallaby_hires" \
  -H "$AUTH" | jq .
```

## Observability

```bash
curl -s "$BASE/api/v2/health" | jq .
curl -s "$BASE/api/v2/ready" | jq .
curl -s "$BASE/api/v2/metrics"
curl -s "$BASE/api/v2/executions/$EXEC_ID/events" -H "$AUTH" | jq .
```

## OpenAPI

```bash
beampipe openapi export > openapi.json
```

The committed OpenAPI document is used by Redoc, Bruno, and external clients.

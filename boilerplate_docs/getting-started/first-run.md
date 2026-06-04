# First run

This flow registers one `wallaby_hires` source, triggers discovery, creates an execution, and queues it for a mock backend run. Use real backend variables only after the mock path is healthy.

## 1. Start the stack

```bash
docker compose build api
docker compose up -d postgres api scheduler worker
```

Set a base URL for the examples:

```bash
BASE=http://127.0.0.1:8080
```

## 2. Login

```bash
TOKEN=$(curl -s -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"change-me"}' | jq -r .access_token)
AUTH="Authorization: Bearer $TOKEN"
```

## 3. Upload or verify project config

Validate locally first:

```bash
beampipe project validate -f config/wallaby_hires.v1.yaml
```

Upload through the API:

```bash
curl -s -X POST "$BASE/api/v2/project-configs" \
  -H "$AUTH" \
  -H 'Content-Type: application/x-yaml' \
  --data-binary @config/wallaby_hires.v1.yaml | jq .
```

## 4. Register a source

```bash
SOURCE=$(curl -s -X POST "$BASE/api/v2/sources" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{
    "project_module": "wallaby_hires",
    "source_identifier": "HIPASSJ1313-15",
    "enabled": true
  }')
echo "$SOURCE" | jq .
SOURCE_ID=$(echo "$SOURCE" | jq -r .uuid)
```

## 5. Trigger discovery

```bash
curl -s -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires"}' | jq .
```

Poll readiness:

```bash
curl -s "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH" | jq .
```

## 6. Create and execute a run

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
echo "$EXEC" | jq .
EXEC_ID=$(echo "$EXEC" | jq -r .uuid)
```

Queue the execution. Keep `do_stage` and `do_submit` false until real CASDA, TM, DIM, and Slurm access are configured.

```bash
curl -s -X POST "$BASE/api/v2/executions/$EXEC_ID/execute" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"do_stage":false,"do_submit":false}' | jq .
```

## 7. Inspect state

```bash
curl -s "$BASE/api/v2/executions/$EXEC_ID/status" -H "$AUTH" | jq .
curl -s "$BASE/api/v2/executions/$EXEC_ID/summary" -H "$AUTH" | jq .
curl -s "$BASE/api/v2/executions/$EXEC_ID/ledger-snapshot" -H "$AUTH" | jq .
```

The worker records provenance events on sources, executions, and project modules. See [Observability](../operations/observability.md) for metrics and event queries.

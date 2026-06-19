# First run

This workflow proves the control plane path with one `wallaby_hires` source: authenticate, upload config, register a source, run discovery, create an execution, and queue a dry backend run. Keep `do_stage` and `do_submit` disabled until real CASDA, Translator Manager, DIM, or Slurm access is configured.

## 1. Start services

For Compose:

```bash
docker compose build api
docker compose up -d postgres api scheduler worker
docker compose run --rm api migrate
```

For a host binary, start the API and at least one worker as described in [Installation](installation.md). The examples below assume:

```bash
BASE=http://127.0.0.1:8080
```

## 2. Login

Create an admin user first if you have not already done so.

```bash
TOKEN=$(curl -s -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"change-me"}' | jq -r .access_token)
AUTH="Authorization: Bearer $TOKEN"
```

## 3. Validate and upload project config

Validate locally before API upload:

```bash
beampipe project validate -f config/wallaby_hires.v2.yaml
```

Upload the YAML:

```bash
curl -s -X POST "$BASE/api/v2/project-configs" \
  -H "$AUTH" \
  -H 'Content-Type: application/x-yaml' \
  --data-binary @config/wallaby_hires.v2.yaml | jq .
```

Confirm the active config:

```bash
curl -s "$BASE/api/v2/project-configs/wallaby_hires" -H "$AUTH" | jq .
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
SOURCE_ID=$(echo "$SOURCE" | jq -r .uuid)
echo "$SOURCE" | jq .
```

## 5. Trigger discovery

```bash
curl -s -X POST "$BASE/api/v2/sources/discover" \
  -H "$AUTH" \
  -H 'Content-Type: application/json' \
  -d '{"project_module":"wallaby_hires","source_identifiers":["HIPASSJ1313-15"]}' | jq .
```

Poll source status until metadata and discovery flags are present:

```bash
curl -s "$BASE/api/v2/sources/$SOURCE_ID/status" -H "$AUTH" | jq .
curl -s "$BASE/api/v2/sources/$SOURCE_ID/events" -H "$AUTH" | jq .
```

## 6. Create and queue an execution

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
echo "$EXEC" | jq .
```

Queue a dry run:

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
curl -s "$BASE/api/v2/executions/$EXEC_ID/events" -H "$AUTH" | jq .
```

Next: review [Configuration](configuration.md) for environment variables, then choose a backend in [Deployment profiles](../architecture/deployment-profiles.md).

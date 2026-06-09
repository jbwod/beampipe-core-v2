# Bruno collection

The Bruno collection is the operator-friendly API scratchpad. It should mirror the workflow in [API workflow guide](../api/index.md) and the schema in [Redoc reference](../api/reference.md).

## Environment

Recommended environment variables:

| Name | Example |
|------|---------|
| `BASE` | `http://127.0.0.1:8080` |
| `USERNAME` | `admin` |
| `PASSWORD` | `change-me` |
| `TOKEN` | set after login |

## Recommended order

1. Login.
2. Upload or fetch project config.
3. Register source or bulk source list.
4. Trigger discovery.
5. Inspect source status and source events.
6. Create or select deployment profile.
7. Create execution.
8. Queue execution.
9. Inspect execution status, summary, ledger snapshot, and events.

## curl equivalent

```bash
BASE=http://127.0.0.1:8080
TOKEN=$(curl -s -X POST "$BASE/api/v2/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"change-me"}' | jq -r .access_token)
AUTH="Authorization: Bearer $TOKEN"
```

## Keep it current

```bash
beampipe openapi export > openapi.json
cp openapi.json boilerplate_docs/openapi.json
```

Update Bruno request bodies when Redoc schema changes, especially project config upload, deployment profiles, execution creation, and dry-run flags.

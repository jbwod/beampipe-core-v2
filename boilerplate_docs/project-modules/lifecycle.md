# How project modules work

Project modules are **Python packages** that teach beampipe-core how to talk to a specific survey: which archives to query, how to shape metadata, and how to build <img class="bp-brand-icon bp-daliuge" src="../assets/daliuge.png" alt="DALiuGE"> manifests. Core stays survey-agnostic. All survey logic lives in the plugin.

## Registration

Modules register via setuptools entry points in `pyproject.toml`:

```toml
[project.entry-points."beampipe.projects"]
my_survey = "my_survey.module"
```

At runtime, core calls `importlib.metadata.entry_points(group="beampipe.projects")`, loads the module, and validates required hooks and constants.

Verify installation:

```bash
curl -s http://127.0.0.1:8000/api/v1/projects | jq .
curl -s http://127.0.0.1:8000/api/v1/projects/contracts/my_survey | jq .
```

## Lifecycle

`discover` ⮕ `prepare_metadata` ⮕ (execution) ⮕ `manifest` ⮕ translate ⮕ submit

### 1. Discovery (`discover`)

Triggered by:

- `POST /api/v1/sources/discover` (operator)
- ARQ scheduler cron (`DISCOVERY_SCHEDULE_MINUTES`)
- Restate `DiscoveryBatchWorkflow` (when `WORKFLOW_ENGINE_DISCOVERY=restate`)

Core loads adapters declared in `REQUIRED_ADAPTERS` (e.g. `casda`, `vizier`) and calls:

```python
def discover(source_identifier: str, adapters: dict | None = None) -> DiscoverBundle:
    ...
```

Returns `query_results` (+ optional `enrichments`): raw archive rows, not yet normalized for the ledger.

### 2. Metadata preparation (`prepare_metadata`)

Core passes discovery output to:

```python
def prepare_metadata(
    source_identifier: str,
    query_results: DiscoverBundle,
    **kwargs,
) -> tuple[list[dict], dict[str, bool]]:
    ...
```

Output rows are persisted on the **source record** in PostgreSQL.

### 3. Execution create

Operator (or automation) calls `POST /api/v1/executions` with `source_identifiers` and optional `deployment_profile_name`. Core validates sources are registered and enabled, then creates a **ledger row** in `pending` status.

### 4. Manifest (`manifest`)

On `POST /api/v1/executions/{id}/execute`, the worker:

1. **Stages** archive files to compute-accessible storage
2. Calls the module's **`manifest()`** with staged URLs and metadata
3. Stores manifest JSON on the execution record

```python
def manifest(
    metadata_by_source: dict[str, list[dict]],
    *,
    staged_urls_by_scan_id: dict[str, str],
    ...
) -> list[dict]:
    ...
```

### 5. Translate + submit

Core resolves the <img class="bp-brand-icon bp-daliuge" src="../assets/daliuge.png" alt="DALiuGE"> graph from the module's `GRAPH_GITHUB_URL` / `GRAPH_PATH`, merges manifest config, and submits using the execution's [deployment profile](../deployment-profiles/index.md):

- **`rest_remote`:** Translator ⮕ DIM REST deploy
- **`slurm_remote`:** Translator ⮕ SSH ⮕ `create_dlg_job` on Slurm

Optional hooks `graph_overrides_from_sources()` and `apply_graph_translate_overrides()` let modules patch graph JSON before translation.

## Required module surface

| Symbol | Purpose |
|--------|---------|
| `PROJECT_NAME` | Stable module id (matches entry point name) |
| `REQUIRED_ADAPTERS` | Adapter names core must inject into `discover()` |
| `discover()` | Query archives, return `query_results` (+ optional `enrichments`) |
| `prepare_metadata()` | Normalize discovery output for the ledger |
| `manifest()` | Build <img class="bp-brand-icon bp-daliuge" src="../assets/daliuge.png" alt="DALiuGE"> manifest items for staged data |

Optional: `GRAPH_GITHUB_URL`, `GRAPH_PATH`, automation dicts, graph override hooks.

## What core provides to modules

| Core facility | Module uses it for… |
|---------------|----------------------|
| **`beampipe.adapters`** | TAP queries inside `discover()` |
| **Deployment profiles** | TM URL, REST DIM hosts, or Slurm SSH targets |
| **Shaping / admission** | Rate limits on discovery batches and in-flight executions |
| **Automation constants** | `WORKFLOW_DISCOVERY_AUTOMATION`, `WORKFLOW_EXECUTION_AUTOMATION` |

## Authoring your own module

See [Authoring a module](authoring.md) for package layout, entry points, and testing with `python -m app.core.projects.test_load`.

## API reference

| Endpoint | Purpose |
|----------|---------|
| `GET /api/v1/projects` | Installed module names |
| `GET /api/v1/projects/contracts` | Contract validation for all |
| `GET /api/v1/projects/contracts/{name}` | One module. Returns 404 if not installed |

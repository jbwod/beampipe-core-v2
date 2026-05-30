# Authoring a project module

Project modules are Python packages that register a `beampipe.projects` entry point. They plug survey-specific discovery, metadata preparation, and manifest shaping into the runtime.

## 1. Implement hooks

```python
PROJECT_NAME = "my_survey"
REQUIRED_ADAPTERS = ["casda"]


def discover(source_identifier, adapters=None):
    return {"query_results": []}


def prepare_metadata(source_identifier, query_results, **kwargs):
    return [], {}


def manifest(
    metadata_by_source,
    *,
    staged_urls_by_scan_id,
    eval_urls_by_sbid,
    checksum_urls_by_scan_id,
    eval_checksum_urls_by_sbid,
):
    return []
```

See [How it works](lifecycle.md) for hook signatures and return shapes.

## 2. Register the entry point

In your package `pyproject.toml`:

```toml
[project.entry-points."beampipe.projects"]
my_survey = "my_survey.module"
```

## 3. Install alongside core

Development:

```bash
uv pip install -e ../my-survey-package
```

Production: install the wheel on worker and web images.

## 4. Validate

```bash
curl -s http://127.0.0.1:8000/api/v1/projects/contracts/my_survey | jq .
```

## 5. Use in API calls

Set `"project_module": "my_survey"` on source registration and execution create requests.

!!! tip
    Run `GET /api/v1/projects` before registering sources to confirm the module is loaded.

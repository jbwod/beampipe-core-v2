# Project modules

Survey-specific logic lives in **separate Python packages** loaded at runtime via the `beampipe.projects` entry point group. beampipe-core provides the control plane (registry, ledger, orchestration, adapters) and calls into your module for archive queries and manifest shaping.

!!! tip "Start here"
    Read [How it works](lifecycle.md) for the full discover ⮕ metadata ⮕ manifest ⮕ execute flow.

## Why plugins?

Radio astronomy surveys differ in archive queries, metadata fields, and <img class="bp-brand-icon bp-daliuge" src="../assets/daliuge.png" alt="DALiuGE"> graph inputs. Survey logic belongs in a plugin, not in core:

- **Core:** generic REST API, PostgreSQL ledger, ARQ/Restate workers, archive adapters
- **Project module:** survey hooks, automation policy, graph reference
- **Science graphs:** <img class="bp-brand-icon bp-daliuge" src="../assets/daliuge.png" alt="DALiuGE"> `.graph` files maintained in a companion repository

## Entry point contract

Every module must export three hooks (see [How it works](lifecycle.md)):

| Hook | When core calls it |
|------|-------------------|
| `discover(source_identifier, adapters)` | Archive polling / on-demand rediscovery |
| `prepare_metadata(source_identifier, query_results, **kwargs)` | After discovery, persist metadata rows |
| `manifest(metadata_by_source, **kwargs)` | During execute, build <img class="bp-brand-icon bp-daliuge" src="../assets/daliuge.png" alt="DALiuGE"> input items |

Optional: `graph_overrides_from_sources`, `apply_graph_translate_overrides`, automation dicts, `GRAPH_GITHUB_URL`.

## Built-in archive adapters

Core ships adapters under `beampipe.adapters`:

| Name | Archive |
|------|---------|
| `casda` | [CASDA](https://casda.csiro.au/) TAP / staging |
| `vizier` | [VizieR](https://vizier.cds.unistra.fr/) catalog queries |

Modules declare requirements via `REQUIRED_ADAPTERS = ["casda", "vizier"]` (or a subset).

## Install a module

Modules are **not vendored** in core. Install alongside the app image or dev venv:

```bash
# Example: optional extra that installs the reference module package
uv sync --extra wallaby

# Or install your survey package directly
pip install -e ./path-to-your-module
```

Verify:

```bash
python -m app.core.projects.test_load
curl -s http://127.0.0.1:8000/api/v1/projects/contracts/wallaby_hires | jq .
```

## Automation policy

Modules may ship defaults consumed by schedulers:

```python
WORKFLOW_DISCOVERY_AUTOMATION = {
    "enabled": True,
    "archive": "casda",
    "batch_size": 10,
    ...
}
WORKFLOW_EXECUTION_AUTOMATION = {
    "enabled": True,
    "deployment_profile_name": "hpc-slurm-remote",
    ...
}
```

Global caps still apply via `.env` shaping variables (`SHAPING_*`).

## Further reading

- [How it works](lifecycle.md) (end-to-end lifecycle)
- [Authoring a module](authoring.md) (build your own survey package)

from types import ModuleType
from typing import Any, Protocol, TypedDict, cast

REQUIRED_HOOK_NAMES: tuple[str, ...] = (
    "discover",
    "prepare_metadata",
    "manifest",
)

OPTIONAL_HOOK_NAMES: tuple[str, ...] = (
    "graph_overrides_from_sources",
    "apply_graph_translate_overrides",
    "ping",
)

KNOWN_HOOK_NAMES: tuple[str, ...] = REQUIRED_HOOK_NAMES + OPTIONAL_HOOK_NAMES

OPTIONAL_CONSTANT_NAMES: tuple[str, ...] = (
    "PROJECT_NAME",
    "REQUIRED_ADAPTERS",
    "DISCOVERY_ENRICHMENT_KEYS",
    "GRAPH_PATH",
    "GRAPH_GITHUB_URL",
    "WORKFLOW_EXECUTION_AUTOMATION",
    "WORKFLOW_DISCOVERY_AUTOMATION",
)

# MANIFEST_SCHEMA
# GRAPH_PATH
# GRAPH_GITHUB_URL
# WORKFLOW_EXECUTION_AUTOMATION [Optional]: project-specific execution scheduling and retry policy.
# WORKFLOW_DISCOVERY_AUTOMATION [Optional]: project-specific discovery scheduling policy.

class DiscoverBundle(TypedDict, total=False):
    query_results: Any
    enrichments: dict[str, Any]


def validate_project_module_interface(module: ModuleType, module_name: str) -> None:
    for hook_name in REQUIRED_HOOK_NAMES:
        if not callable(getattr(module, hook_name, None)):
            raise ValueError(
                f"module '{module_name}' must implement {hook_name}(...) — "
                "see app.core.projects.contracts for the expected signature"
            )


def known_exports(module: ModuleType) -> list[str]:
    return [name for name in KNOWN_HOOK_NAMES if hasattr(module, name)]


def extract_discover_bundle(discover_output: Any, module_name: str) -> DiscoverBundle:
    """Validate and return expectted discover output."""
    if not isinstance(discover_output, dict):
        raise ValueError(
            "Project module "
            f"'{module_name}' discover() must return dict bundle with required key: query_results"
        )
    required_keys = {"query_results"}
    missing = sorted(required_keys.difference(discover_output.keys()))
    if missing:
        raise ValueError(
            f"Project module '{module_name}' discover() missing bundle keys: {missing}"
        )
    enrichments = discover_output.get("enrichments")
    if enrichments is not None and not isinstance(enrichments, dict):
        raise ValueError(
            f"Project module '{module_name}' discover() key 'enrichments' must be a dict when provided"
        )
    return cast(DiscoverBundle, discover_output)

# """
# {
#     "enrichments": {
#         "key": "value",
#         "sbid_to_eval_file": "wallaby_eval_file.txt",
#     }
# }
# sit it in the module when validating the discovery outputs
# """
def get_discover_enrichment(
    bundle: DiscoverBundle,
    key: str,
    *,
    default: Any = None,
    expected_type: type[Any] | tuple[type[Any], ...] | None = None,
    module_name: str | None = None,
) -> Any:
    """Read discover enrichments"""
    enrichments = bundle.get("enrichments")
    if enrichments is None:
        return default
    value = enrichments.get(key, default)
    if expected_type is not None and value is not None and not isinstance(value, expected_type):
        module_label = module_name or "unknown"
        expected_label = (
            "|".join(t.__name__ for t in expected_type)
            if isinstance(expected_type, tuple)
            else expected_type.__name__
        )
        raise ValueError(
            f"module '{module_label}' discover() enrichment '{key}' must be {expected_label}"
        )
    return value


__all__ = [
    "KNOWN_HOOK_NAMES",
    "OPTIONAL_CONSTANT_NAMES",
    "OPTIONAL_HOOK_NAMES",
    "REQUIRED_HOOK_NAMES",
    "DiscoverBundle",
    "extract_discover_bundle",
    "get_discover_enrichment",
    "known_exports",
    "validate_project_module_interface",
]

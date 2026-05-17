"""Tests for project module contracts and discovery helpers."""
from types import ModuleType

import pytest

from app.core.projects import list_project_modules, load_project_module
from app.core.projects.contracts import (
    KNOWN_HOOK_NAMES,
    REQUIRED_HOOK_NAMES,
    extract_discover_bundle,
    get_discover_enrichment,
    known_exports,
    validate_project_module_interface,
)
from app.core.utils import validate_prepared_metadata_records


def _make_valid_module(name: str = "m") -> ModuleType:
    module = ModuleType(name)
    module.discover = lambda sid, adapters=None: {"query_results": []}
    module.prepare_metadata = lambda sid, qr, **kw: ([], {})
    module.manifest = lambda metadata_by_source, **kw: []
    module.REQUIRED_ADAPTERS = ["casda"]
    return module


def test_validate_project_module_interface_accepts_valid_module():
    validate_project_module_interface(_make_valid_module(), "m")


@pytest.mark.parametrize("missing", REQUIRED_HOOK_NAMES)
def test_validate_project_module_interface_rejects_missing_required(missing: str):
    module = _make_valid_module()
    delattr(module, missing)
    with pytest.raises(ValueError, match=missing):
        validate_project_module_interface(module, "m")


def test_known_exports_returns_only_defined_hooks():
    module = _make_valid_module()
    exports = known_exports(module)
    for required in REQUIRED_HOOK_NAMES:
        assert required in exports
    for name in exports:
        assert name in KNOWN_HOOK_NAMES


def test_extract_discover_bundle():
    with pytest.raises(ValueError, match="missing bundle keys"):
        extract_discover_bundle({"enrichments": {}}, "p")
    b = extract_discover_bundle({"query_results": []}, "p")
    assert b["query_results"] == []


def test_validate_prepared_metadata_records():
    with pytest.raises(ValueError, match="non-null 'sbid'"):
        validate_prepared_metadata_records(
            [{"dataset_id": "d1"}],
            project_module="p",
            source_identifier="s",
        )
    with pytest.raises(ValueError, match="dataset_id.*visibility_filename"):
        validate_prepared_metadata_records(
            [{"sbid": "1"}],
            project_module="p",
            source_identifier="s",
        )


def test_get_discover_enrichment():
    bundle = {"query_results": [], "enrichments": {"k": {"x": 1}}}
    assert get_discover_enrichment(bundle, "k", expected_type=dict, module_name="p") == {"x": 1}
    assert get_discover_enrichment(bundle, "missing", default=[]) == []
    with pytest.raises(ValueError, match="must be dict"):
        get_discover_enrichment(
            {"query_results": [], "enrichments": {"k": "not-dict"}},
            "k",
            expected_type=dict,
            module_name="p",
        )


def test_installed_project_modules_conform():
    modules = list_project_modules()
    if not modules:
        pytest.skip("No beampipe.projects entry points")
    for name in modules:
        validate_project_module_interface(load_project_module(name), name)

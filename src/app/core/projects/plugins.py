"""Entry-point discovery and loading for beampipe.projects."""

import logging
from functools import lru_cache
from importlib.metadata import entry_points
from types import ModuleType
from typing import cast

from .contracts import validate_project_module_interface

logger = logging.getLogger(__name__)

_ENTRY_POINT_GROUP = "beampipe.projects"


def _entry_points_for(group: str):
    eps = entry_points()
    if hasattr(eps, "select"):
        return eps.select(group=group)
    return eps.get(group, [])


def list_project_modules() -> list[str]:
    return [ep.name for ep in _entry_points_for(_ENTRY_POINT_GROUP)]


@lru_cache(maxsize=64)
def load_project_module(name: str) -> ModuleType:
    for ep in _entry_points_for(_ENTRY_POINT_GROUP):
        if ep.name == name:
            try:
                module = ep.load()
            except Exception as exc:
                logger.warning(
                    "event=project_module_load_failed module=%s reason=import_error error=%s",
                    name,
                    exc,
                )
                raise
            try:
                validate_project_module_interface(module, name)
            except ValueError as exc:
                logger.warning(
                    "event=project_module_load_failed module=%s reason=contract error=%s",
                    name,
                    exc,
                )
                raise
            return cast(ModuleType, module)
    available = list_project_modules()
    logger.warning(
        "event=project_module_load_failed module=%s reason=not_registered available=%s",
        name,
        available,
    )
    raise ValueError(
        f"Project module '{name}' not found. Available: {available}"
    )

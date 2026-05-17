"""Project modules (plugins) for domain-specific logic.

Survey-specific implementations, try to get this like a module system

Handles WALLABY-specific workflow generation and processing.
# - WALLABY dataset validation
# - DALiuGE workflow manifest generation
# - ASKAPsoft pipeline configuration
# - Result processing and validation
# https://github.com/ICRAR/wallaby-hires/blob/main/

so thinking perhaps an entry point?
    [project.entry-points."beampipe.projects"]
    wallaby_hires = "wallaby_hires.module"
"""

from .plugins import list_project_modules, load_project_module
from .service import (
    get_graph_github_url,
    get_graph_path,
    get_workflow_discovery_automation_policy,
    get_workflow_execution_automation_policy,
    resolve_graph_content,
    resolve_workflow_discovery_step_overrides,
    resolve_workflow_execute_step_overrides,
)

__all__ = [
    "get_graph_github_url",
    "get_graph_path",
    "get_workflow_discovery_automation_policy",
    "get_workflow_execution_automation_policy",
    "list_project_modules",
    "load_project_module",
    "resolve_graph_content",
    "resolve_workflow_discovery_step_overrides",
    "resolve_workflow_execute_step_overrides",
]

from fastapi import APIRouter, HTTPException, Request, status

from ...core.openapi import RESPONSES_NOT_FOUND, merge_responses
from ...core.projects.service import project_module_service
from ...core.registry.service import invalid_project_module_message
from ...schemas.projects import (
    ProjectModuleContractListResponse,
    ProjectModuleContractStatus,
    ProjectModuleListResponse,
)

router = APIRouter(prefix="/projects", tags=["projects"])


@router.get(
    "",
    response_model=ProjectModuleListResponse,
    summary="List project modules",
    responses={
        status.HTTP_200_OK: {
            "model": ProjectModuleListResponse,
            "description": "Registered project module names",
        },
    },
)
async def list_projects(request: Request) -> dict[str, object]:
    """List registered project module names (from beampipe.projects entry points)."""
    _ = request
    projects = project_module_service.list_project_names()
    return {"projects": projects}


@router.get(
    "/contracts",
    response_model=ProjectModuleContractListResponse,
    summary="List project contracts",
    responses={
        status.HTTP_200_OK: {
            "model": ProjectModuleContractListResponse,
            "description": "Discovery contract validation status for all modules",
        },
    },
)
async def list_project_contracts(request: Request) -> dict[str, object]:
    """Return discovery contract validation status for every registered project module."""
    _ = request
    statuses = project_module_service.list_contract_statuses()
    return {"count": len(statuses), "modules": statuses}


@router.get(
    "/contracts/{project_module}",
    response_model=ProjectModuleContractStatus,
    summary="Get project contract",
    responses=merge_responses(
        {
            status.HTTP_200_OK: {
                "model": ProjectModuleContractStatus,
                "description": "Contract validation status for one module",
            },
        },
        RESPONSES_NOT_FOUND,
    ),
)
async def get_project_module_contract(
    request: Request,
    project_module: str,
) -> dict[str, object]:
    """Return discovery contract validation status for a single project module."""
    _ = request
    if not project_module_service.project_exists(project_module):
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=invalid_project_module_message(project_module),
        )
    return project_module_service.get_contract_status(project_module)

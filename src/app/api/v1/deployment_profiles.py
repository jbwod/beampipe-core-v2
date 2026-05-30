from typing import Annotated, Any
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from fastcrud import PaginatedListResponse, compute_offset, paginated_response
from pydantic import ValidationError
from sqlalchemy.ext.asyncio import AsyncSession
from starlette.responses import Response

from ...api.dependencies import get_current_user
from ...core.db.database import async_get_db
from ...core.exceptions.http_exceptions import NotFoundException
from ...core.openapi import (
    RESPONSES_NOT_FOUND,
    RESPONSES_UNPROCESSABLE,
    authenticated_responses,
    merge_responses,
)
from ...crud.crud_daliuge_deployment_profile import crud_daliuge_deployment_profile
from ...schemas.daliuge import (
    DaliugeDeploymentProfileCreate,
    DaliugeDeploymentProfileRead,
    DaliugeDeploymentProfileStored,
    DaliugeDeploymentProfileUpdate,
    expand_update_with_nested_optional,
    merge_deployment_profile_state,
)

router = APIRouter(prefix="/deployment-profiles", tags=["deployment-profiles"])


def _to_read_dict(row: dict[str, Any]) -> dict[str, Any]:
    return DaliugeDeploymentProfileRead.from_stored_dict(row).model_dump()


@router.get(
    "",
    response_model=PaginatedListResponse[DaliugeDeploymentProfileRead],
    summary="List deployment profiles",
    responses=authenticated_responses(),
)
async def list_deployment_profiles(
    request: Request,
    db: Annotated[AsyncSession, Depends(async_get_db)],
    current_user: Annotated[dict, Depends(get_current_user)],
    page: Annotated[int, Query(ge=1, description="Page number (1-based)")] = 1,
    items_per_page: Annotated[int, Query(ge=1, le=100, description="Items per page")] = 10,
    project_module: Annotated[
        str | None, Query(description="Filter by project module; omit for all")
    ] = None,
    global_only: Annotated[
        bool, Query(description="When true, return only global (non-project) profiles")
    ] = False,
) -> dict[str, Any]:
    """List DALiuGE deployment profiles. Filter by project_module or global_only."""
    filters: dict[str, Any] = {}
    if project_module is not None:
        filters["project_module"] = project_module
    if global_only:
        filters["project_module"] = None
    data = await crud_daliuge_deployment_profile.get_multi(
        db=db,
        offset=compute_offset(page, items_per_page),
        limit=items_per_page,
        schema_to_select=DaliugeDeploymentProfileStored,
        return_total_count=True,
        **filters,
    )
    response: dict[str, Any] = paginated_response(
        crud_data=data, page=page, items_per_page=items_per_page
    )
    raw_items = response.get("data") or []
    response["data"] = [_to_read_dict(row) for row in raw_items]
    return response


@router.post(
    "",
    response_model=DaliugeDeploymentProfileRead,
    status_code=status.HTTP_201_CREATED,
    summary="Create deployment profile",
    responses=authenticated_responses(),
)
async def create_deployment_profile(
    request: Request,
    body: DaliugeDeploymentProfileCreate,
    db: Annotated[AsyncSession, Depends(async_get_db)],
    current_user: Annotated[dict, Depends(get_current_user)],
) -> dict[str, Any]:
    """Create a DALiuGE deployment profile with translation and deployment configuration."""
    profile = await crud_daliuge_deployment_profile.create(
        db=db,
        object=body.to_db_create(),
        schema_to_select=DaliugeDeploymentProfileStored,
    )
    return _to_read_dict(profile)


@router.get(
    "/{profile_id}",
    response_model=DaliugeDeploymentProfileRead,
    summary="Get deployment profile",
    responses=authenticated_responses(RESPONSES_NOT_FOUND),
)
async def get_deployment_profile(
    request: Request,
    profile_id: UUID,
    db: Annotated[AsyncSession, Depends(async_get_db)],
    current_user: Annotated[dict, Depends(get_current_user)],
) -> dict[str, Any]:
    """Return a deployment profile by UUID."""
    profile = await crud_daliuge_deployment_profile.get(
        db=db,
        uuid=profile_id,
        schema_to_select=DaliugeDeploymentProfileStored,
    )
    if profile is None:
        raise NotFoundException(f"Deployment profile {profile_id} not found")
    return _to_read_dict(profile)


@router.patch(
    "/{profile_id}",
    response_model=DaliugeDeploymentProfileRead,
    summary="Update deployment profile",
    responses=authenticated_responses(
        merge_responses(RESPONSES_NOT_FOUND, RESPONSES_UNPROCESSABLE),
    ),
)
async def update_deployment_profile(
    request: Request,
    profile_id: UUID,
    body: DaliugeDeploymentProfileUpdate,
    db: Annotated[AsyncSession, Depends(async_get_db)],
    current_user: Annotated[dict, Depends(get_current_user)],
) -> dict[str, Any]:
    """Partially update a deployment profile. Nested translation/deployment fields are merged."""
    current = await crud_daliuge_deployment_profile.get(
        db=db,
        uuid=profile_id,
        schema_to_select=DaliugeDeploymentProfileStored,
    )
    if current is None:
        raise NotFoundException(f"Deployment profile {profile_id} not found")

    if not body.model_dump(exclude_unset=True):
        return _to_read_dict(current)

    nested_patch = expand_update_with_nested_optional(current, body)
    merged = merge_deployment_profile_state(current, nested_patch)
    try:
        validated = DaliugeDeploymentProfileCreate.model_validate(merged)
    except ValidationError as e:
        raise HTTPException(status_code=422, detail=e.errors()) from e

    await crud_daliuge_deployment_profile.update(
        db=db,
        object=validated.to_db_create().model_dump(),
        uuid=profile_id,
    )
    updated = await crud_daliuge_deployment_profile.get(
        db=db,
        uuid=profile_id,
        schema_to_select=DaliugeDeploymentProfileStored,
    )
    if updated is None:
        raise NotFoundException(f"Deployment profile {profile_id} not found")
    return _to_read_dict(updated)


@router.delete(
    "/{profile_id}",
    status_code=status.HTTP_204_NO_CONTENT,
    summary="Delete deployment profile",
    responses=authenticated_responses(RESPONSES_NOT_FOUND),
)
async def delete_deployment_profile(
    request: Request,
    profile_id: UUID,
    db: Annotated[AsyncSession, Depends(async_get_db)],
    current_user: Annotated[dict, Depends(get_current_user)],
) -> Response:
    """Permanently delete a deployment profile."""
    existing = await crud_daliuge_deployment_profile.get(
        db=db,
        uuid=profile_id,
        schema_to_select=DaliugeDeploymentProfileStored,
    )
    if existing is None:
        raise NotFoundException(f"Deployment profile {profile_id} not found")
    await crud_daliuge_deployment_profile.delete(db=db, uuid=profile_id)
    return Response(status_code=status.HTTP_204_NO_CONTENT)

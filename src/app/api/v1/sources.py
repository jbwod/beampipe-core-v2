import logging
from enum import StrEnum
from typing import Annotated, Any, Literal
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from fastapi.encoders import jsonable_encoder
from fastapi.responses import JSONResponse, Response
from fastcrud import PaginatedListResponse, compute_offset, paginated_response
from sqlalchemy.ext.asyncio import AsyncSession

from ...api.dependencies import get_current_user
from ...core.archive.service import archive_metadata_service
from ...core.db.database import async_get_db
from ...core.exceptions.http_exceptions import NotFoundException
from ...core.ledger.service import execution_ledger_service
from ...core.openapi import (
    RESPONSES_BAD_REQUEST,
    RESPONSES_NOT_FOUND,
    RESPONSES_SERVER_ERROR,
    RESPONSES_SERVICE_UNAVAILABLE,
    authenticated_responses,
    merge_responses,
)
from ...core.projects import list_project_modules
from ...core.registry.service import invalid_project_module_message, source_registry_service
from ...crud.crud_source_registry import crud_source_registry
from ...schemas.registry import (
    DiscoverTriggerRequest,
    DiscoverTriggerResponse,
    SourceLinkedExecutionItem,
    SourceMetadataResponse,
    SourceRegistryBulkCreate,
    SourceRegistryBulkCreateResponse,
    SourceRegistryCreate,
    SourceRegistryRead,
    SourceRegistryUpdate,
)

class SourceSortField(StrEnum):
    created_at = "created_at"
    updated_at = "updated_at"
    source_identifier = "source_identifier"
    last_checked_at = "last_checked_at"

router = APIRouter(prefix="/sources", tags=["sources"])
logger = logging.getLogger(__name__)


def _json_response(data: dict[str, Any], *, status_code: int) -> JSONResponse:
    return JSONResponse(content=jsonable_encoder(data), status_code=status_code)


def _ensure_valid_project_module(project_module: str) -> None:
    available_modules = list_project_modules()
    if project_module not in available_modules:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=invalid_project_module_message(project_module, available_modules),
        )


@router.get(
    "",
    response_model=PaginatedListResponse[SourceRegistryRead],
    summary="List sources",
    responses=merge_responses(
        {
            status.HTTP_200_OK: {
                "description": "Paginated source registry entries",
            },
        },
    ),
)
async def list_sources(
    request: Request,
    db: Annotated[AsyncSession, Depends(async_get_db)],
    project_module: Annotated[
        str | None, Query(description="Filter by project module identifier")
    ] = None,
    enabled: Annotated[bool | None, Query(description="Filter by enabled flag")] = None,
    page: Annotated[int, Query(ge=1, description="Page number (1-based)")] = 1,
    items_per_page: Annotated[int, Query(ge=1, le=100, description="Items per page")] = 10,
    sort_by: Annotated[SourceSortField, Query(description="Column to sort by")] = SourceSortField.created_at,
    order: Annotated[Literal["asc", "desc"], Query(description="Sort direction")] = "desc",
) -> dict[str, Any]:
    """List registered sources with optional filters and pagination."""
    filters: dict[str, Any] = {}
    if project_module:
        filters["project_module"] = project_module
    if enabled is not None:
        filters["enabled"] = enabled

    sources_data = await crud_source_registry.get_multi(
        db=db,
        offset=compute_offset(page, items_per_page),
        limit=items_per_page,
        schema_to_select=SourceRegistryRead,
        return_total_count=True,
        sort_columns=[sort_by.value],
        sort_orders=[order],
        **filters,
    )

    response: dict[str, Any] = paginated_response(
        crud_data=sources_data, page=page, items_per_page=items_per_page
    )
    return response


@router.post(
    "",
    response_model=SourceRegistryRead,
    status_code=status.HTTP_201_CREATED,
    summary="Register source",
    responses=merge_responses(
        {
            status.HTTP_200_OK: {
                "model": SourceRegistryRead,
                "description": "Source already registered (idempotent)",
            },
            status.HTTP_201_CREATED: {
                "model": SourceRegistryRead,
                "description": "New source created",
            },
        },
        authenticated_responses(RESPONSES_BAD_REQUEST),
    ),
)
async def register_source(
    request: Request,
    source_data: SourceRegistryCreate,
    current_user: Annotated[dict, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> Response:
    """Register a source in the registry. Returns 200 when the source already exists."""
    _ensure_valid_project_module(source_data.project_module)

    existing = await source_registry_service.check_existing_source(
        db=db,
        project_module=source_data.project_module,
        source_identifier=source_data.source_identifier,
    )
    if existing:
        logger.info(
            "event=sources_register_existing project_module=%s source_identifier=%s",
            source_data.project_module,
            source_data.source_identifier,
        )
        return _json_response(existing, status_code=200)

    source = await source_registry_service.register_source(
        db=db,
        project_module=source_data.project_module,
        source_identifier=source_data.source_identifier,
        enabled=source_data.enabled,
    )
    logger.info(
        "event=sources_register_created project_module=%s source_identifier=%s",
        source_data.project_module,
        source_data.source_identifier,
    )
    return _json_response(source, status_code=201)


@router.post(
    "/bulk",
    response_model=SourceRegistryBulkCreateResponse,
    status_code=status.HTTP_200_OK,
    summary="Bulk register sources",
    responses=authenticated_responses(RESPONSES_BAD_REQUEST),
)
async def bulk_register_sources(
    request: Request,
    bulk_data: SourceRegistryBulkCreate,
    current_user: Annotated[dict, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> dict[str, Any]:
    """Register many sources in one request. Existing sources are returned in `existing` without error."""
    created: list[dict[str, Any]] = []
    existing: list[dict[str, Any]] = []

    for item in bulk_data.items:
        _ensure_valid_project_module(item.project_module)
        already = await source_registry_service.check_existing_source(
            db=db,
            project_module=item.project_module,
            source_identifier=item.source_identifier,
        )
        if already:
            existing.append(already)
            continue

        new_source = await source_registry_service.register_source(
            db=db,
            project_module=item.project_module,
            source_identifier=item.source_identifier,
            enabled=item.enabled,
        )
        created.append(new_source)

    logger.info(
        "event=sources_bulk_register total_created=%s total_existing=%s",
        len(created),
        len(existing),
    )
    return {
        "created": created,
        "existing": existing,
        "total_created": len(created),
        "total_existing": len(existing),
    }


@router.post(
    "/discover",
    response_model=DiscoverTriggerResponse,
    status_code=status.HTTP_200_OK,
    summary="Trigger discovery",
    responses=authenticated_responses(RESPONSES_BAD_REQUEST),
)
async def trigger_discovery(
    request: Request,
    body: DiscoverTriggerRequest,
    current_user: Annotated[dict, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> dict[str, Any]:
    """Mark sources for rediscovery. Discovery runs asynchronously via the background scheduler."""
    _ensure_valid_project_module(body.project_module)
    if body.source_identifier is not None:
        source_identifiers: list[str] | None = [body.source_identifier]
    else:
        source_identifiers = body.source_identifiers

    identifiers = await source_registry_service.mark_sources_for_rediscovery(
        db=db,
        project_module=body.project_module,
        source_identifiers=source_identifiers,
    )
    logger.info(
        "event=sources_trigger_discovery project_module=%s marked_count=%s",
        body.project_module,
        len(identifiers),
    )
    return {
        "project_module": body.project_module,
        "marked_count": len(identifiers),
        "source_identifiers": identifiers,
    }


@router.get(
    "/{source_id}/executions",
    response_model=PaginatedListResponse[SourceLinkedExecutionItem],
    summary="List source executions",
    responses=merge_responses(RESPONSES_NOT_FOUND),
)
async def list_source_executions(
    request: Request,
    source_id: UUID,
    db: Annotated[AsyncSession, Depends(async_get_db)],
    page: Annotated[int, Query(ge=1, description="Page number (1-based)")] = 1,
    items_per_page: Annotated[int, Query(ge=1, le=50, description="Page size (max 50)")] = 10,
) -> dict[str, Any]:
    """List batch executions that included this registry source."""
    source = await source_registry_service.get_source(db=db, source_id=source_id)
    offset = compute_offset(page, items_per_page)
    items_raw, total = await execution_ledger_service.list_executions_for_source(
        db,
        source["project_module"],
        source["source_identifier"],
        offset=offset,
        limit=items_per_page,
    )
    data = [SourceLinkedExecutionItem.model_validate(x) for x in items_raw]
    return paginated_response(
        crud_data={"data": data, "total_count": total},
        page=page,
        items_per_page=items_per_page,
    )


@router.get(
    "/{source_id}",
    response_model=SourceRegistryRead,
    summary="Get source",
    responses=merge_responses(RESPONSES_NOT_FOUND),
)
async def get_source(
    request: Request,
    source_id: UUID,
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> dict[str, Any]:
    """Return a single source registry entry by UUID."""
    return await source_registry_service.get_source(db=db, source_id=source_id)


@router.get(
    "/{source_id}/metadata",
    response_model=SourceMetadataResponse,
    summary="Get source metadata",
    responses=merge_responses(RESPONSES_NOT_FOUND),
)
async def get_source_metadata(
    request: Request,
    source_id: UUID,
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> dict[str, Any]:
    """Return archive metadata rows linked to this source."""
    source = await source_registry_service.get_source(db=db, source_id=source_id)

    metadata_list = await archive_metadata_service.list_metadata_for_source(
        db=db,
        project_module=source["project_module"],
        source_identifier=source["source_identifier"],
    )

    return {
        "source": source,
        "metadata": metadata_list,
        "metadata_count": len(metadata_list),
    }


@router.patch(
    "/{source_id}",
    response_model=SourceRegistryRead,
    summary="Update source",
    responses=authenticated_responses(RESPONSES_NOT_FOUND),
)
async def update_source(
    request: Request,
    source_id: UUID,
    source_data: SourceRegistryUpdate,
    current_user: Annotated[dict, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> dict[str, Any]:
    """Update enabled flag and/or stale-after hours for a source."""
    return await source_registry_service.update_source(
        db=db,
        source_id=source_id,
        enabled=source_data.enabled,
        stale_after_hours=source_data.stale_after_hours,
        update_stale_after_hours="stale_after_hours" in source_data.model_fields_set,
    )


@router.delete(
    "/{source_id}",
    status_code=status.HTTP_204_NO_CONTENT,
    summary="Delete source",
    responses=authenticated_responses(RESPONSES_NOT_FOUND),
)
async def delete_source(
    request: Request,
    source_id: UUID,
    current_user: Annotated[dict, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> Response:
    """Permanently remove a source from the registry."""
    source = await crud_source_registry.get(
        db=db,
        uuid=source_id,
        schema_to_select=SourceRegistryRead,
    )
    if not source:
        raise NotFoundException(f"Source with id {source_id} not found")

    await crud_source_registry.delete(db=db, uuid=source_id)
    return Response(status_code=status.HTTP_204_NO_CONTENT)

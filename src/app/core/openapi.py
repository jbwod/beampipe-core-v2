from typing import Any

from fastapi import FastAPI, status
from fastapi.openapi.utils import get_openapi
from pydantic import BaseModel, Field

OpenAPIResponses = dict[int | str, dict[str, Any]]

_HTTP_422: int = (
    status.HTTP_422_UNPROCESSABLE_CONTENT
    if hasattr(status, "HTTP_422_UNPROCESSABLE_CONTENT")
    else 422
)

OPENAPI_TAGS: list[dict[str, str]] = [
    {
        "name": "login",
        "description": "OAuth2 password flow and token refresh.",
    },
    {
        "name": "projects",
        "description": "Registered project modules and discovery contract validation status.",
    },
    {
        "name": "sources",
        "description": "Source registry: register astronomical sources, trigger discovery, and read archive metadata.",
    },
    {
        "name": "executions",
        "description": "Batch execution ledger: create runs, enqueue staging/submit jobs, and inspect status.",
    },
    {
        "name": "deployment-profiles",
        "description": "DALiuGE deployment profiles (translation + REST/Slurm remote deployment configuration).",
    },
    {
        "name": "health",
        "description": "Liveness, readiness, and archive TAP connectivity probes.",
    },
    {
        "name": "users",
        "description": "User management (admin).",
    },
]

OPENAPI_TAG_GROUPS: list[dict[str, Any]] = [
    {"name": "Authentication", "tags": ["login"]},
    {
        "name": "Core workflow",
        "tags": ["projects", "sources", "executions", "deployment-profiles"],
    },
    {"name": "Operations", "tags": ["health"]},
    {"name": "Admin", "tags": ["users"]},
]

OPENAPI_SERVERS: list[dict[str, str]] = [
    {"url": "/", "description": "Current host (API routes are under /api/v1)"},
]


class ErrorDetail(BaseModel):
    detail: str = Field(description="Human-readable error message", examples=["Resource not found"])


def _response(status_code: int, description: str, model: type[BaseModel] = ErrorDetail) -> OpenAPIResponses:
    return {
        status_code: {
            "model": model,
            "description": description,
        }
    }


RESPONSES_BAD_REQUEST = _response(status.HTTP_400_BAD_REQUEST, "Invalid request (e.g. unknown project module)")
RESPONSES_UNAUTHORIZED = _response(status.HTTP_401_UNAUTHORIZED, "Missing or invalid bearer token")
RESPONSES_FORBIDDEN = _response(status.HTTP_403_FORBIDDEN, "Authenticated but not permitted")
RESPONSES_NOT_FOUND = _response(status.HTTP_404_NOT_FOUND, "Resource not found")
RESPONSES_UNPROCESSABLE = _response(
    _HTTP_422,
    "Request validation failed",
)
RESPONSES_SERVER_ERROR = _response(status.HTTP_500_INTERNAL_SERVER_ERROR, "Unexpected server error")
RESPONSES_SERVICE_UNAVAILABLE = _response(
    status.HTTP_503_SERVICE_UNAVAILABLE,
    "Required dependency unavailable (e.g. Redis queue)",
)

RESPONSES_AUTHENTICATED: OpenAPIResponses = {
    **RESPONSES_UNAUTHORIZED,
    **RESPONSES_FORBIDDEN,
    **RESPONSES_UNPROCESSABLE,
}


def merge_responses(*maps: OpenAPIResponses) -> OpenAPIResponses:
    merged: OpenAPIResponses = {}
    for response_map in maps:
        merged.update(response_map)
    return merged


def authenticated_responses(*extra: OpenAPIResponses) -> OpenAPIResponses:
    return merge_responses(RESPONSES_AUTHENTICATED, *extra)


def build_openapi_schema(app: FastAPI) -> dict[str, Any]:
    if app.openapi_schema:
        return app.openapi_schema

    schema = get_openapi(
        title=app.title,
        version=app.version,
        openapi_version="3.1.0",
        description=app.description,
        routes=app.routes,
        tags=OPENAPI_TAGS,
    )
    if app.contact:
        schema.setdefault("info", {})["contact"] = app.contact
    if app.license_info:
        schema.setdefault("info", {})["license"] = app.license_info

    schema["servers"] = OPENAPI_SERVERS
    schema["x-tagGroups"] = OPENAPI_TAG_GROUPS

    app.openapi_schema = schema
    return schema

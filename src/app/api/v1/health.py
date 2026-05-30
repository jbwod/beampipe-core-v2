import logging
from datetime import UTC, datetime
from typing import Annotated

from fastapi import APIRouter, Depends, Response, status
from redis.asyncio import Redis
from sqlalchemy.ext.asyncio import AsyncSession

from ...core.archive.tap_health import get_tap_health
from ...core.config import settings
from ...core.db.database import async_get_db
from ...core.health import check_database_health, check_redis_health
from ...core.openapi import merge_responses
from ...core.schemas import HealthCheck, ReadyCheck, TapHealthCheck
from ...core.utils.cache import async_get_redis

router = APIRouter(tags=["health"])

STATUS_HEALTHY = "healthy"
STATUS_UNHEALTHY = "unhealthy"

logger = logging.getLogger(__name__)

_READY_RESPONSES = merge_responses(
    {
        status.HTTP_200_OK: {
            "model": ReadyCheck,
            "description": "All dependencies healthy",
        },
        status.HTTP_503_SERVICE_UNAVAILABLE: {
            "model": ReadyCheck,
            "description": "One or more dependencies unhealthy",
        },
    },
)


@router.get(
    "/health",
    response_model=HealthCheck,
    summary="Liveness probe",
    responses={
        status.HTTP_200_OK: {
            "model": HealthCheck,
            "description": "Application process is running",
        },
    },
)
async def health() -> HealthCheck:
    return HealthCheck(
        status=STATUS_HEALTHY,
        environment=settings.ENVIRONMENT.value,
        version=settings.APP_VERSION,
        timestamp=datetime.now(UTC).isoformat(timespec="seconds"),
    )


@router.get(
    "/ready",
    response_model=ReadyCheck,
    summary="Readiness probe",
    responses=_READY_RESPONSES,
)
async def ready(
    response: Response,
    redis: Annotated[Redis, Depends(async_get_redis)],
    db: Annotated[AsyncSession, Depends(async_get_db)],
) -> ReadyCheck:
    """Check PostgreSQL and Redis connectivity. Returns HTTP 503 when any dependency is down."""
    database_status = await check_database_health(db=db)
    logger.debug("event=health_ready_db_check ok=%s", database_status)
    redis_status = await check_redis_health(redis=redis)
    logger.debug("event=health_ready_redis_check ok=%s", redis_status)

    overall_status = STATUS_HEALTHY if database_status and redis_status else STATUS_UNHEALTHY
    if overall_status != STATUS_HEALTHY:
        response.status_code = status.HTTP_503_SERVICE_UNAVAILABLE

    return ReadyCheck(
        status=overall_status,
        environment=settings.ENVIRONMENT.value,
        version=settings.APP_VERSION,
        app=STATUS_HEALTHY,
        database=STATUS_HEALTHY if database_status else STATUS_UNHEALTHY,
        redis=STATUS_HEALTHY if redis_status else STATUS_UNHEALTHY,
        timestamp=datetime.now(UTC).isoformat(timespec="seconds"),
    )


@router.get(
    "/health/tap",
    response_model=TapHealthCheck,
    summary="Archive TAP probe",
    responses={
        status.HTTP_200_OK: {
            "model": TapHealthCheck,
            "description": "TAP endpoint probe results (may include unreachable endpoints)",
        },
    },
)
async def tap_health() -> TapHealthCheck:
    """Probe configured archive TAP endpoints used by discovery."""
    timeout = getattr(settings, "DISCOVERY_TAP_HEALTH_TIMEOUT_SECONDS", 10.0)
    health = await get_tap_health(timeout_seconds=float(timeout))
    return TapHealthCheck(
        all_ok=all(health.values()),
        endpoints=health,
        timestamp=datetime.now(UTC).isoformat(timespec="seconds"),
    )

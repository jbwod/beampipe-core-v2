import uuid as uuid_pkg
from datetime import UTC, datetime

from pydantic import BaseModel, ConfigDict, Field
from uuid6 import uuid7


class HealthCheck(BaseModel):
    model_config = ConfigDict(
        json_schema_extra={
            "examples": [
                {
                    "status": "healthy",
                    "environment": "local",
                    "version": "0.1.0",
                    "timestamp": "2026-05-29T12:00:00+00:00",
                }
            ]
        }
    )

    status: str = Field(description="Overall health: `healthy` or `unhealthy`", examples=["healthy"])
    environment: str = Field(description="Deployment environment name", examples=["local"])
    version: str | None = Field(default=None, description="Application version string")
    timestamp: str = Field(description="ISO-8601 timestamp of the probe")


class ReadyCheck(BaseModel):
    """Readiness probe: required dependencies (database, Redis) are reachable."""

    model_config = ConfigDict(
        json_schema_extra={
            "examples": [
                {
                    "status": "healthy",
                    "environment": "local",
                    "version": "0.1.0",
                    "app": "healthy",
                    "database": "healthy",
                    "redis": "healthy",
                    "timestamp": "2026-05-29T12:00:00+00:00",
                }
            ]
        }
    )

    status: str = Field(description="Overall readiness: `healthy` or `unhealthy`", examples=["healthy"])
    environment: str = Field(description="Deployment environment name", examples=["local"])
    version: str | None = Field(default=None, description="Application version string")
    app: str = Field(description="Application process status", examples=["healthy"])
    database: str = Field(description="PostgreSQL connectivity status", examples=["healthy"])
    redis: str = Field(description="Redis connectivity status", examples=["healthy"])
    timestamp: str = Field(description="ISO-8601 timestamp of the probe")


class TapHealthCheck(BaseModel):
    """Archive TAP endpoint reachability (used by discovery)."""

    model_config = ConfigDict(
        json_schema_extra={
            "examples": [
                {
                    "all_ok": True,
                    "endpoints": {"casda": True, "vizier": True},
                    "timestamp": "2026-05-29T12:00:00+00:00",
                }
            ]
        }
    )

    all_ok: bool = Field(description="True when every configured TAP endpoint responded successfully")
    endpoints: dict[str, bool] = Field(
        description="Per-endpoint probe result keyed by adapter name",
        examples=[{"casda": True}],
    )
    timestamp: str = Field(description="ISO-8601 timestamp of the probe")


# -------------- mixins --------------
class UUIDSchema(BaseModel):
    uuid: uuid_pkg.UUID = Field(default_factory=uuid7)


class TimestampSchema(BaseModel):
    created_at: datetime = Field(default_factory=lambda: datetime.now(UTC).replace(tzinfo=None))
    updated_at: datetime | None = Field(default=None)


class PersistentDeletion(BaseModel):
    deleted_at: datetime | None = Field(default=None)
    is_deleted: bool = False


# -------------- token --------------
class Token(BaseModel):
    access_token: str = Field(description="JWT bearer token")
    token_type: str = Field(default="bearer", description="Token type (always `bearer`)")


class TokenData(BaseModel):
    username_or_email: str


class TokenBlacklistBase(BaseModel):
    token: str
    expires_at: datetime


class TokenBlacklistRead(TokenBlacklistBase):
    id: int


class TokenBlacklistCreate(TokenBlacklistBase):
    pass


class TokenBlacklistUpdate(TokenBlacklistBase):
    pass

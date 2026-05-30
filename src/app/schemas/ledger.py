from datetime import datetime
from typing import Annotated, Any
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, model_validator

from ..core.schemas import TimestampSchema, UUIDSchema
from ..models.ledger import ExecutionPhase, ExecutionStatus


# /Users/jblackwo/beampipe-core/docs/user-guide/database/schemas.md
class ExecutionSourceSpec(BaseModel):
    """Per-source spec with optional SBID filter."""

    model_config = ConfigDict(json_schema_extra={"examples": [{"source_identifier": "HIPASSJ1318-21"}]})

    source_identifier: Annotated[str, Field(min_length=1, max_length=100, examples=["HIPASSJ1318-21"])]
    sbids: list[str] | None = Field(
        default=None,
        description="Optional: restrict to these SBIDs for this source. Omit to include all.",
        json_schema_extra={"examples": [["12345", "12346"]]},
    )


class BatchExecutionRecordBase(BaseModel):
    project_module: Annotated[
        str, Field(min_length=1, max_length=50, examples=["wallaby_hires"], description="Project module identifier")
    ]
    sources: Annotated[
        list[ExecutionSourceSpec],
        Field(min_length=1, description="Sources with optional per-source SBID filters"),
    ]
    archive_name: Annotated[
        str, Field(min_length=1, max_length=50, examples=["casda"], description="Archive name")
    ]


class BatchExecutionRecordCreate(BatchExecutionRecordBase):
    model_config = ConfigDict(
        extra="forbid",
        json_schema_extra={
            "examples": [
                {
                    "project_module": "wallaby_hires",
                    "sources": [{"source_identifier": "HIPASSJ1313-15"}],
                    "archive_name": "casda",
                    "deployment_profile_name": "setonix-slurm-ini",
                }
            ]
        },
    )

    deployment_profile_name: str | None = Field(
        default=None,
        min_length=1,
        max_length=50,
        description="DALiuGE deployment profile name to resolve at create time",
    )


class BatchExecutionRecordCreateInternal(BatchExecutionRecordBase):
    """(resolved UUID + status)"""

    model_config = ConfigDict(extra="forbid")

    deployment_profile_id: UUID | None = Field(
        default=None,
        description="DALiuGE deployment profile row UUID",
    )
    created_by_id: int | None = Field(default=None, description="User ID who triggered the execution")
    status: ExecutionStatus = Field(default=ExecutionStatus.PENDING, description="Initial execution status")


class BatchExecutionRecordRead(TimestampSchema, BatchExecutionRecordBase, UUIDSchema):
    model_config = ConfigDict(from_attributes=True)

    deployment_profile_id: UUID | None = None
    status: ExecutionStatus
    execution_phase: ExecutionPhase | None = None
    workflow_manifest: dict | None = None
    beampipe_run_record: dict[str, Any] | None = Field(
        default=None,
    )
    scheduler_name: str | None = None
    scheduler_job_id: str | None = None
    dim_session_status_url: str | None = Field(
        default=None,
        description="DIM GET /api/sessions/{id}/status",
    )
    dim_graph_status_url: str | None = Field(
        default=None,
        description="DIM GET /api/sessions/{id}/graph/status",
    )
    retry_count: int = 0
    last_error: str | None = None
    created_by_id: int | None = None
    started_at: datetime | None = None
    completed_at: datetime | None = None

    @model_validator(mode="before")
    @classmethod
    def hoist_beampipe_run_record(cls, data: Any) -> Any:
        if isinstance(data, dict):
            wm = data.get("workflow_manifest")
            if isinstance(wm, dict):
                rr = wm.get("beampipe_run_record")
                if isinstance(rr, dict):
                    return {**data, "beampipe_run_record": dict(rr)}
        return data


class BatchExecutionRecordListItem(TimestampSchema, BatchExecutionRecordBase, UUIDSchema):
    model_config = ConfigDict(from_attributes=True)

    deployment_profile_id: UUID | None = None
    status: ExecutionStatus
    execution_phase: ExecutionPhase | None = None
    scheduler_name: str | None = None
    scheduler_job_id: str | None = None
    retry_count: int = 0
    last_error: str | None = None
    created_by_id: int | None = None
    started_at: datetime | None = None
    completed_at: datetime | None = None


class BatchExecutionStatusResponse(BaseModel):
    model_config = ConfigDict(from_attributes=True)

    uuid: UUID
    status: ExecutionStatus
    execution_phase: ExecutionPhase | None = None
    scheduler_name: str | None = None
    scheduler_job_id: str | None = None
    last_error: str | None = None
    retry_count: int = 0
    started_at: datetime | None = None
    completed_at: datetime | None = None
    slurm_state: str | None = Field(
        default=None,
        description="Last observed SLURM state.",
    )
    dim_state: str | None = Field(
        default=None,
        description="Last observed DIM session state.",
    )

    @model_validator(mode="before")
    @classmethod
    def derive_observed_states(cls, data: Any) -> Any:
        if not isinstance(data, dict):
            return data
        wm = data.get("workflow_manifest")
        if not isinstance(wm, dict):
            return data
        rr = wm.get("beampipe_run_record")
        if not isinstance(rr, dict):
            return data
        out = dict(data)
        slurm = rr.get("slurm")
        if isinstance(slurm, dict):
            obs = slurm.get("last_observation")
            if isinstance(obs, dict) and obs.get("state"):
                out.setdefault("slurm_state", str(obs.get("state")))
        dim = rr.get("dim")
        if isinstance(dim, dict):
            obs = dim.get("last_observation")
            if isinstance(obs, dict) and obs.get("session_state"):
                out.setdefault("dim_state", str(obs.get("session_state")))
        return out


class BatchExecutionSummary(BaseModel):

    model_config = ConfigDict(from_attributes=True)

    uuid: UUID
    project_module: str
    archive_name: str
    status: ExecutionStatus
    execution_phase: ExecutionPhase | None = None
    scheduler_name: str | None = None
    scheduler_job_id: str | None = None
    requested_source_count: int = 0
    requested_source_identifiers: list[str] = Field(default_factory=list)
    slurm_state: str | None = None
    dim_state: str | None = None
    last_observation_at: datetime | None = None
    last_error: str | None = None
    retry_count: int = 0
    started_at: datetime | None = None
    completed_at: datetime | None = None
    created_at: datetime | None = None
    duration_seconds: float | None = Field(
        default=None,
        description="completed_at - started_at when both are present (rounded ms precision).",
    )

    @model_validator(mode="before")
    @classmethod
    def derive_summary_fields(cls, data: Any) -> Any:
        if not isinstance(data, dict):
            return data
        out = dict(data)
        wm = out.get("workflow_manifest")
        rr: dict | None = None
        if isinstance(wm, dict):
            maybe_rr = wm.get("beampipe_run_record")
            if isinstance(maybe_rr, dict):
                rr = maybe_rr
        if rr is None:
            maybe_rr = out.get("beampipe_run_record")
            if isinstance(maybe_rr, dict):
                rr = maybe_rr
        if isinstance(rr, dict):
            requested = rr.get("requested_sources")
            if isinstance(requested, dict):
                ids = requested.get("source_identifiers")
                if isinstance(ids, list):
                    out.setdefault("requested_source_identifiers", [str(s) for s in ids])
                count = requested.get("count")
                if isinstance(count, int):
                    out.setdefault("requested_source_count", count)
            slurm = rr.get("slurm")
            if isinstance(slurm, dict):
                obs = slurm.get("last_observation")
                if isinstance(obs, dict):
                    if obs.get("state"):
                        out.setdefault("slurm_state", str(obs.get("state")))
                    if obs.get("observed_at"):
                        out.setdefault("last_observation_at", obs.get("observed_at"))
            dim = rr.get("dim")
            if isinstance(dim, dict):
                obs = dim.get("last_observation")
                if isinstance(obs, dict):
                    if obs.get("session_state"):
                        out.setdefault("dim_state", str(obs.get("session_state")))
                    if obs.get("observed_at"):
                        out.setdefault("last_observation_at", obs.get("observed_at"))

        if not out.get("requested_source_identifiers"):
            sources = out.get("sources") or []
            if isinstance(sources, list):
                ids = [
                    str(s.get("source_identifier"))
                    for s in sources
                    if isinstance(s, dict) and s.get("source_identifier")
                ]
                if ids:
                    out["requested_source_identifiers"] = ids
                    out.setdefault("requested_source_count", len(ids))
        started = out.get("started_at")
        completed = out.get("completed_at")
        if isinstance(started, datetime) and isinstance(completed, datetime):
            try:
                out.setdefault(
                    "duration_seconds",
                    round((completed - started).total_seconds(), 3),
                )
            except (TypeError, ValueError):
                pass
        return out


class BatchExecutionRecordUpdate(BaseModel):
    """Schema for updating execution records via API.

    Note: status, started_at, and completed_at are managed automatically
    by the service layer based on status transitions.
    """
    model_config = ConfigDict(extra="forbid")

    status: ExecutionStatus | None = Field(default=None, description="New execution status")
    workflow_manifest: dict | None = Field(default=None, description="Workflow manifest JSON")
    scheduler_name: str | None = Field(default=None, max_length=50, description="Name of scheduler")
    scheduler_job_id: str | None = Field(default=None, max_length=512, description="Scheduler job ID")
    last_error: str | None = Field(default=None, description="Error message if execution failed")


class BatchExecutionRecordUpdateInternal(BatchExecutionRecordUpdate):
    updated_at: datetime
    started_at: datetime | None = None
    completed_at: datetime | None = None
    execution_phase: ExecutionPhase | None = None


class BatchExecutionRecordDelete(BaseModel):
    model_config = ConfigDict(extra="forbid")
    is_deleted: bool = Field(default=True, description="Soft delete flag for the execution record")
    deleted_at: datetime | None = Field(default=None, description="Timestamp when the record was deleted")


# Prepare execution (validate + preview, no DB write)
class ExecuteRequest(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
        json_schema_extra={"examples": [{"do_stage": True, "do_submit": True}]},
    )

    do_stage: bool = Field(default=True, description="Stage data from the archive before execution")
    do_submit: bool = Field(default=True, description="Submit the graph to DALiuGE after staging")


class PrepareExecutionRequest(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
        json_schema_extra={
            "examples": [
                {
                    "project_module": "wallaby_hires",
                    "sources": [{"source_identifier": "HIPASSJ1313-15"}],
                }
            ]
        },
    )

    project_module: Annotated[
        str, Field(min_length=1, max_length=50, examples=["wallaby_hires"], description="Project module identifier")
    ]
    sources: Annotated[
        list[ExecutionSourceSpec],
        Field(min_length=1, description="Sources with optional per-source SBID filters"),
    ]


class PrepareExecutionResponse(BaseModel):
    """Preview of what would be included in an execution."""

    project_module: str = Field(description="Project module identifier")
    sources: list[ExecutionSourceSpec] = Field(description="Requested source specs")
    sources_preview: list[dict[str, Any]] = Field(
        description="Per-source preview: source_identifier, sbid_count, dataset_count",
    )
    total_datasets: int = Field(description="Total archive datasets across all sources")
    valid: bool = Field(description="True when the execution would pass validation")
    errors: list[str] = Field(default_factory=list, description="Validation errors when valid is false")


class ExecutionLedgerSnapshot(BaseModel):
    """Compact execution view for operators and Restate workflow correlation."""

    model_config = ConfigDict(extra="allow")

    execution_id: str = Field(description="Batch execution UUID")
    project_module: str | None = Field(default=None, description="Project module identifier")
    status: str | None = Field(default=None, description="Execution status")
    execution_phase: str | None = Field(default=None, description="Current execution phase")
    scheduler_job_id: str | None = Field(default=None, description="Scheduler session or job id")
    scheduler_name: str | None = Field(default=None, description="Scheduler backend name")
    has_manifest: bool = Field(description="Whether a workflow manifest is persisted")
    has_beampipe_run_record: bool = Field(description="Whether beampipe run record data exists in the manifest")
    last_error: str | None = Field(default=None, description="Most recent error message")
    retry_count: int = Field(default=0, description="Number of retries attempted")
    deployment_profile_id: str | None = Field(default=None, description="Linked deployment profile UUID")
    created_at: str | None = Field(default=None, description="ISO-8601 creation timestamp")
    updated_at: str | None = Field(default=None, description="ISO-8601 last update timestamp")
    started_at: str | None = Field(default=None, description="ISO-8601 start timestamp")
    completed_at: str | None = Field(default=None, description="ISO-8601 completion timestamp")
    sources: list[Any] = Field(default_factory=list, description="Source specs included in the execution")
    source_identifiers: list[str] = Field(default_factory=list, description="Flattened source identifier list")
    beampipe_run_record: dict[str, Any] | None = Field(
        default=None,
        description="Embedded beampipe run record from workflow manifest when present",
    )
    dim_session_status_url: str | None = Field(default=None, description="DIM session status URL when available")
    dim_graph_status_url: str | None = Field(default=None, description="DIM graph status URL when available")


class ExecuteAcceptedResponse(BaseModel):
    model_config = ConfigDict(
        json_schema_extra={
            "examples": [
                {
                    "status": "accepted",
                    "execution_id": "019302ab-1234-7890-abcd-ef1234567890",
                    "job_id": "abc123",
                    "do_stage": True,
                    "do_submit": True,
                }
            ]
        }
    )

    status: str = Field(default="accepted", description="Always `accepted` when enqueue succeeds")
    execution_id: str = Field(description="Batch execution UUID")
    job_id: str = Field(description="ARQ worker job id")
    do_stage: bool = Field(description="Whether archive staging was requested")
    do_submit: bool = Field(description="Whether DALiuGE submit was requested")

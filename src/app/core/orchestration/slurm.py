import json
import logging
import os
import re
import shlex
from configparser import ConfigParser
from io import StringIO
from typing import Any
from uuid import UUID

import httpx
from sqlalchemy.ext.asyncio import AsyncSession

from ...models.ledger import ExecutionStatus
from ..config import settings
from ..exceptions.workflow_exceptions import (
    WorkflowErrorCode,
    WorkflowFailure,
    wf_execution_not_found,
)
from ..ledger.run_record import merge_slurm_poll_into_manifest, merge_slurm_submit_into_manifest
from ..ledger.service import execution_ledger_service
from ..ledger.source_readiness import source_identifiers_from_specs
from ..registry.service import source_registry_service
from .slurm_client import (
    SBATCH_PARSABLE_RE,
    SlurmClientError,
    SlurmDeployClient,
    compose_scheduler_job_id,
    parse_scheduler_job_id,
    session_debug_paths,
    shell_quote,
)
from .translate import (
    fail_execution_after_translate_error,
    translate_lg_to_pgt_artifact,
)

logger = logging.getLogger(__name__)

# https://github.com/ICRAR/daliuge/blob/master/daliuge-engine/dlg/deploy/slurm_client.py
_JOBSUB_CREATED_RE = re.compile(
    r"Created job submission script\s+(?P<path>\S+/jobsub\.sh)"
)

__all__ = [
    "cancel_session",
    "poll_session",
    "slurm_session_debug_paths",
    "submit_session_payload",
    "translate",
]


def slurm_session_debug_paths(scheduler_job_id: str) -> dict[str, str]:
    return session_debug_paths(scheduler_job_id)

def _resolve_remote_user(deployment_config: dict[str, Any]) -> str | None:
    return (
        deployment_config.get("remote_user")
        or os.environ.get("SLURM_REMOTE_USER")
        or os.environ.get("USER")
    )
def _ssh_port_from_deployment(deployment_config: dict[str, Any] | None) -> int:
    if not deployment_config:
        # Probably 22
        return 22
    raw = deployment_config.get("ssh_port")
    if raw is None:
        return 22
    try:
        p = int(raw)
    except (TypeError, ValueError):
        return 22
    if 1 <= p <= 65535:
        return p
    return 22


def _slurm_ssh_passphrase() -> str | None:
    sec = settings.SLURM_SSH_PRIVATE_KEY_PASSPHRASE
    if sec is None:
        return None
    val = sec.get_secret_value()
    return val if val else None


def _slurm_ssh_kwargs(
    login_node: str,
    username: str,
    *,
    deployment_config: dict[str, Any] | None,
) -> dict[str, Any]:
    if settings.SLURM_SSH_USE_AGENT and settings.SLURM_SSH_AUTH_SOCK:
        os.environ["SSH_AUTH_SOCK"] = settings.SLURM_SSH_AUTH_SOCK

    client_keys = [settings.SLURM_SSH_PRIVATE_KEY_PATH] if settings.SLURM_SSH_PRIVATE_KEY_PATH else None
    out: dict[str, Any] = {
        "host": login_node,
        "username": username,
        "port": _ssh_port_from_deployment(deployment_config),
        "client_keys": client_keys,
        "known_hosts": settings.SLURM_SSH_KNOWN_HOSTS,
        "connect_timeout": settings.SLURM_SSH_CONNECT_TIMEOUT_SECONDS,
        "command_timeout": settings.SLURM_SSH_COMMAND_TIMEOUT_SECONDS,
    }
    pp = _slurm_ssh_passphrase()
    if pp and client_keys:
        out["passphrase"] = pp
    return out


async def translate(
    *,
    db: AsyncSession,
    execution: dict,
    execution_id: UUID,
    project_module: str,
    session_id: str,
    graph_json: Any,
    lg_name: str,
    profile: dict,
) -> dict[str, Any]:
    """slurm_remote: TM + UNROL/PARTITION + validate the SSH profile."""
    from .rest_client.translator_client import DaliugeTranslatorClient

    tm_url = profile.get("tm_url")
    if not tm_url:
        raise WorkflowFailure(
            WorkflowErrorCode.EXECUTION_DEPLOYMENT_PROFILE,
            "slurm_remote requires tm_url on the deployment profile",
        )

    deployment_config = dict(profile.get("deployment_config") or {})
    dlg_root = str(deployment_config.get("dlg_root") or "").strip()
    if not dlg_root:
        raise WorkflowFailure(
            WorkflowErrorCode.EXECUTION_DEPLOYMENT_PROFILE,
            "slurm_remote requires dlg_root on the deployment profile",
        )
    login_node = str(deployment_config.get("login_node") or "").strip()
    if not login_node:
        raise WorkflowFailure(
            WorkflowErrorCode.EXECUTION_DEPLOYMENT_PROFILE,
            "slurm_remote requires login_node on the deployment profile",
        )

    username = _resolve_remote_user(deployment_config)
    if not username:
        raise WorkflowFailure(
            WorkflowErrorCode.EXECUTION_DEPLOYMENT_PROFILE,
            "slurm_remote could not resolve a remote_user (set on profile or USER env)",
        )

    translator = DaliugeTranslatorClient(
        base_url=tm_url,
        verify=profile["verify_ssl"],
    )
    try:
        try:
            pgt_name, pgt_json = await translate_lg_to_pgt_artifact(
                translator=translator,
                lg_name=lg_name,
                graph_json=graph_json,
                profile=profile,
            )
            # DALiuGE's SlurmClient derives its session dir name from the PGT handle.
            # Use hyphens (not underscores) so daliuge-engine's split(\"_\") doesn't truncate.
            pgt_handle = f"{session_id}.pgt.graph"
            if isinstance(pgt_json, list) and len(pgt_json) >= 1:
                pgt_json[0] = pgt_handle
            pgt_name = pgt_handle
        except (httpx.RequestError, json.JSONDecodeError, ValueError) as e:
            err_detail = str(e)
            if isinstance(e, httpx.HTTPStatusError) and e.response is not None:
                body = (e.response.text or "").strip()[:1200]
                if body:
                    err_detail = f"{err_detail} response_body={body}"
            logger.warning(
                "event=translate_slurm_tm_error execution_id=%s project_module=%s error=%s",
                execution_id,
                project_module,
                err_detail,
                exc_info=True,
            )
            return await fail_execution_after_translate_error(
                db=db,
                execution=execution,
                execution_id=execution_id,
                project_module=project_module,
                error_message=err_detail,
                session_id=session_id,
            )
    finally:
        translator.close()

    return {
        "status": "ready_slurm",
        "session_id": session_id,
        "pgt_name": pgt_name,
        "pgt_json": pgt_json,
        "deployment_config": deployment_config,
        "dlg_root": dlg_root,
        "login_node": login_node,
        "username": str(username),
    }


def _env_prelude(deployment_config: dict[str, Any]) -> str:
    """Build the bash prelude that loads modules + activates the venv.
    """
    parts = ["set -euo pipefail"]
    modules = str(deployment_config.get("modules") or "").strip()
    if modules:
        parts.append("set +u")
        for line in modules.splitlines():
            line = line.strip()
            if line:
                parts.append(line)
        parts.append("set -u")
    venv = str(deployment_config.get("venv") or "").strip()
    if venv:
        parts.append("set +u")
        parts.append(venv)
        parts.append("set -u")
    return "\n".join(parts)


def _create_dlg_job_argv(
    *,
    deployment_config: dict[str, Any],
    pgt_remote_path: str,
    config_file_remote_path: str,
    slurm_template_remote_path: str | None,
) -> list[str]:
    facility = str(deployment_config.get("facility") or "setonix")

    argv: list[str] = [
        "python3",
        "-m",
        "dlg.deploy.create_dlg_job",
        "--action",
        "submit",
        "-f",
        facility,
        "-P",
        pgt_remote_path,
        "--config_file",
        config_file_remote_path,
    ]
    if slurm_template_remote_path:
        argv.extend(["--slurm_template", slurm_template_remote_path])
    return argv


# https://github.com/ICRAR/daliuge/tree/master/daliuge-engine/dlg/deploy/configs
# https://daliuge.readthedocs.io/en/latest/deployment/slurm_deployment.html#configuration-ini
def _render_generated_ini(
    *,
    deployment_config: dict[str, Any],
    username: str,
    pgt_remote_path: str,
    dlg_root: str,
) -> str:
    cfg = ConfigParser(interpolation=None)
    cfg.optionxform = str

    all_nics = bool(deployment_config.get("all_nics"))
    check_with_session = bool(deployment_config.get("check_with_session"))
    zerorun = bool(deployment_config.get("zerorun"))
    sleepncopy = bool(deployment_config.get("sleepncopy"))

    cfg["DEPLOYMENT"] = {
        "remote": "False",
        "submit": "False",
    }
    cfg["ENGINE"] = {
        "NUM_NODES": str(int(deployment_config.get("num_nodes") or 1)),
        "NUM_ISLANDS": str(int(deployment_config.get("num_islands") or 1)),
        "ALL_NICS": "True" if all_nics else "",
        "CHECK_WITH_SESSION": "True" if check_with_session else "",
        "MAX_THREADS": str(int(deployment_config.get("max_threads") or 0)),
        "VERBOSE_LEVEL": str(int(deployment_config.get("verbose_level") or 1)),
        "ZERORUN": "True" if zerorun else "",
        "SLEEPNCOPY": "True" if sleepncopy else "",
    }
    cfg["GRAPH"] = {
        "PHYSICAL_GRAPH": pgt_remote_path,
    }
    cfg["FACILITY"] = {
        "USER": username,
        "ACCOUNT": str(deployment_config.get("account") or ""),
        "LOGIN_NODE": str(deployment_config.get("login_node") or ""),
        "HOME_DIR": str(deployment_config.get("home_dir") or ""),
        "DLG_ROOT": dlg_root,
        "LOG_DIR": str(deployment_config.get("log_dir") or f"{dlg_root.rstrip('/')}/log"),
        "MODULES": str(deployment_config.get("modules") or ""),
        "VENV": str(deployment_config.get("venv") or ""),
        "EXEC_PREFIX": str(deployment_config.get("exec_prefix") or "srun -l"),
    }

    out = StringIO()
    cfg.write(out)
    return out.getvalue()


def _parse_jobsub_path(stdout: str, *, stderr: str = "") -> str:
    match = _JOBSUB_CREATED_RE.search(stdout or "")
    if not match:
        stderr_clean = (stderr or "").strip()
        stderr_suffix = f" stderr={stderr_clean!r}" if stderr_clean else ""
        raise SlurmClientError(
            "create_dlg_job did not print a 'Created job submission script ...' "
            f"line; stdout was: {stdout!r}{stderr_suffix}"
        )
    return match.group("path")


async def submit_session_payload(
    db: AsyncSession,
    execution_id: UUID,
    *,
    session_id: str,
    pgt_json: Any,
    deployment_config: dict[str, Any],
    dlg_root: str,
    login_node: str,
    username: str,
) -> None:
    """Stage the PGT, invoke create_dlg_job, sbatch, and record the job id.
    """
    from ...crud.crud_execution_record import crud_batch_execution_records
    from ...schemas.ledger import BatchExecutionRecordRead

    execution = await crud_batch_execution_records.get(
        db=db, uuid=execution_id, schema_to_select=BatchExecutionRecordRead
    )
    if not execution:
        raise wf_execution_not_found(execution_id)
    if execution.get("scheduler_name") == "slurm" and execution.get("scheduler_job_id"):
        existing = str(execution["scheduler_job_id"])
        if existing.startswith(session_id) or existing == session_id:
            return

    staging_dir = f"{dlg_root.rstrip('/')}/staging"
    # Use the execution id (not session id) so the remote filename is
    # deterministic without the BeampipeExecution
    pgt_remote_path = f"{staging_dir}/BeampipeExecution_{execution_id}.pgt.graph"
    config_file_remote_path = f"{staging_dir}/BeampipeExecution_{execution_id}.ini"
    slurm_template_remote_path: str | None = None
    slurm_template_raw = deployment_config.get("slurm_template")
    slurm_template_body = str(slurm_template_raw) if slurm_template_raw else ""

    if slurm_template_body.strip():
        slurm_template_remote_path = (
            f"{staging_dir}/BeampipeExecution_{execution_id}.slurm"
        )

    logger.info(
        "event=slurm_submit_begin execution_id=%s session_id=%s login_node=%s "
        "pgt_remote=%s config_file=%s slurm_template=%s",
        execution_id,
        session_id,
        login_node,
        pgt_remote_path,
        config_file_remote_path,
        slurm_template_remote_path,
    )

    async with SlurmDeployClient(
        **_slurm_ssh_kwargs(login_node, username, deployment_config=deployment_config)
    ) as client:
        await client.mkdir_p(staging_dir)
        await client.put_text(pgt_remote_path, json.dumps(pgt_json))
        await client.put_text(
            config_file_remote_path,
            _render_generated_ini(
                deployment_config=deployment_config,
                username=username,
                pgt_remote_path=pgt_remote_path,
                dlg_root=dlg_root,
            ),
        )
        if slurm_template_remote_path:
            await client.put_text(slurm_template_remote_path, slurm_template_body)

        prelude = _env_prelude(deployment_config)
        argv = _create_dlg_job_argv(
            deployment_config=deployment_config,
            pgt_remote_path=pgt_remote_path,
            config_file_remote_path=config_file_remote_path,
            slurm_template_remote_path=slurm_template_remote_path,
        )
        # export DLG_ROOT=... guarantees
        # prefers the env var over --dlg_root, so we keep them aligned.
        # https://github.com/ICRAR/daliuge/blob/master/daliuge-engine/dlg/deploy/slurm_client.py
        inner = (
            f"{prelude}\n"
            f"export DLG_ROOT={shell_quote(dlg_root)}\n"
            f"{shlex.join(argv)}"
        )
        create_cmd = f"bash -lc {shell_quote(inner)}"
        create_stdout, create_stderr, _ = await client.run_command(create_cmd)
        jobsub_remote = _parse_jobsub_path(create_stdout, stderr=create_stderr)
        session_dir = jobsub_remote.rsplit("/", 1)[0]
        manifest_json = execution.get("workflow_manifest") or {}
        await client.put_text(
            f"{session_dir}/manifest.json",
            json.dumps(manifest_json, indent=2, sort_keys=True) + "\n",
        )
        logger.info(
            "event=slurm_create_job_generated execution_id=%s session_dir=%s jobsub=%s",
            execution_id,
            session_dir,
            jobsub_remote,
        )

        sbatch_inner = (
            f"cd {shell_quote(session_dir)} && "
            f"sbatch --parsable {shell_quote(jobsub_remote)}"
        )
        sbatch_cmd = f"bash -lc {shell_quote(sbatch_inner)}"
        sbatch_stdout, _sbatch_stderr, _ = await client.run_command(sbatch_cmd)
        match = SBATCH_PARSABLE_RE.search(sbatch_stdout or "")
        if not match:
            raise SlurmClientError(
                f"unable to parse sbatch --parsable output: {sbatch_stdout!r}"
            )
        slurm_job_id = match.group(1)

    try:
        composite = compose_scheduler_job_id(
            session_id=session_id,
            slurm_job_id=slurm_job_id,
            session_dir=session_dir,
        )
    except ValueError as e:
        raise WorkflowFailure(
            WorkflowErrorCode.EXECUTION_SLURM_STATE,
            (
                "Unable to persist scheduler_job_id for submitted SLURM job; "
                f"session_id={session_id!r} slurm_job_id={slurm_job_id!r} "
                f"session_dir={session_dir!r}"
            ),
            cause=e,
        ) from e
    logger.info(
        "event=slurm_submit_done execution_id=%s session_id=%s slurm_job_id=%s session_dir=%s",
        execution_id,
        session_id,
        slurm_job_id,
        session_dir,
    )
    path_bundle: dict[str, str] = {
        "session_dir": session_dir,
        "dlg_staging_dir": staging_dir,
        "pgt_remote_path": pgt_remote_path,
        "config_file_remote_path": config_file_remote_path,
        "jobsub_remote_path": jobsub_remote,
    }
    if slurm_template_remote_path:
        path_bundle["slurm_template_remote_path"] = slurm_template_remote_path
    merged_manifest = merge_slurm_submit_into_manifest(
        execution.get("workflow_manifest"),
        session_id=session_id,
        slurm_job_id=str(slurm_job_id),
        composite_scheduler_job_id=composite,
        login_node=login_node,
        remote_user=username,
        paths=path_bundle,
    )
    # The SLURM job is now in the schedulers hands. We flip the ledger to
    # AWAITING_SCHEDULER so the row stops counting against in-flight caps - here we hook
    # separate SlurmCompletionWorkflow (kicked off by the caller) owns the
    # squeue/sacct poll loop from here on.
    await execution_ledger_service.update_execution_status(
        db=db,
        execution_id=execution_id,
        status=ExecutionStatus.AWAITING_SCHEDULER,
        scheduler_name="slurm",
        scheduler_job_id=composite,
        execution_phase=None,
        workflow_manifest=merged_manifest,
    )


async def cancel_session(
    db: AsyncSession,
    execution_id: UUID,
    *,
    execution: dict[str, Any],
    profile: dict[str, Any],
) -> dict[str, Any]:
    raw_id = str(execution.get("scheduler_job_id") or "")
    if not raw_id:
        return {"cancelled": False, "reason": "no_scheduler_job_id"}
    parsed = parse_scheduler_job_id(raw_id)
    slurm_job_id = parsed.slurm_job_id
    if not slurm_job_id:
        return {"cancelled": False, "reason": "no_slurm_job_id"}

    deployment_config = dict(profile.get("deployment_config") or {})
    login_node = str(deployment_config.get("login_node") or "").strip()
    username = _resolve_remote_user(deployment_config)
    if not login_node or not username:
        return {"cancelled": False, "reason": "incomplete_profile"}

    try:
        async with SlurmDeployClient(
            **_slurm_ssh_kwargs(login_node, str(username), deployment_config=deployment_config)
        ) as client:
            await client.cancel_job(slurm_job_id)
        logger.info(
            "event=slurm_scancel_dispatched execution_id=%s slurm_job_id=%s",
            execution_id,
            slurm_job_id,
        )
        return {"cancelled": True, "slurm_job_id": slurm_job_id}
    except Exception as e:
        logger.warning(
            "event=slurm_scancel_failed execution_id=%s slurm_job_id=%s error=%s",
            execution_id,
            slurm_job_id,
            e,
        )
        return {"cancelled": False, "reason": "scancel_error", "error": str(e)}


async def poll_session(
    db: AsyncSession,
    execution_id: UUID,
    *,
    execution: dict[str, Any],
    profile: dict[str, Any],
) -> dict[str, Any]:
    """Poll a SLURM job"""
    raw_id = str(execution.get("scheduler_job_id") or "")
    if not raw_id:
        raise WorkflowFailure(
            WorkflowErrorCode.EXECUTION_SLURM_STATE,
            f"Execution {execution_id} has no scheduler_job_id; submit_slurm first",
        )
    parsed = parse_scheduler_job_id(raw_id)
    slurm_job_id = parsed.slurm_job_id

    deployment_config = dict(profile.get("deployment_config") or {})
    login_node = str(deployment_config.get("login_node") or "").strip()
    username = _resolve_remote_user(deployment_config)
    if not login_node or not username:
        raise WorkflowFailure(
            WorkflowErrorCode.EXECUTION_DEPLOYMENT_PROFILE,
            "slurm_remote polling requires login_node + remote_user",
        )

    async with SlurmDeployClient(
        **_slurm_ssh_kwargs(login_node, str(username), deployment_config=deployment_config)
    ) as client:
        result = await client.query_job_state(slurm_job_id)

    state = str(result.get("state") or "UNKNOWN")
    source = str(result.get("source") or "")
    if state == "UNKNOWN" and source == "none":
        logger.warning(
            "event=slurm_poll_state_unknown execution_id=%s slurm_job_id=%s",
            execution_id,
            slurm_job_id,
        )

        merged_unknown = merge_slurm_poll_into_manifest(
            execution.get("workflow_manifest"),
            composite_scheduler_job_id=raw_id,
            slurm_job_id=slurm_job_id,
            state=state,
            source=source,
            exit_code=result.get("exit_code"),
            raw_line=result.get("raw") if isinstance(result.get("raw"), str) else None,
            record_terminal=False,
            terminal_ledger_status=None,
            remote_session_dir=parsed.session_dir,
        )
        await execution_ledger_service.update_execution_status(
            db=db,
            execution_id=execution_id,
            workflow_manifest=merged_unknown,
        )
        return {
            "terminal": False,
            "slurm_state": state,
            "slurm_job_id": slurm_job_id,
            "slurm_source": source,
        }
    if state in {"PENDING", "RUNNING"}:
        logger.debug(
            "event=slurm_poll_active execution_id=%s slurm_job_id=%s state=%s source=%s",
            execution_id,
            slurm_job_id,
            state,
            source,
        )
        merged = merge_slurm_poll_into_manifest(
            execution.get("workflow_manifest"),
            composite_scheduler_job_id=raw_id,
            slurm_job_id=slurm_job_id,
            state=state,
            source=source,
            exit_code=result.get("exit_code"),
            raw_line=result.get("raw") if isinstance(result.get("raw"), str) else None,
            record_terminal=False,
            terminal_ledger_status=None,
            remote_session_dir=parsed.session_dir,
        )
        next_status: ExecutionStatus | None = None
        current_status_raw = execution.get("status")
        current_status_value = (
            current_status_raw.value
            if isinstance(current_status_raw, ExecutionStatus)
            else str(current_status_raw or "")
        )
        if state == "RUNNING" and current_status_value == ExecutionStatus.AWAITING_SCHEDULER.value:
            next_status = ExecutionStatus.RUNNING
            logger.info(
                "event=slurm_job_running execution_id=%s slurm_job_id=%s",
                execution_id,
                slurm_job_id,
            )
        await execution_ledger_service.update_execution_status(
            db=db,
            execution_id=execution_id,
            status=next_status,
            workflow_manifest=merged,
        )
        return {
            "terminal": False,
            "slurm_state": state,
            "slurm_job_id": slurm_job_id,
            "slurm_source": source,
        }

    project_module = execution["project_module"]
    source_identifiers = source_identifiers_from_specs(execution.get("sources"))
    await source_registry_service.clear_workflow_pending_for_sources(
        db=db,
        project_module=project_module,
        source_identifiers=source_identifiers,
        commit=False,
    )

    exit_code = result.get("exit_code")

    if state == "COMPLETED":
        logger.info(
            "event=slurm_job_completed execution_id=%s slurm_job_id=%s exit_code=%s",
            execution_id,
            slurm_job_id,
            exit_code,
        )
        merged = merge_slurm_poll_into_manifest(
            execution.get("workflow_manifest"),
            composite_scheduler_job_id=raw_id,
            slurm_job_id=slurm_job_id,
            state=state,
            source=source,
            exit_code=exit_code,
            raw_line=result.get("raw") if isinstance(result.get("raw"), str) else None,
            record_terminal=True,
            terminal_ledger_status="completed",
            remote_session_dir=parsed.session_dir,
        )
        await execution_ledger_service.update_execution_status(
            db=db,
            execution_id=execution_id,
            status=ExecutionStatus.COMPLETED,
            scheduler_name="slurm",
            scheduler_job_id=raw_id,
            execution_phase=None,
            workflow_manifest=merged,
        )
        logger.info(
            "event=ledger_slurm_terminal_persisted execution_id=%s slurm_job_id=%s "
            "state=%s source=%s exit_code=%s ledger_status=completed",
            execution_id,
            slurm_job_id,
            state,
            source,
            exit_code,
        )
        return {
            "terminal": True,
            "status": "completed",
            "slurm_state": state,
            "slurm_job_id": slurm_job_id,
            "slurm_exit_code": exit_code,
        }

    if state == "CANCELLED":
        terminal_status = ExecutionStatus.CANCELLED
        terminal_ledger = "cancelled"
        reason = "scheduler_cancelled"
    elif state == "TIMEOUT":
        terminal_status = ExecutionStatus.FAILED
        terminal_ledger = "failed"
        reason = "timeout"
    else:
        terminal_status = ExecutionStatus.FAILED
        terminal_ledger = "failed"
        reason = state.lower() if state else "unknown"

    error_msg = (
        f"SLURM job {slurm_job_id} finished in state={state} reason={reason}"
        + (f" exit_code={exit_code}" if exit_code is not None else "")
    )
    if parsed.session_dir:
        # Help operators jump straight to the failing job's stderr.
        error_msg += f" stderr_glob={parsed.session_dir.rstrip('/')}/logs/err-*.log"
    logger.error(
        "event=slurm_job_terminal execution_id=%s slurm_job_id=%s state=%s "
        "ledger_status=%s reason=%s exit_code=%s",
        execution_id,
        slurm_job_id,
        state,
        terminal_ledger,
        reason,
        exit_code,
    )
    diag: dict[str, Any] | None = None
    if parsed.session_dir:
        diag = {"stderr_glob": f"{parsed.session_dir.rstrip('/')}/logs/err-*.log"}
    merged = merge_slurm_poll_into_manifest(
        execution.get("workflow_manifest"),
        composite_scheduler_job_id=raw_id,
        slurm_job_id=slurm_job_id,
        state=state,
        source=source,
        exit_code=exit_code,
        raw_line=result.get("raw") if isinstance(result.get("raw"), str) else None,
        record_terminal=True,
        terminal_ledger_status=terminal_ledger,
        remote_session_dir=parsed.session_dir,
        diagnostics=diag,
        reason=reason,
    )
    error_for_ledger = error_msg if terminal_status == ExecutionStatus.FAILED else None
    await execution_ledger_service.update_execution_status(
        db=db,
        execution_id=execution_id,
        status=terminal_status,
        error=error_for_ledger,
        scheduler_name="slurm",
        scheduler_job_id=raw_id,
        workflow_manifest=merged,
    )
    logger.info(
        "event=ledger_slurm_terminal_persisted execution_id=%s slurm_job_id=%s "
        "state=%s source=%s exit_code=%s ledger_status=%s reason=%s",
        execution_id,
        slurm_job_id,
        state,
        source,
        exit_code,
        terminal_ledger,
        reason,
    )
    return {
        "terminal": True,
        "status": terminal_ledger,
        "slurm_state": state,
        "slurm_job_id": slurm_job_id,
        "slurm_exit_code": exit_code,
        "reason": reason,
        "error": error_for_ledger,
    }

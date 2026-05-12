from datetime import UTC, datetime
from typing import Any

BEAMPIPE_RUN_RECORD_KEY = "beampipe_run_record"
_RAW_LINE_MAX = 512
_GRAPH_ERROR_UIDS_MAX = 20


def _now_iso_z() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def _trim_raw(raw: str | None) -> str | None:
    if not raw:
        return None
    s = raw.strip()
    if len(s) <= _RAW_LINE_MAX:
        return s
    return s[: _RAW_LINE_MAX] + "..."


def _observation_dict(
    *,
    state: str,
    exit_code: Any,
    source: str,
    raw_line: str | None,
) -> dict[str, Any]:
    observed_at = _now_iso_z()
    out: dict[str, Any] = {
        "state": state,
        "exit_code": exit_code,
        "source": source,
        "observed_at": observed_at,
    }
    tr = _trim_raw(raw_line)
    if tr:
        out["raw"] = tr
    return out


def _merge_paths(slurm: dict[str, Any], paths: dict[str, str] | None) -> None:
    if not paths:
        return
    merged = dict(slurm.get("paths") or {})
    for k, v in paths.items():
        if v:
            merged[str(k)] = str(v).strip()
    if merged:
        slurm["paths"] = merged


def merge_slurm_poll_into_manifest(
    existing_manifest: dict[str, Any] | None,
    *,
    composite_scheduler_job_id: str,
    slurm_job_id: str,
    state: str,
    source: str,
    exit_code: Any,
    raw_line: str | None,
    record_terminal: bool,
    terminal_ledger_status: str | None,
    remote_session_dir: str | None = None,
    diagnostics: dict[str, Any] | None = None,
    reason: str | None = None,
) -> dict[str, Any]:
    base: dict[str, Any] = dict(existing_manifest) if isinstance(existing_manifest, dict) else {}
    rr: dict[str, Any] = dict(base.get(BEAMPIPE_RUN_RECORD_KEY) or {})
    slurm: dict[str, Any] = dict(rr.get("slurm") or {})
    slurm.pop("observations", None)

    slurm["slurm_job_id"] = str(slurm_job_id)
    slurm["composite_scheduler_job_id"] = str(composite_scheduler_job_id)

    obs = _observation_dict(
        state=state,
        exit_code=exit_code,
        source=source,
        raw_line=raw_line,
    )
    slurm["last_observation"] = obs

    if remote_session_dir and str(remote_session_dir).strip():
        _merge_paths(slurm, {"session_dir": str(remote_session_dir).strip()})

    if record_terminal and terminal_ledger_status:
        comp = str(composite_scheduler_job_id)
        existing_terminal = slurm.get("terminal")
        frozen = (
            isinstance(existing_terminal, dict)
            and existing_terminal.get("ledger_status") == terminal_ledger_status
            and str(existing_terminal.get("composite_scheduler_job_id") or "") == comp
        )
        if not frozen:
            term: dict[str, Any] = {
                **obs,
                "ledger_status": terminal_ledger_status,
                "composite_scheduler_job_id": comp,
                "slurm_job_id_ref": str(slurm_job_id),
            }
            if reason:
                term["reason"] = str(reason)
            if diagnostics:
                term["diagnostics"] = {k: v for k, v in diagnostics.items() if v is not None}
            slurm["terminal"] = term

    rr["slurm"] = slurm
    out = dict(base)
    out[BEAMPIPE_RUN_RECORD_KEY] = rr
    return out


def merge_slurm_submit_into_manifest(
    existing_manifest: dict[str, Any] | None,
    *,
    session_id: str,
    slurm_job_id: str,
    composite_scheduler_job_id: str,
    login_node: str | None,
    remote_user: str | None,
    paths: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Stamp Slurm submit metadata (after sbatch)."""
    base: dict[str, Any] = dict(existing_manifest) if isinstance(existing_manifest, dict) else {}
    rr: dict[str, Any] = dict(base.get(BEAMPIPE_RUN_RECORD_KEY) or {})
    slurm: dict[str, Any] = dict(rr.get("slurm") or {})
    slurm.pop("observations", None)
    slurm["session_id"] = str(session_id)
    slurm["slurm_job_id"] = str(slurm_job_id)
    slurm["composite_scheduler_job_id"] = str(composite_scheduler_job_id)
    slurm["submitted_at"] = _now_iso_z()
    if login_node:
        slurm["login_node"] = str(login_node).strip()
    if remote_user:
        slurm["remote_user"] = str(remote_user).strip()
    _merge_paths(slurm, paths)
    rr["slurm"] = slurm
    out = dict(base)
    out[BEAMPIPE_RUN_RECORD_KEY] = rr
    return out


def merge_dim_deploy_into_manifest(
    existing_manifest: dict[str, Any] | None,
    *,
    session_id: str,
    dim_rest_base: str,
    verify_ssl: bool,
    operator_urls: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Record successful DIM deploy."""
    base: dict[str, Any] = dict(existing_manifest) if isinstance(existing_manifest, dict) else {}
    rr: dict[str, Any] = dict(base.get(BEAMPIPE_RUN_RECORD_KEY) or {})
    dim: dict[str, Any] = dict(rr.get("dim") or {})
    dim.pop("observations", None)
    dim["session_id"] = str(session_id)
    deploy: dict[str, Any] = dict(dim.get("deploy") or {})
    deploy["deployed_at"] = _now_iso_z()
    deploy["dim_rest_base"] = str(dim_rest_base).rstrip("/")
    deploy["verify_ssl"] = bool(verify_ssl)
    if operator_urls:
        urls = {k: str(v) for k, v in operator_urls.items() if v}
        if urls:
            deploy["operator_urls"] = urls
    dim["deploy"] = deploy
    rr["dim"] = dim
    out = dict(base)
    out[BEAMPIPE_RUN_RECORD_KEY] = rr
    return out


def _normalize_dim_graph_summary(graph: dict[str, Any] | None) -> dict[str, Any] | None:
    if not graph:
        return None
    out: dict[str, Any] = {}
    st = graph.get("status")
    if isinstance(st, str) and st:
        out["status"] = st
    count = graph.get("error_drop_count")
    if count is not None:
        try:
            out["error_drop_count"] = int(count)
        except (TypeError, ValueError):
            pass
    uids = graph.get("error_drop_uids")
    if isinstance(uids, list) and uids:
        clean = [str(u) for u in uids if u is not None][: _GRAPH_ERROR_UIDS_MAX]
        if clean:
            out["error_drop_uids"] = clean
    return out or None


def merge_dim_poll_into_manifest(
    existing_manifest: dict[str, Any] | None,
    *,
    session_id: str,
    session_state: str,
    http_status: int | None,
    record_terminal: bool,
    terminal_ledger_status: str | None,
    error: str | None = None,
    error_drops_count: int | None = None,
    graph: dict[str, Any] | None = None,
) -> dict[str, Any]:
    base: dict[str, Any] = dict(existing_manifest) if isinstance(existing_manifest, dict) else {}
    rr: dict[str, Any] = dict(base.get(BEAMPIPE_RUN_RECORD_KEY) or {})
    dim: dict[str, Any] = dict(rr.get("dim") or {})
    dim.pop("observations", None)
    dim["session_id"] = str(session_id)
    obs: dict[str, Any] = {
        "session_state": str(session_state),
        "http_status": http_status,
        "observed_at": _now_iso_z(),
    }
    dim["last_observation"] = obs

    graph_summary: dict[str, Any] | None = None
    if graph is not None:
        graph_summary = _normalize_dim_graph_summary(graph)
        if graph_summary is not None:
            dim["graph"] = graph_summary

    if record_terminal and terminal_ledger_status:
        term: dict[str, Any] = {
            **obs,
            "ledger_status": terminal_ledger_status,
            "session_id": str(session_id),
        }
        if error is not None:
            term["error"] = _trim_raw(error) or error[:_RAW_LINE_MAX]
        if error_drops_count is not None:
            term["error_drops_count"] = int(error_drops_count)
        if graph_summary is not None:
            term["graph"] = dict(graph_summary)
        existing_terminal = dim.get("terminal")
        frozen = (
            isinstance(existing_terminal, dict)
            and existing_terminal.get("ledger_status") == terminal_ledger_status
            and str(existing_terminal.get("session_id") or "") == str(session_id)
        )
        if not frozen:
            dim["terminal"] = term

    rr["dim"] = dim
    out = dict(base)
    out[BEAMPIPE_RUN_RECORD_KEY] = rr
    return out


def merge_restate_slurm_completion_timeout_into_manifest(
    existing_manifest: dict[str, Any] | None,
    *,
    error: str,
) -> dict[str, Any]:
    base: dict[str, Any] = dict(existing_manifest) if isinstance(existing_manifest, dict) else {}
    rr: dict[str, Any] = dict(base.get(BEAMPIPE_RUN_RECORD_KEY) or {})
    restate: dict[str, Any] = dict(rr.get("restate") or {})
    restate["slurm_completion"] = {
        "reason": "poll_timeout",
        "error": _trim_raw(error) or (error[:1024] + ("..." if len(error) > 1024 else "")),
        "observed_at": _now_iso_z(),
    }
    rr["restate"] = restate
    out = dict(base)
    out[BEAMPIPE_RUN_RECORD_KEY] = rr
    return out


def extract_beampipe_run_record(workflow_manifest: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(workflow_manifest, dict):
        return None
    rr = workflow_manifest.get(BEAMPIPE_RUN_RECORD_KEY)
    return dict(rr) if isinstance(rr, dict) else None


def has_beampipe_run_record(workflow_manifest: dict[str, Any] | None) -> bool:
    return extract_beampipe_run_record(workflow_manifest) is not None


def preserve_run_record_into_manifest(
    new_manifest: dict[str, Any] | None,
    *,
    existing_manifest: dict[str, Any] | None,
) -> dict[str, Any]:
    new_dict: dict[str, Any] = dict(new_manifest) if isinstance(new_manifest, dict) else {}
    existing_rr = extract_beampipe_run_record(existing_manifest)
    if not existing_rr:
        return new_dict
    new_rr_raw = new_dict.get(BEAMPIPE_RUN_RECORD_KEY)
    new_rr: dict[str, Any] = dict(new_rr_raw) if isinstance(new_rr_raw, dict) else {}
    merged = {**existing_rr, **new_rr}
    new_dict[BEAMPIPE_RUN_RECORD_KEY] = merged
    return new_dict


def merge_execution_request_into_run_record(
    existing_manifest: dict[str, Any] | None,
    *,
    sources: list[Any] | None,
) -> dict[str, Any]:
    base: dict[str, Any] = dict(existing_manifest) if isinstance(existing_manifest, dict) else {}
    rr: dict[str, Any] = dict(base.get(BEAMPIPE_RUN_RECORD_KEY) or {})

    captured: list[dict[str, Any]] = []
    for spec in sources or []:
        if isinstance(spec, dict):
            sid = spec.get("source_identifier")
            if not sid:
                continue
            entry: dict[str, Any] = {"source_identifier": str(sid)}
            sbids = spec.get("sbids")
            if isinstance(sbids, list) and sbids:
                entry["sbids"] = [str(s) for s in sbids]
            captured.append(entry)
        else:
            sid = getattr(spec, "source_identifier", None)
            if not sid:
                continue
            entry = {"source_identifier": str(sid)}
            sbids = getattr(spec, "sbids", None)
            if isinstance(sbids, list) and sbids:
                entry["sbids"] = [str(s) for s in sbids]
            captured.append(entry)

    requested: dict[str, Any] = {
        "count": len(captured),
        "source_identifiers": [e["source_identifier"] for e in captured],
        "sources": captured,
        "captured_at": _now_iso_z(),
    }
    existing = rr.get("requested_sources")
    if isinstance(existing, dict):
        same = (
            existing.get("source_identifiers") == requested["source_identifiers"]
            and existing.get("count") == requested["count"]
        )
        if same:
            return base
    rr["requested_sources"] = requested
    out = dict(base)
    out[BEAMPIPE_RUN_RECORD_KEY] = rr
    return out

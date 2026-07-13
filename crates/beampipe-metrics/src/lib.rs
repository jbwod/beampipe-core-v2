mod otel;
pub mod refresh;
pub mod server;
pub mod trace_context;
pub mod tracing_layer;

pub use beampipe_db::test_modules::INTEGRATION_TEST_MODULE_REGEX as INTERNAL_TEST_MODULE_REGEX;
pub use otel::init_if_enabled;
pub use refresh::{is_internal_test_module, refresh_dependencies, refresh_gauges_from_pool};
pub use trace_context::{
    correlation_id_from_payload, correlation_only, extract_parent_context,
    extract_parent_from_traceparent, header_map_from_pairs, inject_current_traceparent,
    inject_into_payload, new_tick_correlation_id, parent_context_from_payload, payload_with_trace,
    set_span_parent_from_payload, sources_attr_value, trace_context_from_http,
    trace_context_from_payload, traceparent_from_payload, worker_role_from_env, TraceContext,
    CORRELATION_ID_KEY, TRACEPARENT_HEADER, TRACEPARENT_KEY,
};

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

static PROMETHEUS: OnceLock<PrometheusHandle> = OnceLock::new();

pub fn init_recorder() {
    PROMETHEUS.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("failed to install prometheus metrics recorder")
    });
    otel::init_if_enabled();
}

pub fn render_prometheus() -> Option<String> {
    PROMETHEUS.get().map(|h| h.render())
}

pub fn record_discovery_outcome(project_module: &str, outcome: &str) {
    counter!(
        "beampipe_discovery_sources_total",
        "project_module" => project_module.to_string(),
        "outcome" => outcome.to_string()
    )
    .increment(1);
}

pub fn record_discovery_tap_skipped(project_module: &str) {
    counter!(
        "beampipe_discovery_tap_skipped_total",
        "project_module" => project_module.to_string()
    )
    .increment(1);
    record_discovery_outcome(project_module, "skipped_tap");
}

pub fn set_workflow_pending_sources(project_module: &str, count: i64) {
    gauge!(
        "beampipe_workflow_pending_sources",
        "project_module" => project_module.to_string()
    )
    .set(count as f64);
}

pub fn set_pending_age_seconds(project_module: &str, age: i64) {
    gauge!(
        "beampipe_pending_age_seconds",
        "project_module" => project_module.to_string()
    )
    .set(age as f64);
}

/// `phase`: `discovering`, `admitting`, or `executing` (see `list_sources_currently_processing`).
pub fn set_source_processing(
    project_module: &str,
    source_identifier: &str,
    phase: &str,
    active: bool,
) {
    gauge!(
        "beampipe_source_processing",
        "project_module" => project_module.to_string(),
        "source_identifier" => source_identifier.to_string(),
        "phase" => phase.to_string()
    )
    .set(if active { 1.0 } else { 0.0 });
}

pub fn set_dependency_up(name: &str, up: bool) {
    gauge!(
        "beampipe_dependency_up",
        "dependency" => name.to_string()
    )
    .set(if up { 1.0 } else { 0.0 });
}

pub fn record_execution_admitted(project_module: &str, skip_reason: &str) {
    counter!(
        "beampipe_execution_admitted_total",
        "project_module" => project_module.to_string(),
        "skip_reason" => skip_reason.to_string()
    )
    .increment(1);
}

pub fn set_jobs_queue_depth(depth: i64) {
    gauge!("beampipe_jobs_queue_depth").set(depth as f64);
}

pub fn set_jobs_running(running: i64) {
    gauge!("beampipe_jobs_running").set(running as f64);
}

pub fn set_slurm_poll_batch_size(size: usize) {
    gauge!("beampipe_slurm_poll_batch_size").set(size as f64);
}

pub fn set_dim_poll_batch_size(size: usize) {
    gauge!("beampipe_dim_poll_batch_size").set(size as f64);
}

pub fn set_jobs_queued_by_kind(kind: &str, count: i64) {
    gauge!(
        "beampipe_jobs_queued",
        "kind" => kind.to_string()
    )
    .set(count as f64);
}

pub fn set_oldest_queued_job_age_seconds(kind: &str, age: i64) {
    gauge!(
        "beampipe_jobs_oldest_queued_age_seconds",
        "kind" => kind.to_string()
    )
    .set(age as f64);
}

pub fn set_executions_active_by_status(status: &str, count: i64) {
    gauge!(
        "beampipe_executions_active",
        "status" => status.to_string()
    )
    .set(count as f64);
}

pub fn set_oldest_active_execution_age_seconds(status: &str, age: i64) {
    gauge!(
        "beampipe_executions_oldest_active_age_seconds",
        "status" => status.to_string()
    )
    .set(age as f64);
}

pub fn set_executions_inflight_by_scheduler(scheduler_name: &str, count: i64) {
    gauge!(
        "beampipe_executions_inflight",
        "scheduler_name" => scheduler_name.to_string()
    )
    .set(count as f64);
}

pub fn set_execution_axis_count(axis: &str, state: &str, count: i64) {
    gauge!(
        "beampipe_executions_by_external_state",
        "axis" => axis.to_string(),
        "state" => state.to_string()
    )
    .set(count as f64);
}

pub fn set_worker_heartbeat_age(worker_id: &str, pool: &str, seconds: i64) {
    gauge!(
        "beampipe_worker_heartbeat_age_seconds",
        "worker_id" => worker_id.to_string(),
        "pool" => pool.to_string()
    )
    .set(seconds.max(0) as f64);
}

pub fn set_worker_active_leases(worker_id: &str, pool: &str, count: i64) {
    gauge!(
        "beampipe_worker_active_leases",
        "worker_id" => worker_id.to_string(),
        "pool" => pool.to_string()
    )
    .set(count.max(0) as f64);
}

pub fn set_reconciliation_risk_count(count: i64) {
    gauge!("beampipe_reconciliation_risk_executions").set(count.max(0) as f64);
}

pub fn set_execution_retry_total(count: i64) {
    gauge!("beampipe_execution_retries_total").set(count.max(0) as f64);
}

pub fn record_reconciliation_result(component: &str, result: &str) {
    counter!(
        "beampipe_reconciliation_total",
        "component" => component.to_string(),
        "result" => result.to_string()
    )
    .increment(1);
}

pub fn record_slurm_poll_error(reason: &str) {
    counter!(
        "beampipe_slurm_poll_errors_total",
        "reason" => reason.to_string()
    )
    .increment(1);
}

pub fn set_slurm_ssh_sessions_active(login_node: &str, count: usize) {
    gauge!(
        "beampipe_slurm_ssh_sessions_active",
        "login_node" => login_node.to_string()
    )
    .set(count as f64);
}

pub fn set_slurm_ssh_configured(configured: bool) {
    gauge!("beampipe_slurm_ssh_configured").set(if configured { 1.0 } else { 0.0 });
}

pub fn record_security_check_failure(check: &str) {
    counter!(
        "beampipe_security_check_failures",
        "check" => check.to_string()
    )
    .increment(1);
}

pub fn record_secret_ref_configured(kind: &str) {
    counter!(
        "beampipe_secret_refs_configured",
        "kind" => kind.to_string()
    )
    .increment(1);
}

pub fn record_unsafe_inline_secret_rejected(surface: &str) {
    counter!(
        "beampipe_unsafe_inline_secret_rejected_total",
        "surface" => surface.to_string()
    )
    .increment(1);
}

pub fn record_execute_terminal(project_module: &str, status: &str) {
    counter!(
        "beampipe_execute_terminal_total",
        "project_module" => project_module.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

pub fn record_job(kind: &str, status: &str) {
    counter!(
        "beampipe_jobs_total",
        "kind" => kind.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

pub fn record_job_duration(kind: &str, seconds: f64) {
    histogram!(
        "beampipe_job_duration_seconds",
        "kind" => kind.to_string()
    )
    .record(seconds);
}

pub fn record_discovery_duration(project_module: &str, seconds: f64) {
    histogram!(
        "beampipe_discovery_duration_seconds",
        "project_module" => project_module.to_string()
    )
    .record(seconds);
}

pub fn record_scheduler_tick_duration(tick_kind: &str, seconds: f64) {
    histogram!(
        "beampipe_scheduler_tick_duration_seconds",
        "tick_kind" => tick_kind.to_string()
    )
    .record(seconds);
}

pub fn record_execute_duration(phase: &str, seconds: f64) {
    histogram!(
        "beampipe_execute_duration_seconds",
        "phase" => phase.to_string()
    )
    .record(seconds);
}

pub fn record_api_request(method: &str, route: &str, status: u16) {
    counter!(
        "beampipe_api_requests_total",
        "method" => method.to_string(),
        "route" => route.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

pub fn record_api_request_duration(method: &str, route: &str, status: u16, seconds: f64) {
    record_api_request(method, route, status);
    histogram!(
        "beampipe_api_request_duration_seconds",
        "method" => method.to_string(),
        "route" => route.to_string(),
        "status" => status.to_string()
    )
    .record(seconds);
}

pub fn record_discovery_batch_stats(
    project_module: &str,
    changed: usize,
    unchanged: usize,
    no_datasets: usize,
    errors: usize,
    timeouts: usize,
) {
    for _ in 0..changed {
        record_discovery_outcome(project_module, "changed");
    }
    for _ in 0..unchanged {
        record_discovery_outcome(project_module, "unchanged");
    }
    for _ in 0..no_datasets {
        record_discovery_outcome(project_module, "no_datasets");
    }
    for _ in 0..errors {
        record_discovery_outcome(project_module, "error");
    }
    for _ in 0..timeouts {
        record_discovery_outcome(project_module, "timeout");
    }
}

pub fn record_scheduler_skip_reasons(
    project_module: &str,
    reason_counts: &std::collections::BTreeMap<beampipe_domain::SkipReason, u32>,
) {
    for (reason, count) in reason_counts {
        for _ in 0..*count {
            record_execution_admitted(project_module, reason.as_str());
        }
    }
}

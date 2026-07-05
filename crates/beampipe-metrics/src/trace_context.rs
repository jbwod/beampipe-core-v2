//! W3C trace propagation and correlation_id helpers for HTTP → job queue continuity.

use opentelemetry::global;
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

pub const TRACEPARENT_HEADER: &str = "traceparent";
pub const CORRELATION_ID_KEY: &str = "correlation_id";
pub const TRACEPARENT_KEY: &str = "traceparent";

/// Optional trace fields carried in job payloads.
#[derive(Debug, Clone, Default)]
pub struct TraceContext {
    pub correlation_id: Option<String>,
    pub traceparent: Option<String>,
}

struct HeaderExtractor<'a>(&'a HashMap<String, String>);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

struct HeaderInjector<'a>(&'a mut HashMap<String, String>);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_ascii_lowercase(), value);
    }
}

/// Build lowercase header map for OTEL extraction.
pub fn header_map_from_pairs<'a, I>(headers: I) -> HashMap<String, String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    headers
        .into_iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
        .collect()
}

/// Extract OTEL parent context from W3C `traceparent` (and optional `tracestate`).
pub fn extract_parent_context(headers: &HashMap<String, String>) -> opentelemetry::Context {
    let propagator = TraceContextPropagator::new();
    propagator.extract_with_context(&opentelemetry::Context::new(), &HeaderExtractor(headers))
}

/// Serialize the active span's trace context as a W3C `traceparent` header value.
pub fn inject_current_traceparent() -> Option<String> {
    let cx = Span::current().context();
    let otel_span = cx.span();
    let span_context = otel_span.span_context();
    if !span_context.is_valid() {
        return None;
    }
    let mut headers = HashMap::new();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderInjector(&mut headers));
    });
    headers.get(TRACEPARENT_HEADER).cloned()
}

pub fn correlation_id_from_payload(payload: &Value) -> Option<&str> {
    payload
        .get(CORRELATION_ID_KEY)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

pub fn traceparent_from_payload(payload: &Value) -> Option<&str> {
    payload
        .get(TRACEPARENT_KEY)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// Merge correlation_id and traceparent into a job payload (additive, non-destructive).
pub fn inject_into_payload(mut payload: Value, ctx: &TraceContext) -> Value {
    let Some(obj) = payload.as_object_mut() else {
        return payload;
    };
    if let Some(id) = &ctx.correlation_id {
        obj.entry(CORRELATION_ID_KEY).or_insert_with(|| json!(id));
    }
    if let Some(tp) = &ctx.traceparent {
        obj.entry(TRACEPARENT_KEY).or_insert_with(|| json!(tp));
    }
    payload
}

/// Build trace context from HTTP correlation id and optional incoming traceparent header.
pub fn trace_context_from_http(correlation_id: &str, traceparent: Option<&str>) -> TraceContext {
    TraceContext {
        correlation_id: Some(correlation_id.to_string()),
        traceparent: traceparent.map(str::to_string),
    }
}

/// Trace context with only a correlation id (scheduler/worker enqueue paths).
pub fn correlation_only(id: impl Into<String>) -> TraceContext {
    TraceContext {
        correlation_id: Some(id.into()),
        traceparent: None,
    }
}

/// Merge trace fields into a job payload object.
pub fn payload_with_trace(base: Value, ctx: &TraceContext) -> Value {
    inject_into_payload(base, ctx)
}

/// Derive trace context from a job payload with a stable fallback correlation id.
pub fn trace_context_from_payload(payload: &Value, fallback_correlation_id: &str) -> TraceContext {
    TraceContext {
        correlation_id: Some(
            correlation_id_from_payload(payload)
                .unwrap_or(fallback_correlation_id)
                .to_string(),
        ),
        traceparent: traceparent_from_payload(payload).map(str::to_string),
    }
}

/// New tick-level correlation for scheduler-initiated work.
pub fn new_tick_correlation_id() -> String {
    Uuid::now_v7().to_string()
}

/// Extract an OTEL parent from a W3C `traceparent` header value.
pub fn extract_parent_from_traceparent(traceparent: &str) -> opentelemetry::Context {
    let headers = header_map_from_pairs([(TRACEPARENT_HEADER, traceparent)]);
    extract_parent_context(&headers)
}

/// Extract an OTEL parent from a job payload.
pub fn parent_context_from_payload(payload: &Value) -> Option<opentelemetry::Context> {
    traceparent_from_payload(payload).map(extract_parent_from_traceparent)
}

/// Attach the payload's OTEL parent to a known span.
pub fn set_span_parent_from_payload(span: &Span, payload: &Value) {
    if let Some(parent) = parent_context_from_payload(payload) {
        span.set_parent(parent);
    }
}

/// Worker role label for span attributes (`api`, `scheduler`, or `worker`).
pub fn worker_role_from_env() -> &'static str {
    match std::env::var("BEAMPIPE_PROCESS_ROLE")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "api" => "api",
        "scheduler" => "scheduler",
        _ => "worker",
    }
}

/// Cap source list for span attribute size.
pub fn sources_attr_value(sources: &[String]) -> String {
    const MAX: usize = 8;
    if sources.len() <= MAX {
        return sources.join(",");
    }
    format!("{},…+{}", sources[..MAX].join(","), sources.len() - MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_into_payload_merges_fields() {
        let payload = inject_into_payload(
            json!({"execution_id": "abc"}),
            &TraceContext {
                correlation_id: Some("corr-1".into()),
                traceparent: Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into()),
            },
        );
        assert_eq!(payload["correlation_id"], "corr-1");
        assert!(payload["traceparent"].as_str().unwrap().starts_with("00-"));
        assert_eq!(payload["execution_id"], "abc");
    }

    #[test]
    fn inject_does_not_overwrite_existing_correlation() {
        let payload = inject_into_payload(
            json!({"correlation_id": "existing"}),
            &TraceContext {
                correlation_id: Some("new".into()),
                traceparent: None,
            },
        );
        assert_eq!(payload["correlation_id"], "existing");
    }

    #[test]
    fn correlation_id_from_payload_round_trip() {
        let p = json!({"correlation_id": "x"});
        assert_eq!(correlation_id_from_payload(&p), Some("x"));
    }
}

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Request, Response},
    middleware::Next,
};
use beampipe_metrics::trace_context::{extract_parent_from_traceparent, TRACEPARENT_HEADER};
use std::sync::Arc;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Clone, Debug)]
pub struct RequestContext {
    pub request_id: String,
    pub traceparent: Option<String>,
}

impl RequestContext {
    pub fn correlation_id(&self) -> &str {
        &self.request_id
    }

    pub fn trace_context(&self) -> beampipe_metrics::TraceContext {
        let traceparent =
            beampipe_metrics::inject_current_traceparent().or_else(|| self.traceparent.clone());
        beampipe_metrics::trace_context_from_http(&self.request_id, traceparent.as_deref())
    }
}

pub fn request_id_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::now_v7().to_string())
}

fn traceparent_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub async fn correlation_middleware(mut request: Request<Body>, next: Next) -> Response<Body> {
    let request_id = request_id_from_headers(request.headers());
    let traceparent = traceparent_from_headers(request.headers());
    let ctx = Arc::new(RequestContext {
        request_id: request_id.clone(),
        traceparent: traceparent.clone(),
    });
    request.extensions_mut().insert(ctx);

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %request.method(),
        uri = %request.uri().path()
    );
    if let Some(tp) = traceparent.as_deref() {
        span.set_parent(extract_parent_from_traceparent(tp));
    }

    let mut response = async move { next.run(request).await }
        .instrument(span)
        .await;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

use axum::{
    body::Body,
    http::{HeaderMap, Request},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use tracing::Instrument;
use uuid::Uuid;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const TRACEPARENT_HEADER: &str = "traceparent";

#[derive(Clone, Debug)]
pub struct RequestContext {
    pub request_id: String,
    pub traceparent: Option<String>,
}

impl RequestContext {
    pub fn correlation_id(&self) -> &str {
        &self.request_id
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

pub async fn correlation_middleware(mut request: Request<Body>, next: Next) -> Response {
    let request_id = request_id_from_headers(request.headers());
    let traceparent = request
        .headers()
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let ctx = Arc::new(RequestContext {
        request_id: request_id.clone(),
        traceparent,
    });
    request.extensions_mut().insert(ctx);
    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %request.method(),
        uri = %request.uri().path()
    );
    async move { next.run(request).await }
        .instrument(span)
        .await
}

#[allow(dead_code)]
pub fn extension_from_request(request: &Request<Body>) -> Option<Arc<RequestContext>> {
    request.extensions().get::<Arc<RequestContext>>().cloned()
}

use async_trait::async_trait;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, time::Duration};
use thiserror::Error;

pub mod casda_datalink;
pub mod casda_staging;
pub mod tap_async;
pub mod tap_health;
pub mod votable;
pub use casda_datalink::parse_casda_datalink;
pub use casda_staging::{extract_scan_id, parse_eval_job_results, parse_job_results};
pub use tap_health::{
    all_reachable, probe_tap_health, unreachable_adapters, TapEndpointStatus, TapHealthReport,
};
pub use votable::parse_votable_xml;

pub type TapRow = Map<String, Value>;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("transient TAP failure: {0}")]
    Transient(String),
    #[error("permanent TAP failure: {0}")]
    Permanent(String),
    #[error("TAP query returned no rows")]
    EmptyResult,
    #[error("invalid TAP row shape: {0}")]
    InvalidRowShape(String),
    #[error("TAP timeout")]
    Timeout,
    #[error("adapter not implemented: {0}")]
    NotImplemented(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapQueryRequest {
    pub adql: String,
}

impl TapQueryRequest {
    pub fn new(adql: impl Into<String>) -> Self {
        Self { adql: adql.into() }
    }

    /// UWS sync GET (CASDA and others) — uppercase parameter names.
    pub fn params(&self) -> [(&str, &str); 3] {
        [
            ("REQUEST", "doQuery"),
            ("LANG", "ADQL"),
            ("QUERY", self.adql.as_str()),
        ]
    }

    /// TAP sync POST (VizieR) — lowercase names required by CDS.
    pub fn post_params(&self) -> [(&str, &str); 4] {
        [
            ("request", "doQuery"),
            ("lang", "ADQL"),
            ("query", self.adql.as_str()),
            ("format", "votable"),
        ]
    }
}

#[async_trait]
pub trait TapClient: Send + Sync {
    async fn query_rows(&self, adql: &str) -> Result<Vec<TapRow>, AdapterError>;

    async fn health(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}

#[async_trait]
pub trait TapAdapter: Send + Sync {
    async fn query(&self, adql: &str) -> Result<Value, AdapterError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapMode {
    SyncGet,
    SyncPost,
    AsyncJob,
}

#[derive(Debug, Clone)]
pub struct HttpTapAdapter {
    pub base_url: String,
    pub client: reqwest::Client,
    pub timeout: Duration,
    pub retries: u32,
    pub mode: TapMode,
}

impl HttpTapAdapter {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            timeout: Duration::from_secs(30),
            retries: 1,
            mode: TapMode::SyncGet,
        }
    }

    pub fn with_policy(mut self, timeout: Duration, retries: u32) -> Self {
        self.timeout = timeout;
        self.retries = retries;
        self
    }

    pub fn with_mode(mut self, mode: TapMode) -> Self {
        self.mode = mode;
        self
    }

    async fn query_rows_sync_get(&self, adql: &str) -> Result<Vec<TapRow>, AdapterError> {
        let request = TapQueryRequest::new(adql);
        let mut last_error = None;
        for _ in 0..=self.retries {
            let response = self
                .client
                .get(&self.base_url)
                .query(&request.params())
                .timeout(self.timeout)
                .send()
                .await;
            match response {
                Ok(response) => return parse_tap_http_response(response).await,
                Err(err) if err.is_timeout() => last_error = Some(AdapterError::Timeout),
                Err(err) if err.is_connect() || err.is_request() => {
                    last_error = Some(AdapterError::Transient(err.to_string()))
                }
                Err(err) => last_error = Some(AdapterError::Http(err)),
            }
        }
        Err(last_error.unwrap_or_else(|| AdapterError::Transient("request failed".into())))
    }

    async fn query_rows_sync_post(&self, adql: &str) -> Result<Vec<TapRow>, AdapterError> {
        let request = TapQueryRequest::new(adql);
        let mut last_error = None;
        for _ in 0..=self.retries {
            let response = self
                .client
                .post(&self.base_url)
                .form(&request.post_params())
                .timeout(self.timeout)
                .send()
                .await;
            match response {
                Ok(response) => return parse_tap_http_response(response).await,
                Err(err) if err.is_timeout() => last_error = Some(AdapterError::Timeout),
                Err(err) if err.is_connect() || err.is_request() => {
                    last_error = Some(AdapterError::Transient(err.to_string()))
                }
                Err(err) => last_error = Some(AdapterError::Http(err)),
            }
        }
        Err(last_error.unwrap_or_else(|| AdapterError::Transient("request failed".into())))
    }
}

async fn parse_tap_http_response(response: reqwest::Response) -> Result<Vec<TapRow>, AdapterError> {
    let response = response.error_for_status()?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = response.text().await?;
    if content_type.contains("json")
        || text.trim_start().starts_with('{')
        || text.trim_start().starts_with('[')
    {
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| AdapterError::InvalidRowShape(e.to_string()))?;
        return rows_from_json(value);
    }
    if content_type.contains("xml") || text.trim_start().starts_with("<?xml") {
        return parse_votable_xml(&text);
    }
    Err(AdapterError::InvalidRowShape(
        "unsupported TAP response content type".into(),
    ))
}

#[async_trait]
impl TapClient for HttpTapAdapter {
    async fn query_rows(&self, adql: &str) -> Result<Vec<TapRow>, AdapterError> {
        if self.mode == TapMode::AsyncJob {
            let mut last_error = None;
            for _ in 0..=self.retries {
                match tap_async::query_rows_async(&self.client, &self.base_url, adql, self.timeout)
                    .await
                {
                    Ok(rows) => return Ok(rows),
                    Err(AdapterError::Timeout) => last_error = Some(AdapterError::Timeout),
                    Err(err @ AdapterError::Transient(_)) => last_error = Some(err),
                    Err(err) => return Err(err),
                }
            }
            return Err(
                last_error.unwrap_or_else(|| AdapterError::Transient("request failed".into()))
            );
        }
        match self.mode {
            TapMode::SyncPost => self.query_rows_sync_post(adql).await,
            TapMode::SyncGet => self.query_rows_sync_get(adql).await,
            TapMode::AsyncJob => unreachable!("handled above"),
        }
    }
}

#[async_trait]
impl TapAdapter for HttpTapAdapter {
    async fn query(&self, adql: &str) -> Result<Value, AdapterError> {
        Ok(Value::Array(
            self.query_rows(adql)
                .await?
                .into_iter()
                .map(Value::Object)
                .collect(),
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct MockTapClient {
    rows_by_query_name: BTreeMap<String, Vec<TapRow>>,
}

impl MockTapClient {
    pub fn with_rows(query_name: impl Into<String>, rows: Vec<Value>) -> Self {
        let mut rows_by_query_name = BTreeMap::new();
        rows_by_query_name.insert(
            query_name.into(),
            rows.into_iter()
                .filter_map(|v| v.as_object().cloned())
                .collect(),
        );
        Self { rows_by_query_name }
    }

    pub fn insert_rows(&mut self, query_name: impl Into<String>, rows: Vec<Value>) {
        self.rows_by_query_name.insert(
            query_name.into(),
            rows.into_iter()
                .filter_map(|v| v.as_object().cloned())
                .collect(),
        );
    }
}

#[async_trait]
impl TapClient for MockTapClient {
    async fn query_rows(&self, adql: &str) -> Result<Vec<TapRow>, AdapterError> {
        for (name, rows) in &self.rows_by_query_name {
            if adql.contains(name) {
                return Ok(rows.clone());
            }
        }
        Ok(Vec::new())
    }
}

pub fn normalize_casda_tap_url(base_url: impl Into<String>) -> String {
    let url = base_url.into();
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/tap/sync") {
        url
    } else if trimmed.ends_with("/tap") {
        format!("{trimmed}/sync")
    } else {
        url
    }
}

pub fn casda_tap(base_url: impl Into<String>) -> HttpTapAdapter {
    HttpTapAdapter::new(normalize_casda_tap_url(base_url))
}

pub fn normalize_vizier_tap_url(base_url: impl Into<String>) -> String {
    let mut url = base_url.into();
    if url.starts_with("http://") {
        url = url.replacen("http://", "https://", 1);
    }
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/sync") {
        trimmed.to_string()
    } else if trimmed.ends_with("/tap") {
        format!("{trimmed}/sync")
    } else {
        trimmed.to_string()
    }
}

pub fn vizier_tap(base_url: impl Into<String>) -> HttpTapAdapter {
    HttpTapAdapter::new(normalize_vizier_tap_url(base_url)).with_mode(TapMode::SyncPost)
}

pub fn rows_from_json(value: Value) -> Result<Vec<TapRow>, AdapterError> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                item.as_object().cloned().ok_or_else(|| {
                    AdapterError::InvalidRowShape("array item is not an object".into())
                })
            })
            .collect(),
        Value::Object(mut obj) => {
            for key in ["rows", "data", "results", "items"] {
                if let Some(Value::Array(items)) = obj.remove(key) {
                    return rows_from_json(Value::Array(items));
                }
            }
            if obj.contains_key("metadata") && obj.contains_key("data") {
                return Err(AdapterError::InvalidRowShape(
                    "table metadata/data object shape is unsupported".into(),
                ));
            }
            Ok(vec![obj])
        }
        _ => Err(AdapterError::InvalidRowShape(
            "TAP response must be an object or array".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vizier_tap_url_uses_sync_post_endpoint() {
        assert_eq!(
            normalize_vizier_tap_url("http://tapvizier.cds.unistra.fr/TAPVizieR/tap"),
            "https://tapvizier.cds.unistra.fr/TAPVizieR/tap/sync"
        );
    }

    #[test]
    fn casda_tap_url_uses_sync_endpoint() {
        assert_eq!(
            normalize_casda_tap_url("https://casda.csiro.au/casda_vo_tools/tap"),
            "https://casda.csiro.au/casda_vo_tools/tap/sync"
        );
        assert_eq!(
            normalize_casda_tap_url("https://casda.csiro.au/casda_vo_tools/tap/sync"),
            "https://casda.csiro.au/casda_vo_tools/tap/sync"
        );
    }

    #[test]
    fn tap_query_params_include_adql() {
        let request = TapQueryRequest::new("SELECT * FROM t WHERE name = 'A''B'");
        assert_eq!(request.params()[2].1, "SELECT * FROM t WHERE name = 'A''B'");
    }

    #[test]
    fn rows_parse_json_arrays_and_wrapped_rows() {
        assert_eq!(rows_from_json(json!([{"a": 1}])).unwrap().len(), 1);
        assert_eq!(
            rows_from_json(json!({"rows": [{"a": 1}]})).unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn mock_tap_matches_query_fragments() {
        let tap = MockTapClient::with_rows("ivoa.obscore", vec![serde_json::json!({"a": 1})]);
        let rows = tap.query_rows("SELECT * FROM ivoa.obscore").await.unwrap();
        assert_eq!(rows[0]["a"], 1);
    }
}

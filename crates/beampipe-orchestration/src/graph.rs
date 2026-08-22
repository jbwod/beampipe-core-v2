use crate::OrchestrationError;
use beampipe_project::{GraphConfig, ProjectConfig};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const GRAPH_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const GRAPH_MAX_BYTES: usize = 16 * 1024 * 1024;

pub async fn resolve_graph(config: &ProjectConfig) -> Result<Value, OrchestrationError> {
    let Some(graph) = config.graph.as_ref() else {
        return Err(OrchestrationError::Backend(
            "project config has no graph.url or graph.path".into(),
        ));
    };
    resolve_graph_config(graph).await
}

pub async fn resolve_graph_config(graph: &GraphConfig) -> Result<Value, OrchestrationError> {
    resolve_graph_config_with_limits(graph, GRAPH_FETCH_TIMEOUT, GRAPH_MAX_BYTES).await
}

async fn resolve_graph_config_with_limits(
    graph: &GraphConfig,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Value, OrchestrationError> {
    let bytes = if let Some(url) = graph.url.as_deref().filter(|url| !url.trim().is_empty()) {
        fetch_graph(url, timeout, max_bytes).await?
    } else if let Some(path) = graph.path.as_deref().filter(|path| !path.trim().is_empty()) {
        read_graph(Path::new(path), max_bytes).await?
    } else {
        return Err(OrchestrationError::Backend(
            "graph must specify url or path".into(),
        ));
    };

    verify_graph_digest(graph.sha256.as_deref(), &bytes)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        OrchestrationError::Backend(format!("graph contains invalid JSON: {error}"))
    })
}

async fn fetch_graph(
    url: &str,
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, OrchestrationError> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| OrchestrationError::Backend(error.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| OrchestrationError::Backend(format!("graph fetch failed: {error}")))?;
    if !response.status().is_success() {
        return Err(OrchestrationError::Backend(format!(
            "graph fetch failed: HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(graph_too_large(max_bytes));
    }

    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(max_bytes as u64) as usize,
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| OrchestrationError::Backend(format!("graph fetch failed: {error}")))?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(graph_too_large(max_bytes));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_graph(path: &Path, max_bytes: usize) -> Result<Vec<u8>, OrchestrationError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| OrchestrationError::Backend(format!("graph read failed: {error}")))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| OrchestrationError::Backend(format!("graph read failed: {error}")))?;
    if bytes.len() > max_bytes {
        return Err(graph_too_large(max_bytes));
    }
    Ok(bytes)
}

fn verify_graph_digest(expected: Option<&str>, bytes: &[u8]) -> Result<(), OrchestrationError> {
    let expected = expected
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let Some(expected) = expected else {
        return Err(OrchestrationError::Backend(
            "graph.sha256 must contain exactly 64 hexadecimal characters".into(),
        ));
    };
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(OrchestrationError::Backend(format!(
            "graph SHA-256 mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn graph_too_large(max_bytes: usize) -> OrchestrationError {
    OrchestrationError::Backend(format!(
        "graph exceeds the maximum size of {max_bytes} bytes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn remote_graph(url: String, bytes: &[u8]) -> GraphConfig {
        GraphConfig {
            url: Some(url),
            path: None,
            sha256: Some(digest(bytes)),
        }
    }

    #[tokio::test]
    async fn resolves_a_bounded_graph_with_the_expected_digest() {
        let body = br#"{"nodeDataArray":[]}"#;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let graph = remote_graph(server.uri(), body);
        let value = resolve_graph_config_with_limits(&graph, Duration::from_secs(1), 1024)
            .await
            .unwrap();
        assert_eq!(value["nodeDataArray"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn rejects_digest_mismatches() {
        let body = br#"{"nodeDataArray":[]}"#;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;
        let mut graph = remote_graph(server.uri(), body);
        graph.sha256 = Some("0".repeat(64));

        let error = resolve_graph_config_with_limits(&graph, Duration::from_secs(1), 1024)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"));
    }

    #[tokio::test]
    async fn rejects_remote_and_local_graphs_over_the_limit() {
        let body = br#"{"nodeDataArray":[]}"#;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;
        let graph = remote_graph(server.uri(), body);
        assert!(
            resolve_graph_config_with_limits(&graph, Duration::from_secs(1), 4)
                .await
                .unwrap_err()
                .to_string()
                .contains("maximum size")
        );

        let directory = tempdir().unwrap();
        let path = directory.path().join("graph.json");
        fs::write(&path, body).unwrap();
        let local = GraphConfig {
            url: None,
            path: Some(path.display().to_string()),
            sha256: Some(digest(body)),
        };
        assert!(
            resolve_graph_config_with_limits(&local, Duration::from_secs(1), 4)
                .await
                .unwrap_err()
                .to_string()
                .contains("maximum size")
        );
    }

    #[tokio::test]
    async fn times_out_slow_graph_fetches() {
        let body = br#"{"nodeDataArray":[]}"#;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;
        let graph = remote_graph(server.uri(), body);

        let error = resolve_graph_config_with_limits(&graph, Duration::from_millis(10), 1024)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("graph fetch failed"));
    }
}

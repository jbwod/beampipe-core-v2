use crate::OrchestrationError;
use beampipe_project::{GraphConfig, ProjectConfig};
use serde_json::Value;
use std::path::Path;

pub async fn resolve_graph(config: &ProjectConfig) -> Result<Value, OrchestrationError> {
    let Some(graph) = config.graph.as_ref() else {
        return Err(OrchestrationError::Backend(
            "project config has no graph.url or graph.path".into(),
        ));
    };
    resolve_graph_config(graph).await
}

pub async fn resolve_graph_config(graph: &GraphConfig) -> Result<Value, OrchestrationError> {
    if let Some(url) = graph.url.as_deref().filter(|u| !u.trim().is_empty()) {
        let client = reqwest::Client::new();
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(OrchestrationError::Backend(format!(
                "graph fetch failed: HTTP {}",
                resp.status()
            )));
        }
        resp.json()
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))
    } else if let Some(path) = graph.path.as_deref().filter(|p| !p.trim().is_empty()) {
        let bytes = tokio::fs::read(Path::new(path))
            .await
            .map_err(|e| OrchestrationError::Backend(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| OrchestrationError::Backend(e.to_string()))
    } else {
        Err(OrchestrationError::Backend(
            "graph must specify url or path".into(),
        ))
    }
}

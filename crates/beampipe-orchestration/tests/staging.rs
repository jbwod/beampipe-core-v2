use beampipe_orchestration::{PassThroughStagingClient, StagingClient};
use serde_json::json;

#[tokio::test]
async fn pass_through_staging_returns_all_records() {
    let client = PassThroughStagingClient;
    let metadata = vec![json!({"sbid": "1"}), json!({"sbid": "2"})];
    let outcome = client.stage(&metadata).await.unwrap();
    assert_eq!(outcome.metadata.len(), 2);
    assert!(outcome.skipped_sbids.is_empty());
}

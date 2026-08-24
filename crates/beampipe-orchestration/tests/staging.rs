use beampipe_orchestration::{PassThroughStagingClient, StagingClient};
use serde_json::json;

#[tokio::test]
async fn pass_through_staging_returns_all_records() {
    let client = PassThroughStagingClient;
    let metadata = vec![
        json!({"group_key": "1", "record_id": "a"}),
        json!({"group_key": "2", "record_id": "b"}),
    ];
    let outcome = client.stage(&metadata).await.unwrap();
    assert_eq!(outcome.metadata.len(), 2);
    assert!(outcome.skipped_groups.is_empty());
}

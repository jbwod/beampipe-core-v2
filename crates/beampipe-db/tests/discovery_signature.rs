use beampipe_db::{connect, migrate, repo};
use beampipe_domain::discovery::{DiscoverySourceResult, SignatureOptions};
use serde_json::json;
use uuid::Uuid;

async fn test_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = connect(&url).await.ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

async fn teardown_test_module(pool: &sqlx::PgPool, module: &str) {
    let _ = repo::delete_all_sources_for_project_module(pool, module).await;
}

async fn claim_source(pool: &sqlx::PgPool, module: &str, source: &str) -> String {
    repo::mark_sources_for_rediscovery(pool, module, Some(&[source.to_string()]))
        .await
        .unwrap();
    let (token, rows) = repo::claim_source_rows_for_discovery(pool, Some(module), 24, 10, 180)
        .await
        .unwrap();
    assert!(token.is_some(), "claim failed for {module}/{source}");
    assert!(rows.iter().any(|r| r.1 == source));
    token.unwrap()
}

#[tokio::test]
async fn discovery_signature_unchanged_skips_pending() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("sig_test_{}", Uuid::now_v7());
    let source = "src-sig-1".to_string();
    repo::upsert_source(&pool, &module, &source, true)
        .await
        .unwrap();

    let metadata = vec![json!({
        "sbid": "123",
        "dataset_id": "HIPASSJ0001-00.ms",
        "visibility_filename": "HIPASSJ0001-00.ms",
        "scan_id": "9"
    })];
    let flags = json!({"ra_dec_vsys_complete": true});
    let signature = SignatureOptions::default();

    let claim1 = claim_source(&pool, &module, &source).await;
    let stats = repo::persist_discovery_results(
        &pool,
        &module,
        &claim1,
        &[DiscoverySourceResult::HasMetadata {
            source_identifier: source.clone(),
            metadata: metadata.clone(),
            discovery_flags: flags.clone(),
            duration_ms: None,
        }],
        None,
    )
    .await
    .unwrap();
    assert_eq!(stats.changed_count, 1);
    repo::release_discovery_claim(&pool, &module, std::slice::from_ref(&source), &claim1)
        .await
        .unwrap();

    let pending: (bool,) = sqlx::query_as(
        "SELECT workflow_run_pending FROM source_registry WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(&module)
    .bind(&source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(pending.0);

    let claim2 = claim_source(&pool, &module, &source).await;
    let stats2 = repo::persist_discovery_results(
        &pool,
        &module,
        &claim2,
        &[DiscoverySourceResult::HasMetadata {
            source_identifier: source.clone(),
            metadata,
            discovery_flags: flags,
            duration_ms: None,
        }],
        None,
    )
    .await
    .unwrap();
    assert_eq!(stats2.unchanged_count, 1);
    repo::release_discovery_claim(&pool, &module, std::slice::from_ref(&source), &claim2)
        .await
        .unwrap();

    let pending2: (bool,) = sqlx::query_as(
        "SELECT workflow_run_pending FROM source_registry WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(&module)
    .bind(&source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        pending2.0,
        "unchanged re-discovery should not clear existing workflow_run_pending"
    );

    let claim3 = claim_source(&pool, &module, &source).await;
    let stats3 = repo::persist_discovery_results(
        &pool,
        &module,
        &claim3,
        &[DiscoverySourceResult::Unchanged {
            source_identifier: source.clone(),
            duration_ms: Some(0),
        }],
        None,
    )
    .await
    .unwrap();
    assert_eq!(stats3.unchanged_count, 1);
    repo::release_discovery_claim(&pool, &module, std::slice::from_ref(&source), &claim3)
        .await
        .unwrap();

    let _ = signature;
    teardown_test_module(&pool, &module).await;
}

#[tokio::test]
async fn failed_execute_requeues_pending_without_changing_last_executed() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("fail_requeue_{}", Uuid::now_v7());
    let source = "src-fail".to_string();
    repo::upsert_source(&pool, &module, &source, true)
        .await
        .unwrap();
    let sig = "deadbeef123456";
    sqlx::query(
        r#"
        UPDATE source_registry
        SET discovery_signature = $3,
            last_executed_discovery_signature = NULL,
            workflow_run_pending = false,
            last_checked_at = now()
        WHERE project_module = $1 AND source_identifier = $2
        "#,
    )
    .bind(&module)
    .bind(&source)
    .bind(sig)
    .execute(&pool)
    .await
    .unwrap();

    repo::mark_sources_pending_workflow_run(&pool, &module, std::slice::from_ref(&source))
        .await
        .unwrap();
    repo::clear_workflow_pending_for_sources(&pool, &module, std::slice::from_ref(&source))
        .await
        .unwrap();
    repo::mark_sources_pending_workflow_run(&pool, &module, std::slice::from_ref(&source))
        .await
        .unwrap();

    let row: (bool, Option<String>) = sqlx::query_as(
        "SELECT workflow_run_pending, last_executed_discovery_signature FROM source_registry WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(&module)
    .bind(&source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.0);
    assert!(row.1.is_none());
    teardown_test_module(&pool, &module).await;
}

#[tokio::test]
async fn execution_skips_already_executed_signature() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("exec_sig_{}", Uuid::now_v7());
    let source = "src-exec".to_string();
    repo::upsert_source(&pool, &module, &source, true)
        .await
        .unwrap();
    let sig = "abc123deadbeef";
    sqlx::query(
        r#"
        UPDATE source_registry
        SET discovery_signature = $3,
            last_executed_discovery_signature = $3,
            workflow_run_pending = true,
            workflow_run_pending_at = now(),
            last_checked_at = now()
        WHERE project_module = $1 AND source_identifier = $2
        "#,
    )
    .bind(&module)
    .bind(&source)
    .bind(sig)
    .execute(&pool)
    .await
    .unwrap();

    let (valid, skipped) =
        repo::partition_sources_ready_for_execution(&pool, &module, std::slice::from_ref(&source))
            .await
            .unwrap();
    assert!(valid.is_empty());
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].1.contains("already executed"));

    let pending: (bool,) = sqlx::query_as(
        "SELECT workflow_run_pending FROM source_registry WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(&module)
    .bind(&source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending.0);
    teardown_test_module(&pool, &module).await;
}

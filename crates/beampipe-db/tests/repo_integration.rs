use beampipe_db::{connect, migrate, repo};
use beampipe_domain::{ExecutionStatus, LedgerPatch};
use serde_json::json;
use uuid::Uuid;

async fn test_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = connect(&url).await.ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

#[tokio::test]
async fn discovery_claim_and_release() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("test_{}", Uuid::now_v7());
    repo::upsert_source(&pool, &module, "src-1", true)
        .await
        .unwrap();
    let (token, rows) = repo::claim_source_rows_for_discovery(&pool, Some(&module), 24, 10, 180)
        .await
        .unwrap();
    assert!(token.is_some());
    assert_eq!(rows.len(), 1);
    let released =
        repo::release_discovery_claim(&pool, &module, &["src-1".into()], token.as_ref().unwrap())
            .await
            .unwrap();
    assert_eq!(released, 1);
    repo::delete_all_sources_for_project_module(&pool, &module)
        .await
        .unwrap();
}

#[tokio::test]
async fn workflow_pending_claim_and_clear() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("test_{}", Uuid::now_v7());
    repo::upsert_source(&pool, &module, "src-2", true)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE source_registry SET workflow_run_pending = true, workflow_run_pending_at = now() WHERE project_module = $1",
    )
    .bind(&module)
    .execute(&pool)
    .await
    .unwrap();
    let (token, sources) = repo::claim_pending_sources_for_workflow_run(&pool, &module, 10, 180)
        .await
        .unwrap();
    assert!(token.is_some());
    assert_eq!(sources, vec!["src-2".to_string()]);
    let cleared = repo::clear_workflow_pending_for_sources(&pool, &module, &sources)
        .await
        .unwrap();
    assert_eq!(cleared, 1);
    repo::delete_all_sources_for_project_module(&pool, &module)
        .await
        .unwrap();
}

#[tokio::test]
async fn job_queue_deferred_enqueue() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let job = repo::enqueue_job_deferred(
        &pool,
        "scheduler_tick",
        json!({"project_module": "wallaby_hires"}),
        3600,
        None,
        Some(&format!("deferred:{}", Uuid::now_v7())),
    )
    .await
    .unwrap();
    assert_eq!(job.kind, "scheduler_tick");
    assert_eq!(job.status, "queued");
}

#[tokio::test]
async fn deployment_profile_default_fallback() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("test_{}", Uuid::now_v7());
    let profile = repo::create_deployment_profile(
        &pool,
        &format!("profile-{}", Uuid::now_v7()),
        None,
        Some(&module),
        true,
        json!({}),
        json!({"kind": "rest_remote"}),
    )
    .await
    .unwrap();
    let found = repo::get_default_deployment_profile(&pool, &module)
        .await
        .unwrap();
    assert_eq!(found.unwrap().uuid, profile.uuid);
}

#[tokio::test]
async fn list_slurm_executions_pending_poll_returns_active_slurm() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("test_{}", Uuid::now_v7());
    let exec = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "src-slurm"}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::apply_execution_patch(
        &pool,
        exec.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::AwaitingScheduler),
            scheduler_name: Some("slurm".into()),
            scheduler_job_id: Some("BeampipeExecution-test:4242|/tmp/session".into()),
            ..LedgerPatch::default()
        },
    )
    .await
    .unwrap();
    let pending = repo::list_slurm_executions_pending_poll(&pool)
        .await
        .unwrap();
    assert!(
        pending.iter().any(|row| row.uuid == exec.uuid),
        "expected slurm execution in pending poll list"
    );
    repo::apply_execution_patch(
        &pool,
        exec.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::Completed),
            ..LedgerPatch::default()
        },
    )
    .await
    .unwrap();
    let after = repo::list_slurm_executions_pending_poll(&pool)
        .await
        .unwrap();
    assert!(!after.iter().any(|row| row.uuid == exec.uuid));
}

#[tokio::test]
async fn list_rest_executions_pending_poll_returns_active_daliuge() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("test_{}", Uuid::now_v7());
    let exec = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "src-rest"}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::apply_execution_patch(
        &pool,
        exec.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::Running),
            scheduler_name: Some("daliuge".into()),
            scheduler_job_id: Some("BeampipeExecution-rest-session".into()),
            ..LedgerPatch::default()
        },
    )
    .await
    .unwrap();
    let pending = repo::list_rest_executions_pending_poll(&pool)
        .await
        .unwrap();
    assert!(
        pending.iter().any(|row| row.uuid == exec.uuid),
        "expected daliuge execution in pending poll list"
    );
    repo::apply_execution_patch(
        &pool,
        exec.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::Completed),
            ..LedgerPatch::default()
        },
    )
    .await
    .unwrap();
    let after = repo::list_rest_executions_pending_poll(&pool)
        .await
        .unwrap();
    assert!(!after.iter().any(|row| row.uuid == exec.uuid));
}

#[tokio::test]
async fn token_blacklist_blocks_revoked() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let hash = "abc123";
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    repo::blacklist_token(&pool, hash, expires).await.unwrap();
    assert!(repo::is_token_blacklisted(&pool, hash).await.unwrap());
    assert!(!repo::is_token_blacklisted(&pool, "other").await.unwrap());
}

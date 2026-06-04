use beampipe_db::repo;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/beampipe".into());
    let pool = PgPool::connect(&url).await.expect("postgres");
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("migrate");
    pool
}

#[tokio::test]
async fn recurring_job_requeues_after_completion() {
    let pool = pool().await;
    let key = format!("scheduler_tick:test_{}", Uuid::now_v7());

    let job = repo::enqueue_recurring_job(
        &pool,
        "scheduler_tick",
        json!({"project_module": "test_recurring"}),
        &key,
    )
    .await
    .expect("enqueue");
    assert_eq!(job.status, "queued");

    repo::complete_job(&pool, job.uuid).await.expect("complete");

    let requeued = repo::enqueue_recurring_job(
        &pool,
        "scheduler_tick",
        json!({"project_module": "test_recurring"}),
        &key,
    )
    .await
    .expect("re-enqueue");
    assert_eq!(requeued.status, "queued");
    assert_eq!(requeued.uuid, job.uuid);
}

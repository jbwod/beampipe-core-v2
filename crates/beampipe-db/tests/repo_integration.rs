use beampipe_db::{
    connect, migrate,
    models::{ExecutionStatePatch, WorkerRegistration},
    repo,
};
use beampipe_domain::{DaliugeState, ExecutionStatus, LedgerPatch};
use chrono::{Duration, Utc};
use serde_json::json;
use std::collections::BTreeMap;
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
async fn manual_discovery_requeues_its_completed_trigger() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("manual_{}", Uuid::now_v7());
    let source = format!("source-{}", Uuid::now_v7());
    repo::upsert_source(&pool, &module, &source, true)
        .await
        .unwrap();
    let key = format!("discover_trigger:{module}");
    let (_, first) = repo::mark_sources_and_enqueue_discovery_tick(
        &pool,
        &module,
        Some(std::slice::from_ref(&source)),
        json!({"project_module": module}),
        &key,
    )
    .await
    .unwrap();
    let first = first.unwrap();
    repo::complete_job(&pool, first.uuid).await.unwrap();

    let (_, second) = repo::mark_sources_and_enqueue_discovery_tick(
        &pool,
        &module,
        Some(std::slice::from_ref(&source)),
        json!({"project_module": module, "manual": true}),
        &key,
    )
    .await
    .unwrap();
    let second = second.unwrap();
    assert_eq!(second.uuid, first.uuid);
    assert_eq!(second.status, "queued");
    assert_eq!(second.attempts, 0);
    assert_eq!(second.payload["manual"], true);

    repo::complete_job(&pool, second.uuid).await.unwrap();
    repo::delete_all_sources_for_project_module(&pool, &module)
        .await
        .unwrap();
}

#[tokio::test]
async fn queue_gauges_only_include_runnable_jobs() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let future_kind = format!("future_tick_{}", Uuid::now_v7());
    let overdue_kind = format!("overdue_tick_{}", Uuid::now_v7());
    let future = repo::enqueue_job_with_options(
        &pool,
        &future_kind,
        json!({}),
        repo::JobEnqueueOptions {
            next_run_at: Some(Utc::now() + Duration::hours(1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let overdue = repo::enqueue_job_with_options(
        &pool,
        &overdue_kind,
        json!({}),
        repo::JobEnqueueOptions {
            next_run_at: Some(Utc::now() - Duration::seconds(30)),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let by_kind = repo::queue_depth_by_kind(&pool).await.unwrap();
    assert!(!by_kind.iter().any(|(kind, _)| kind == &future_kind));
    assert_eq!(
        by_kind
            .iter()
            .find(|(kind, _)| kind == &overdue_kind)
            .map(|(_, count)| *count),
        Some(1)
    );

    let ages = repo::oldest_queued_job_age_by_kind(&pool).await.unwrap();
    assert!(!ages.iter().any(|(kind, _)| kind == &future_kind));
    let overdue_age = ages
        .iter()
        .find(|(kind, _)| kind == &overdue_kind)
        .map(|(_, age)| *age)
        .expect("overdue job has an age gauge");
    assert!(
        overdue_age >= 29,
        "age should be measured from next_run_at, got {overdue_age} seconds"
    );

    repo::complete_job(&pool, future.uuid).await.unwrap();
    repo::complete_job(&pool, overdue.uuid).await.unwrap();
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
        None,
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
async fn deployment_profile_rejects_duplicate_default_in_same_scope() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("default_scope_{}", Uuid::now_v7());
    let first = repo::create_deployment_profile(
        &pool,
        &format!("profile-{}", Uuid::now_v7()),
        None,
        Some(&module),
        true,
        None,
        json!({}),
        json!({"kind": "rest_remote"}),
    )
    .await
    .unwrap();
    let error = repo::create_deployment_profile(
        &pool,
        &format!("profile-{}", Uuid::now_v7()),
        None,
        Some(&module),
        true,
        None,
        json!({}),
        json!({"kind": "rest_remote"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("already the default"));
    repo::delete_deployment_profile(&pool, first.uuid)
        .await
        .unwrap();
}

#[tokio::test]
async fn execution_create_idempotency_is_scoped_and_payload_bound() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let suffix = Uuid::now_v7().simple().to_string();
    let user = repo::create_user(
        &pool,
        "Idempotency Test",
        &format!("u{}", &suffix[..12]),
        &format!("{}@test.invalid", &suffix[..20]),
        "unused",
        false,
    )
    .await
    .unwrap();
    let module = format!("idempotency_{}", &suffix[..12]);
    let sources = json!([{"source_identifier": "source-1"}]);
    let key = format!("create-{}", &suffix[..16]);
    let (first, created) = repo::create_execution_idempotent_with_correlation(
        &pool,
        &module,
        sources.clone(),
        "casda",
        None,
        None,
        Some(user.id),
        Some("test:create"),
        Some(&key),
        Some("request-hash-a"),
    )
    .await
    .unwrap();
    assert!(created);

    let (replayed, created) = repo::create_execution_idempotent_with_correlation(
        &pool,
        &module,
        sources.clone(),
        "casda",
        None,
        None,
        Some(user.id),
        Some("test:replay"),
        Some(&key),
        Some("request-hash-a"),
    )
    .await
    .unwrap();
    assert!(!created);
    assert_eq!(replayed.uuid, first.uuid);

    let error = repo::create_execution_idempotent_with_correlation(
        &pool,
        &module,
        sources,
        "casda",
        None,
        None,
        Some(user.id),
        Some("test:conflict"),
        Some(&key),
        Some("request-hash-b"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("different request"));
}

#[tokio::test]
async fn successful_reconciliation_clears_transient_error() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("completion_{}", Uuid::now_v7());
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    repo::apply_execution_patch(
        &pool,
        execution.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::Running),
            error: Some("transient DALiuGE status failure".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let completed = repo::apply_execution_state_patch(
        &pool,
        execution.uuid,
        ExecutionStatePatch {
            daliuge_session_id: Some(format!("session-{}", execution.uuid)),
            daliuge_state: Some(DaliugeState::Finished),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.terminal_outcome.as_deref(), Some("succeeded"));
    assert!(completed.last_error.is_none());
}

#[tokio::test]
async fn cancellation_updates_ledger_and_provenance_atomically() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("cancel_{}", Uuid::now_v7());
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let cancelled = repo::cancel_execution_with_correlation(
        &pool,
        execution.uuid,
        "user:test",
        Some("cancel:test"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(cancelled.terminal_outcome.as_deref(), Some("cancelled"));
    assert_eq!(cancelled.control_phase.as_deref(), Some("terminal"));
    assert!(cancelled.completed_at.is_some());

    let events = repo::list_provenance_events_for_execution(&pool, execution.uuid, 20)
        .await
        .unwrap();
    let event = events
        .iter()
        .find(|event| event.event_type == "execution.cancelled")
        .expect("cancellation provenance event");
    assert_eq!(event.actor.as_deref(), Some("user:test"));
    assert_eq!(event.correlation_id.as_deref(), Some("cancel:test"));
}

#[tokio::test]
async fn ignored_terminal_overwrite_does_not_emit_false_provenance() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("terminal_{}", Uuid::now_v7());
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    for status in [ExecutionStatus::Running, ExecutionStatus::Completed] {
        repo::apply_execution_patch(
            &pool,
            execution.uuid,
            LedgerPatch {
                status: Some(status),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    repo::apply_execution_patch_with_correlation(
        &pool,
        execution.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
        Some("late:worker"),
    )
    .await
    .unwrap();
    let events = repo::list_provenance_events_for_execution(&pool, execution.uuid, 20)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "execution.running")
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| event.correlation_id.as_deref() == Some("late:worker")));
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
            status: Some(ExecutionStatus::Running),
            scheduler_name: Some("slurm".into()),
            scheduler_job_id: Some("BeampipeExecution-test:4242|/tmp/session".into()),
            ..LedgerPatch::default()
        },
    )
    .await
    .unwrap();
    repo::apply_execution_patch(
        &pool,
        exec.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::AwaitingScheduler),
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
            ..LedgerPatch::default()
        },
    )
    .await
    .unwrap();
    repo::apply_execution_state_patch(
        &pool,
        exec.uuid,
        ExecutionStatePatch {
            daliuge_session_id: Some("BeampipeExecution-rest-session".into()),
            daliuge_state: Some(DaliugeState::Running),
            ..Default::default()
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

fn worker_registration(id: Uuid, pool: &str, capabilities: &[&str]) -> WorkerRegistration {
    WorkerRegistration {
        uuid: id,
        instance_name: format!("integration-worker-{id}"),
        host_name: "integration-host".into(),
        process_id: None,
        role: "worker".into(),
        pool: pool.into(),
        capabilities: capabilities.iter().map(|value| (*value).into()).collect(),
        labels: json!({"test": "worker_leases"}),
        version: env!("CARGO_PKG_VERSION").into(),
        concurrency_limit: 1,
    }
}

#[tokio::test]
async fn active_job_lease_cannot_be_stolen() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let queue = format!("lease_active_{}", Uuid::now_v7());
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    repo::register_worker_instance(
        &pool,
        &worker_registration(first, &queue, &["casda-discovery"]),
    )
    .await
    .unwrap();
    repo::register_worker_instance(
        &pool,
        &worker_registration(second, &queue, &["casda-discovery"]),
    )
    .await
    .unwrap();
    let job = repo::enqueue_job_with_options(
        &pool,
        "lease_test",
        json!({}),
        repo::JobEnqueueOptions {
            idempotency_key: Some(format!("lease-active:{}", Uuid::now_v7())),
            pool: Some(queue.clone()),
            required_capability: Some("casda-discovery".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let claimed =
        repo::claim_next_job_for_worker(&pool, first, &queue, &["casda-discovery".into()], 60)
            .await
            .unwrap()
            .expect("first worker claims job");
    let stolen =
        repo::claim_next_job_for_worker(&pool, second, &queue, &["casda-discovery".into()], 60)
            .await
            .unwrap();
    assert!(stolen.is_none());
    assert!(
        repo::complete_job_with_lease(&pool, job.uuid, first, claimed.lease_token.unwrap(),)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn expired_job_lease_is_recovered_with_new_fence() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let queue = format!("lease_recovery_{}", Uuid::now_v7());
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    for worker in [first, second] {
        repo::register_worker_instance(
            &pool,
            &worker_registration(worker, &queue, &["daliuge-deployment"]),
        )
        .await
        .unwrap();
    }
    let job = repo::enqueue_job_with_options(
        &pool,
        "lease_recovery_test",
        json!({}),
        repo::JobEnqueueOptions {
            idempotency_key: Some(format!("lease-recovery:{}", Uuid::now_v7())),
            pool: Some(queue.clone()),
            required_capability: Some("daliuge-deployment".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let original =
        repo::claim_next_job_for_worker(&pool, first, &queue, &["daliuge-deployment".into()], 60)
            .await
            .unwrap()
            .unwrap();
    sqlx::query(
        "UPDATE jobs SET lease_expires_at = now() - interval '1 second', locked_until = now() - interval '1 second' WHERE uuid = $1",
    )
    .bind(job.uuid)
    .execute(&pool)
    .await
    .unwrap();
    let recovered =
        repo::claim_next_job_for_worker(&pool, second, &queue, &["daliuge-deployment".into()], 60)
            .await
            .unwrap()
            .expect("expired lease should be recovered");
    assert_eq!(recovered.lease_owner, Some(second));
    assert_ne!(recovered.lease_token, original.lease_token);
    assert_eq!(recovered.attempts, original.attempts + 1);
    let history = repo::list_job_claim_history(&pool, job.uuid).await.unwrap();
    assert_eq!(
        history
            .iter()
            .map(|event| event.event.as_str())
            .collect::<Vec<_>>(),
        vec!["claimed", "recovered"]
    );
    assert!(
        repo::complete_job_with_lease(&pool, job.uuid, second, recovered.lease_token.unwrap(),)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn claim_requires_advertised_capability() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let queue = format!("lease_capability_{}", Uuid::now_v7());
    let discovery_worker = Uuid::now_v7();
    let slurm_worker = Uuid::now_v7();
    repo::register_worker_instance(
        &pool,
        &worker_registration(discovery_worker, &queue, &["casda-discovery"]),
    )
    .await
    .unwrap();
    repo::register_worker_instance(
        &pool,
        &worker_registration(slurm_worker, &queue, &["slurm-remote"]),
    )
    .await
    .unwrap();
    let job = repo::enqueue_job_with_options(
        &pool,
        "capability_test",
        json!({}),
        repo::JobEnqueueOptions {
            idempotency_key: Some(format!("capability:{}", Uuid::now_v7())),
            pool: Some(queue.clone()),
            required_capability: Some("slurm-remote".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let ineligible = repo::claim_next_job_for_worker(
        &pool,
        discovery_worker,
        &queue,
        &["casda-discovery".into()],
        60,
    )
    .await
    .unwrap();
    assert!(ineligible.is_none());
    let eligible =
        repo::claim_next_job_for_worker(&pool, slurm_worker, &queue, &["slurm-remote".into()], 60)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(eligible.uuid, job.uuid);
    assert!(repo::complete_job_with_lease(
        &pool,
        job.uuid,
        slurm_worker,
        eligible.lease_token.unwrap(),
    )
    .await
    .unwrap());
}

#[tokio::test]
async fn claim_requires_all_worker_labels() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let queue = format!("lease_labels_{}", Uuid::now_v7());
    let ineligible = Uuid::now_v7();
    let eligible = Uuid::now_v7();
    let mut wrong_site = worker_registration(ineligible, &queue, &["slurm-remote"]);
    wrong_site.labels = json!({"site": "local", "scheduler": "slurm"});
    let mut right_site = worker_registration(eligible, &queue, &["slurm-remote"]);
    right_site.labels = json!({"site": "pawsey", "scheduler": "slurm"});
    repo::register_worker_instance(&pool, &wrong_site)
        .await
        .unwrap();
    repo::register_worker_instance(&pool, &right_site)
        .await
        .unwrap();

    let job = repo::enqueue_job_with_options(
        &pool,
        "label_test",
        json!({}),
        repo::JobEnqueueOptions {
            idempotency_key: Some(format!("labels:{}", Uuid::now_v7())),
            pool: Some(queue.clone()),
            required_capability: Some("slurm-remote".into()),
            required_labels: BTreeMap::from([("site".into(), "pawsey".into())]),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(repo::claim_next_job_for_worker(
        &pool,
        ineligible,
        &queue,
        &["slurm-remote".into()],
        60,
    )
    .await
    .unwrap()
    .is_none());
    let claimed =
        repo::claim_next_job_for_worker(&pool, eligible, &queue, &["slurm-remote".into()], 60)
            .await
            .unwrap()
            .expect("matching worker should claim job");
    assert_eq!(claimed.uuid, job.uuid);
}

#[tokio::test]
async fn worker_concurrency_is_enforced_by_the_claim_transaction() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let queue = format!("lease_capacity_{}", Uuid::now_v7());
    let worker = Uuid::now_v7();
    repo::register_worker_instance(
        &pool,
        &worker_registration(worker, &queue, &["manifest-generation"]),
    )
    .await
    .unwrap();
    for suffix in ["first", "second"] {
        repo::enqueue_job_with_options(
            &pool,
            "capacity_test",
            json!({}),
            repo::JobEnqueueOptions {
                idempotency_key: Some(format!("capacity:{suffix}:{}", Uuid::now_v7())),
                pool: Some(queue.clone()),
                required_capability: Some("manifest-generation".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    assert!(repo::claim_next_job_for_worker(
        &pool,
        worker,
        &queue,
        &["manifest-generation".into()],
        60,
    )
    .await
    .unwrap()
    .is_some());
    assert!(repo::claim_next_job_for_worker(
        &pool,
        worker,
        &queue,
        &["manifest-generation".into()],
        60,
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn failed_pre_submission_execution_retries_atomically_from_submit() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("retry_test_{}", Uuid::now_v7());
    let source = "retry-source";
    repo::upsert_source(&pool, &module, source, true)
        .await
        .unwrap();
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": source}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let original = repo::enqueue_job(
        &pool,
        "execute",
        json!({"execution_id": execution.uuid}),
        Some(execution.uuid),
        Some(&format!("execute:{}", execution.uuid)),
    )
    .await
    .unwrap();
    repo::complete_job(&pool, original.uuid).await.unwrap();
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET status = 'failed', execution_phase = 'submit',
            submission_state = 'failed', scheduler_state = 'not_submitted',
            daliuge_state = 'not_created', terminal_outcome = 'failed',
            workflow_manifest = '{"manifest":true}'::jsonb,
            last_error = 'translator unavailable', completed_at = now()
        WHERE uuid = $1
        "#,
    )
    .bind(execution.uuid)
    .execute(&pool)
    .await
    .unwrap();

    let retried = repo::retry_execution(
        &pool,
        execution.uuid,
        "operator:test",
        "translator connectivity restored",
        None,
    )
    .await
    .unwrap();
    assert_eq!(retried.execution.status, "retrying");
    assert_eq!(retried.execution.retry_count, 1);
    assert_eq!(retried.execution.execution_phase.as_deref(), Some("submit"));
    assert_eq!(
        retried.execution.submission_state.as_deref(),
        Some("not_started")
    );
    assert!(retried.execution.completed_at.is_none());
    assert_eq!(retried.job.status, "queued");
    assert_eq!(retried.job.pool, original.pool);
    assert_eq!(retried.job.payload["do_stage"], false);
    assert_eq!(retried.job.payload["do_submit"], true);
}

#[tokio::test]
async fn submitted_external_work_cannot_be_retried_in_place() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("retry_blocked_{}", Uuid::now_v7());
    let execution = repo::create_execution(&pool, &module, json!([]), "casda", None, None, None)
        .await
        .unwrap();
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET status = 'failed', execution_phase = 'submit',
            submission_state = 'submitted', scheduler_name = 'slurm',
            scheduler_job_id = '12345', scheduler_state = 'failed',
            daliuge_state = 'not_created', terminal_outcome = 'failed',
            workflow_manifest = '{"manifest":true}'::jsonb
        WHERE uuid = $1
        "#,
    )
    .bind(execution.uuid)
    .execute(&pool)
    .await
    .unwrap();
    let error = repo::retry_execution(&pool, execution.uuid, "operator:test", "try again", None)
        .await
        .unwrap_err();
    assert_eq!(error.code(), "submission_may_exist");
}

use beampipe_db::{
    connect, migrate,
    models::{ExecutionArtifactInput, ExecutionStatePatch, WorkerRegistration},
    repo,
};
use beampipe_domain::{ControlPhase, DaliugeState, ExecutionStatus, LedgerPatch};
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
    let runnable = repo::runnable_queue_depth(&pool).await.unwrap();
    assert_eq!(repo::queue_depth(&pool).await.unwrap(), runnable);
    assert_eq!(
        repo::operator_overview_counts(&pool, 120)
            .await
            .unwrap()
            .queue_depth,
        runnable
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
async fn automated_execution_and_execute_job_commit_together() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("auto_atomic_{}", Uuid::now_v7().simple());
    let (execution, job) = repo::create_automated_execution_and_enqueue(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "local",
        None,
        None,
        Some("scheduler:test"),
        repo::AutomatedExecutionEnqueue {
            scheduler_manifest: json!({"scheduler": {"policy_decision": "admitted"}}),
            job_payload: json!({"traceparent": "test"}),
            worker_pool: "automation".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(execution.scheduler_name.as_deref(), Some("workflow_auto"));
    assert_eq!(
        execution.workflow_manifest.as_ref().unwrap()["scheduler"]["policy_decision"],
        "admitted"
    );
    assert_eq!(job.execution_id, Some(execution.uuid));
    assert_eq!(job.payload["execution_id"], execution.uuid.to_string());
    assert_eq!(job.payload["traceparent"], "test");
    assert_eq!(job.pool, "automation");
    assert_eq!(
        job.required_capability.as_deref(),
        Some("daliuge-deployment")
    );
}

#[tokio::test]
async fn automated_execution_rolls_back_when_enqueue_fails() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("auto_rollback_{}", Uuid::now_v7().simple());
    let error = repo::create_automated_execution_and_enqueue(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "local",
        None,
        None,
        Some("scheduler:test"),
        repo::AutomatedExecutionEnqueue {
            scheduler_manifest: json!({"scheduler": {"policy_decision": "admitted"}}),
            job_payload: json!({}),
            worker_pool: "x".repeat(65),
        },
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("too long"));
    let executions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM batch_execution_record WHERE project_module = $1")
            .bind(&module)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(executions, 0, "ledger insert must roll back with enqueue");
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
async fn required_outputs_hold_success_until_inventory_artifact_commits() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("output_required_{}", Uuid::now_v7().simple());
    let spec = json!({
        "apiVersion": "beampipe.dev/v2",
        "kind": "ProjectConfig",
        "metadata": {"id": module},
        "output_verification": {
            "required": true,
            "inventory_schema": "wallaby-hires-output-inventory/v1"
        }
    });
    let config = repo::insert_project_config(&pool, &module, spec, &"c".repeat(64))
        .await
        .unwrap();
    repo::upsert_source(&pool, &module, "source-1", true)
        .await
        .unwrap();
    sqlx::query(
        r#"
        UPDATE source_registry
        SET discovery_signature = $3,
            workflow_run_pending = true,
            workflow_run_pending_at = now()
        WHERE project_module = $1 AND source_identifier = $2
        "#,
    )
    .bind(&module)
    .bind("source-1")
    .bind("1".repeat(64))
    .execute(&pool)
    .await
    .unwrap();
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "local",
        None,
        Some(config.uuid),
        None,
    )
    .await
    .unwrap();
    assert!(execution.output_verification_required);
    assert_eq!(execution.output_state.as_deref(), Some("pending"));
    assert_eq!(
        execution.output_verification_policy["inventory_schema"],
        "wallaby-hires-output-inventory/v1"
    );

    let held = repo::apply_execution_state_patch(
        &pool,
        execution.uuid,
        ExecutionStatePatch {
            daliuge_state: Some(DaliugeState::Finished),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(held.status, "running");
    assert_eq!(held.output_state.as_deref(), Some("pending"));
    assert!(held.terminal_outcome.is_none());
    assert!(held.completed_at.is_none());

    let direct_completion = sqlx::query(
        "UPDATE batch_execution_record SET status = 'completed' WHERE uuid = $1",
    )
    .bind(execution.uuid)
    .execute(&pool)
    .await;
    assert!(
        direct_completion.is_err(),
        "the database must reject required, unverified completion"
    );

    let inventory_sha256 = "a".repeat(64);
    let artifact = ExecutionArtifactInput {
        kind: "output_inventory".into(),
        storage_kind: "remote".into(),
        uri: Some("file:///durable/wallaby/run-1".into()),
        inline_json: Some(json!({
            "schema": "wallaby-hires-output-inventory/v1",
            "products": [{"path": "image.fits", "bytes": 42, "sha256": "b".repeat(64)}],
            "inventory_sha256": inventory_sha256,
        })),
        media_type: "application/vnd.wallaby.output-inventory+json".into(),
        sha256: "e".repeat(64),
        size_bytes: Some(512),
        producer_phase: "publication_acknowledged".into(),
        metadata: json!({
            "inventory_schema": "wallaby-hires-output-inventory/v1",
            "inventory_sha256": inventory_sha256,
            "publication": {"acknowledged": true, "receipt_id": "receipt-1"},
        }),
    };
    let (completed, stored) = repo::verify_execution_outputs(
        &pool,
        execution.uuid,
        artifact.clone(),
        "trusted-publisher:test",
        Some("outputs:test"),
    )
    .await
    .unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.output_state.as_deref(), Some("verified"));
    assert_eq!(completed.terminal_outcome.as_deref(), Some("succeeded"));
    assert!(completed.completed_at.is_some());
    assert_eq!(stored.uri, artifact.uri);
    assert_eq!(
        completed.workflow_manifest.as_ref().unwrap()["beampipe_run_record"]
            ["output_verification"]["artifact_id"],
        stored.uuid.to_string()
    );
    let source = repo::get_source_by_identifier(&pool, &module, "source-1")
        .await
        .unwrap()
        .unwrap();
    assert!(!source.workflow_run_pending);
    assert_eq!(
        source.last_executed_discovery_signature,
        source.discovery_signature
    );

    sqlx::query(
        r#"
        UPDATE source_registry
        SET discovery_signature = $3,
            workflow_run_pending = true,
            workflow_run_pending_at = now(),
            workflow_claim_token = 'newer-claim',
            workflow_claimed_at = now(),
            workflow_claim_expires_at = now() + interval '5 minutes'
        WHERE project_module = $1 AND source_identifier = $2
        "#,
    )
    .bind(&module)
    .bind("source-1")
    .bind("2".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    let (replayed, replayed_artifact) = repo::verify_execution_outputs(
        &pool,
        execution.uuid,
        artifact,
        "trusted-publisher:test",
        Some("outputs:replay"),
    )
    .await
    .unwrap();
    assert_eq!(replayed.status, "completed");
    assert_eq!(replayed_artifact.uuid, stored.uuid);
    let source_after_replay = repo::get_source_by_identifier(&pool, &module, "source-1")
        .await
        .unwrap()
        .unwrap();
    assert!(source_after_replay.workflow_run_pending);
    assert_eq!(
        source_after_replay.workflow_claim_token.as_deref(),
        Some("newer-claim")
    );
    assert_ne!(
        source_after_replay.last_executed_discovery_signature,
        source_after_replay.discovery_signature
    );

    let events = repo::list_provenance_events_for_execution(&pool, execution.uuid, 20)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "execution.outputs_verified")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "execution.completed")
            .count(),
        1
    );
    let verified_event = events
        .iter()
        .find(|event| event.event_type == "execution.outputs_verified")
        .unwrap();
    assert_eq!(
        verified_event.payload["inventory_schema"],
        "wallaby-hires-output-inventory/v1"
    );
}

#[tokio::test]
async fn output_verification_preserves_discovery_that_changed_after_admission() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("output_changed_{}", Uuid::now_v7().simple());
    let spec = json!({
        "apiVersion": "beampipe.dev/v2",
        "kind": "ProjectConfig",
        "metadata": {"id": module},
        "output_verification": {
            "required": true,
            "inventory_schema": "wallaby-hires-output-inventory/v1"
        }
    });
    let config = repo::insert_project_config(&pool, &module, spec, &"d".repeat(64))
        .await
        .unwrap();
    repo::upsert_source(&pool, &module, "source-1", true)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE source_registry SET discovery_signature = $3, workflow_run_pending = true WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(&module)
    .bind("source-1")
    .bind("3".repeat(64))
    .execute(&pool)
    .await
    .unwrap();
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "local",
        None,
        Some(config.uuid),
        None,
    )
    .await
    .unwrap();
    repo::apply_execution_state_patch(
        &pool,
        execution.uuid,
        ExecutionStatePatch {
            daliuge_state: Some(DaliugeState::Finished),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE source_registry
        SET discovery_signature = $3,
            workflow_run_pending = true,
            workflow_claim_token = 'changed-claim',
            workflow_claimed_at = now(),
            workflow_claim_expires_at = now() + interval '5 minutes'
        WHERE project_module = $1 AND source_identifier = $2
        "#,
    )
    .bind(&module)
    .bind("source-1")
    .bind("4".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    let artifact = ExecutionArtifactInput {
        kind: "output_inventory".into(),
        storage_kind: "remote".into(),
        uri: Some("file:///durable/wallaby/run-changed".into()),
        inline_json: Some(json!({"inventory_sha256": "5".repeat(64)})),
        media_type: "application/vnd.wallaby.output-inventory+json".into(),
        sha256: "6".repeat(64),
        size_bytes: Some(512),
        producer_phase: "publication_acknowledged".into(),
        metadata: json!({
            "inventory_schema": "wallaby-hires-output-inventory/v1",
            "inventory_sha256": "5".repeat(64),
        }),
    };
    let (completed, _) = repo::verify_execution_outputs(
        &pool,
        execution.uuid,
        artifact,
        "trusted-publisher:test",
        Some("outputs:changed"),
    )
    .await
    .unwrap();
    assert_eq!(completed.status_enum(), Some(ExecutionStatus::Completed));
    let source = repo::get_source_by_identifier(&pool, &module, "source-1")
        .await
        .unwrap()
        .unwrap();
    assert!(source.workflow_run_pending);
    assert_eq!(source.workflow_claim_token.as_deref(), Some("changed-claim"));
    assert_ne!(
        source.last_executed_discovery_signature,
        source.discovery_signature
    );
    let events = repo::list_provenance_events_for_execution(&pool, execution.uuid, 20)
        .await
        .unwrap();
    let completed_event = events
        .iter()
        .find(|event| event.event_type == "execution.completed")
        .unwrap();
    assert_eq!(completed_event.payload["source_signatures_finalized"], false);
}

#[tokio::test]
async fn output_verification_opt_out_cannot_be_marked_verified() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("output_optout_{}", Uuid::now_v7().simple());
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": "source-1"}]),
        "local",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(!execution.output_verification_required);
    let result = repo::verify_execution_outputs(
        &pool,
        execution.uuid,
        ExecutionArtifactInput {
            kind: "output_inventory".into(),
            storage_kind: "remote".into(),
            uri: Some("file:///durable/wallaby/run-2".into()),
            inline_json: Some(json!({"inventory": true})),
            media_type: "application/json".into(),
            sha256: "d".repeat(64),
            size_bytes: Some(1),
            producer_phase: "publication_acknowledged".into(),
            metadata: json!({
                "inventory_schema": "wallaby-hires-output-inventory/v1",
                "inventory_sha256": "d".repeat(64)
            }),
        },
        "trusted-publisher:test",
        None,
    )
    .await;
    assert!(matches!(
        result,
        Err(repo::VerifyExecutionOutputsError::Rejected(_))
    ));
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
    let execute_job = repo::enqueue_job(
        &pool,
        "execute",
        json!({"execution_id": execution.uuid}),
        Some(execution.uuid),
        Some(&format!("execute:cancel:{}", execution.uuid)),
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
    let invalidated =
        repo::get_job_by_idempotency_key(&pool, execute_job.idempotency_key.as_deref().unwrap())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(invalidated.status, "cancelled");
    assert!(invalidated.lease_token.is_none());
    assert!(repo::get_active_job_for_execution(&pool, execution.uuid)
        .await
        .unwrap()
        .is_none());

    let stale_success = repo::apply_execution_state_patch_with_transition(
        &pool,
        execution.uuid,
        ExecutionStatePatch {
            control_phase: Some(ControlPhase::OutputVerification),
            scheduler_state: Some(beampipe_domain::SchedulerState::Succeeded),
            daliuge_state: Some(DaliugeState::Finished),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!stale_success.entered_terminal);
    assert_eq!(stale_success.row.status, "cancelled");
    assert_eq!(stale_success.row.control_phase.as_deref(), Some("terminal"));
    assert_eq!(
        stale_success.row.terminal_outcome.as_deref(),
        Some("cancelled")
    );

    let events = repo::list_provenance_events_for_execution(&pool, execution.uuid, 20)
        .await
        .unwrap();
    let event = events
        .iter()
        .find(|event| event.event_type == "execution.cancelled")
        .expect("cancellation provenance event");
    assert_eq!(event.actor.as_deref(), Some("user:test"));
    assert_eq!(event.correlation_id.as_deref(), Some("cancel:test"));
    assert_eq!(event.payload["invalidated_execute_jobs"], 1);
}

#[tokio::test]
async fn terminal_failure_is_not_reopened_by_a_metadata_state_patch() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("failed_patch_{}", Uuid::now_v7());
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
            status: Some(ExecutionStatus::Failed),
            error: Some("DIM poll timeout".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let patched = repo::apply_execution_state_patch_with_transition(
        &pool,
        execution.uuid,
        ExecutionStatePatch {
            control_phase: Some(ControlPhase::Terminal),
            terminal_outcome: Some(beampipe_domain::TerminalOutcome::Failed),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!patched.entered_terminal);
    assert_eq!(patched.row.status, "failed");
    assert_eq!(patched.row.control_phase.as_deref(), Some("terminal"));
    assert!(patched.row.completed_at.is_some());
}

#[tokio::test]
async fn execution_source_readiness_is_rechecked_after_admission() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("dispatch_ready_{}", Uuid::now_v7().simple());
    let source = "source-1";
    repo::upsert_source(&pool, &module, source, true)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE source_registry SET last_checked_at = now(), discovery_signature = 'ready' WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(&module)
    .bind(source)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO archive_metadata (uuid, project_module, source_identifier, sbid, metadata_json)
        VALUES ($1, $2, $3, '1', '{"discovery_flags":{"ready":true}}'::jsonb)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(&module)
    .bind(source)
    .execute(&pool)
    .await
    .unwrap();
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": source, "sbids": ["1"]}]),
        "local",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::execution_source_readiness_errors(&pool, &execution)
        .await
        .unwrap()
        .is_empty());

    let missing_sbid = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": source, "sbids": ["2"]}]),
        "local",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let errors = repo::execution_source_readiness_errors(&pool, &missing_sbid)
        .await
        .unwrap();
    assert!(errors
        .iter()
        .any(|error| error.contains("no discovered metadata") && error.contains("2")));

    let malformed = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": source, "sbids": ["1", 2]}]),
        "local",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let errors = repo::execution_source_readiness_errors(&pool, &malformed)
        .await
        .unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("sbids[1] must be a non-empty string"));

    sqlx::query(
        "UPDATE source_registry SET enabled = false WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(&module)
    .bind(source)
    .execute(&pool)
    .await
    .unwrap();
    let errors = repo::execution_source_readiness_errors(&pool, &execution)
        .await
        .unwrap();
    assert!(errors.iter().any(|error| error.contains("disabled")));

    sqlx::query("DELETE FROM source_registry WHERE project_module = $1 AND source_identifier = $2")
        .bind(&module)
        .bind(source)
        .execute(&pool)
        .await
        .unwrap();
    let errors = repo::execution_source_readiness_errors(&pool, &execution)
        .await
        .unwrap();
    assert!(errors.iter().any(|error| error.contains("not registered")));
}

#[test]
fn persisted_execution_scope_parser_rejects_ambiguous_selections() {
    let parsed = repo::parse_execution_source_scope(&json!([
        {"source_identifier": " source-1 ", "sbids": [" 2 ", "1"]},
        {"source_identifier": "source-2"}
    ]))
    .unwrap();
    assert_eq!(
        parsed.source_identifiers(),
        vec!["source-1".to_string(), "source-2".to_string()]
    );
    assert_eq!(
        parsed.sources["source-1"]
            .as_ref()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["1".to_string(), "2".to_string()]
    );

    for invalid in [
        json!([]),
        json!([{"source_identifier": ""}]),
        json!([{"source_identifier": "source-1", "sbids": []}]),
        json!([{"source_identifier": "source-1", "sbids": ["1", " 1 "]}]),
        json!([
            {"source_identifier": "source-1"},
            {"source_identifier": " source-1 "}
        ]),
    ] {
        assert!(repo::parse_execution_source_scope(&invalid).is_err());
    }
}

async fn prepare_submission_receipt_execution(
    pool: &sqlx::PgPool,
    module: &str,
    backend: &str,
    session_id: &str,
) -> beampipe_db::models::ExecutionRow {
    let execution = repo::create_execution(
        pool,
        module,
        json!([{"source_identifier": "source-1", "sbids": ["1"]}]),
        "local",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET status = 'running', execution_phase = 'submit',
            control_phase = 'submission_pending', submission_state = 'preparing',
            scheduler_state = 'not_submitted', daliuge_state = 'not_created'
        WHERE uuid = $1
        "#,
    )
    .bind(execution.uuid)
    .execute(pool)
    .await
    .unwrap();
    assert!(repo::begin_execution_submission(pool, execution.uuid, backend, session_id, None)
        .await
        .unwrap());
    assert!(!repo::begin_execution_submission(pool, execution.uuid, backend, session_id, None)
        .await
        .unwrap());
    repo::get_execution(pool, execution.uuid)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn slurm_submission_receipt_is_atomic_idempotent_and_conflict_safe() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("receipt_slurm_{}", Uuid::now_v7().simple());
    let session_id = format!("BeampipeExecution-{}", Uuid::now_v7());
    let execution =
        prepare_submission_receipt_execution(&pool, &module, "slurm", &session_id).await;
    let session_dir = format!("/remote/sessions/{session_id}");
    let staging_root = format!("{session_dir}/wallaby-staging");
    let input = repo::SubmissionReceiptInput {
        scheduler_name: "slurm".into(),
        scheduler_job_id: Some("4242".into()),
        daliuge_session_id: Some(session_id.clone()),
        remote_session_dir: Some(session_dir.clone()),
        staging_root: Some(staging_root.clone()),
        workflow_manifest: json!({
            "sources": [],
            "beampipe_run_record": {"slurm": {"job_id": "4242"}},
        }),
        physical_graph: json!([{"oid": "drop-1"}]),
        next_status: ExecutionStatus::AwaitingScheduler,
        actor: "system:test".into(),
        correlation_id: Some("receipt-test".into()),
        poll_job: None,
    };

    let recorded = repo::record_submission_receipt(&pool, execution.uuid, input.clone())
        .await
        .unwrap();
    assert!(!recorded.replayed);
    assert_eq!(recorded.execution.status, "awaiting_scheduler");
    assert_eq!(recorded.execution.control_phase.as_deref(), Some("submitted"));
    assert_eq!(
        recorded.execution.submission_state.as_deref(),
        Some("submitted")
    );
    assert_eq!(recorded.execution.scheduler_state.as_deref(), Some("pending"));
    assert_eq!(
        recorded.execution.daliuge_state.as_deref(),
        Some("not_created")
    );
    assert_eq!(
        recorded.execution.remote_session_dir.as_deref(),
        Some(session_dir.as_str())
    );
    assert_eq!(
        recorded.execution.physical_graph_sha256.as_deref(),
        Some(recorded.physical_graph_artifact.sha256.as_str())
    );
    assert_eq!(
        recorded.physical_graph_artifact.metadata["staging_root"],
        staging_root
    );
    assert!(recorded.physical_graph_artifact.metadata["submission_receipt_sha256"]
        .as_str()
        .is_some_and(|value| value.len() == 64));

    let artifact_count = repo::list_execution_artifacts(&pool, execution.uuid)
        .await
        .unwrap()
        .len();
    let observation_count = repo::list_execution_observations(&pool, execution.uuid, 100, 0)
        .await
        .unwrap()
        .len();
    let provenance_count = repo::list_provenance_events_for_execution(&pool, execution.uuid, 100)
        .await
        .unwrap()
        .len();
    let submission_events = repo::list_provenance_events_for_execution(&pool, execution.uuid, 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "execution.submission_recorded")
        .count();
    assert_eq!(submission_events, 1);

    sqlx::query(
        "UPDATE batch_execution_record SET workflow_manifest = '{\"poll_round\":1}'::jsonb WHERE uuid = $1",
    )
    .bind(execution.uuid)
    .execute(&pool)
    .await
    .unwrap();
    let replayed = repo::record_submission_receipt(&pool, execution.uuid, input.clone())
        .await
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(
        replayed.physical_graph_artifact.uuid,
        recorded.physical_graph_artifact.uuid
    );
    assert_eq!(
        repo::list_execution_artifacts(&pool, execution.uuid)
            .await
            .unwrap()
            .len(),
        artifact_count
    );
    assert_eq!(
        repo::list_execution_observations(&pool, execution.uuid, 100, 0)
            .await
            .unwrap()
            .len(),
        observation_count
    );
    assert_eq!(
        repo::list_provenance_events_for_execution(&pool, execution.uuid, 100)
            .await
            .unwrap()
            .len(),
        provenance_count
    );

    let mut conflicts = Vec::new();
    let mut changed_id = input.clone();
    changed_id.scheduler_job_id = Some("9999".into());
    conflicts.push(changed_id);
    let mut changed_graph = input.clone();
    changed_graph.physical_graph = json!([{"oid": "different"}]);
    conflicts.push(changed_graph);
    let mut changed_manifest = input.clone();
    changed_manifest.workflow_manifest = json!({"changed": true});
    conflicts.push(changed_manifest);
    let mut changed_path = input;
    changed_path.remote_session_dir = Some("/remote/sessions/other".into());
    changed_path.staging_root = Some("/remote/sessions/other/wallaby-staging".into());
    conflicts.push(changed_path);
    for conflict in conflicts {
        let error = repo::record_submission_receipt(&pool, execution.uuid, conflict)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("conflict"));
    }
    let unchanged = repo::get_execution(&pool, execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.scheduler_job_id.as_deref(), Some("4242"));
    assert_eq!(unchanged.physical_graph_sha256, recorded.execution.physical_graph_sha256);
}

#[tokio::test]
async fn rest_submission_receipt_persists_axes_and_poll_job_atomically() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("receipt_rest_{}", Uuid::now_v7().simple());
    let session_id = format!("BeampipeExecution-{}", Uuid::now_v7());
    let execution =
        prepare_submission_receipt_execution(&pool, &module, "daliuge", &session_id).await;
    let result = repo::record_submission_receipt(
        &pool,
        execution.uuid,
        repo::SubmissionReceiptInput {
            scheduler_name: "daliuge".into(),
            scheduler_job_id: None,
            daliuge_session_id: Some(session_id.clone()),
            remote_session_dir: None,
            staging_root: None,
            workflow_manifest: json!({"sources": []}),
            physical_graph: json!([{"oid": "drop-rest"}]),
            next_status: ExecutionStatus::Running,
            actor: "system:test".into(),
            correlation_id: None,
            poll_job: Some(repo::SubmissionReceiptPollJob {
                payload: json!({"execution_id": execution.uuid, "poll_round": 0}),
                worker_pool: "default".into(),
            }),
        },
    )
    .await
    .unwrap();
    assert_eq!(result.execution.status, "running");
    assert_eq!(
        result.execution.scheduler_state.as_deref(),
        Some("not_submitted")
    );
    assert_eq!(result.execution.daliuge_state.as_deref(), Some("running"));
    assert_eq!(
        result.execution.daliuge_session_id.as_deref(),
        Some(session_id.as_str())
    );
    assert!(result.execution.remote_session_dir.is_none());
    let poll_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE execution_id = $1 AND kind = 'dim_poll' AND status = 'queued'",
    )
    .bind(execution.uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(poll_jobs, 1);
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
async fn incomplete_axes_do_not_regress_running_or_duplicate_provenance() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("active_axes_{}", Uuid::now_v7());
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
    repo::apply_execution_patch_with_correlation(
        &pool,
        execution.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::Running),
            ..LedgerPatch::default()
        },
        Some("execute:first"),
    )
    .await
    .unwrap();

    let reconciled = repo::apply_execution_state_patch(
        &pool,
        execution.uuid,
        ExecutionStatePatch {
            control_phase: Some(ControlPhase::GraphPatched),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reconciled.status_enum(), Some(ExecutionStatus::Running));

    repo::apply_execution_patch_with_correlation(
        &pool,
        execution.uuid,
        LedgerPatch {
            status: Some(ExecutionStatus::Running),
            ..LedgerPatch::default()
        },
        Some("execute:submit"),
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
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET status = 'running',
            output_verification_required = true,
            output_state = 'pending',
            scheduler_state = 'succeeded',
            daliuge_state = 'finished',
            terminal_outcome = NULL
        WHERE uuid = $1
        "#,
    )
    .bind(exec.uuid)
    .execute(&pool)
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
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET status = 'running',
            output_verification_required = true,
            output_state = 'pending',
            scheduler_state = 'not_submitted',
            daliuge_state = 'finished',
            terminal_outcome = NULL
        WHERE uuid = $1
        "#,
    )
    .bind(exec.uuid)
    .execute(&pool)
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
    let profile = repo::create_deployment_profile(
        &pool,
        &format!("slurm-retry-{}", Uuid::now_v7()),
        None,
        Some(&module),
        false,
        None,
        json!({}),
        json!({"kind": "slurm_remote", "login_node": "login.example"}),
    )
    .await
    .unwrap();
    let execution = repo::create_execution(
        &pool,
        &module,
        json!([{"source_identifier": source}]),
        "casda",
        Some(profile.uuid),
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
    assert_eq!(
        retried.job.required_capability.as_deref(),
        Some("slurm-remote")
    );
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

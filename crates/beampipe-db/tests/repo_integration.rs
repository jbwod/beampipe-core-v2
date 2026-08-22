use beampipe_db::{
    connect, migrate,
    models::{
        ExecutionArtifactInput, ExecutionObservationInput, ExecutionStatePatch, WorkerRegistration,
    },
    repo,
};
use beampipe_domain::{ControlPhase, DaliugeState, ExecutionStatus, LedgerPatch, SubmissionState};
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

    let direct_completion =
        sqlx::query("UPDATE batch_execution_record SET status = 'completed' WHERE uuid = $1")
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
        completed.workflow_manifest.as_ref().unwrap()["beampipe_run_record"]["output_verification"]
            ["artifact_id"],
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
    assert_eq!(
        source.workflow_claim_token.as_deref(),
        Some("changed-claim")
    );
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
    assert_eq!(
        completed_event.payload["source_signatures_finalized"],
        false
    );
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
    let stale_success_patch = ExecutionStatePatch {
        control_phase: Some(ControlPhase::OutputVerification),
        submission_state: Some(SubmissionState::Submitted),
        scheduler_name: Some("slurm".into()),
        scheduler_job_id: Some("9999".into()),
        scheduler_state: Some(beampipe_domain::SchedulerState::Succeeded),
        scheduler_raw_state: Some("COMPLETED".into()),
        scheduler_reason: Some("late scheduler success".into()),
        daliuge_session_id: Some("late-session".into()),
        daliuge_manager_url: Some("http://late.invalid".into()),
        daliuge_state: Some(DaliugeState::Finished),
        daliuge_raw_status: Some(json!({"status": 3})),
        remote_session_dir: Some("/late/session".into()),
        output_state: Some(beampipe_domain::OutputState::Verified),
        terminal_outcome: Some(beampipe_domain::TerminalOutcome::Succeeded),
        clear_failure_context: true,
        ..Default::default()
    };
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

    repo::record_execution_observation(
        &pool,
        execution.uuid,
        ExecutionObservationInput {
            kind: "scheduler".into(),
            normalized_state: "succeeded".into(),
            raw_state: Some("COMPLETED".into()),
            reason: Some("late scheduler evidence".into()),
            payload: json!({"captured_before_cancellation": true}),
            source_version: None,
            observed_at: Some(chrono::Utc::now()),
        },
    )
    .await
    .unwrap();
    let stale_success = repo::apply_execution_state_patch_with_transition(
        &pool,
        execution.uuid,
        stale_success_patch,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!stale_success.entered_terminal);
    assert_eq!(stale_success.row.status, "cancelled");
    assert_eq!(stale_success.row.control_phase, cancelled.control_phase);
    assert_eq!(
        stale_success.row.submission_state,
        cancelled.submission_state
    );
    assert_eq!(stale_success.row.scheduler_name, cancelled.scheduler_name);
    assert_eq!(
        stale_success.row.scheduler_job_id,
        cancelled.scheduler_job_id
    );
    assert_eq!(stale_success.row.scheduler_state, cancelled.scheduler_state);
    assert_eq!(
        stale_success.row.scheduler_raw_state,
        cancelled.scheduler_raw_state
    );
    assert_eq!(
        stale_success.row.scheduler_reason,
        cancelled.scheduler_reason
    );
    assert_eq!(
        stale_success.row.daliuge_session_id,
        cancelled.daliuge_session_id
    );
    assert_eq!(stale_success.row.daliuge_state, cancelled.daliuge_state);
    assert_eq!(
        stale_success.row.daliuge_raw_status,
        cancelled.daliuge_raw_status
    );
    assert_eq!(stale_success.row.output_state, cancelled.output_state);
    assert_eq!(
        stale_success.row.terminal_outcome,
        cancelled.terminal_outcome
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
    let observations = repo::list_execution_observations(&pool, execution.uuid, 20, 0)
        .await
        .unwrap();
    assert!(observations.iter().any(|observation| {
        observation.raw_state.as_deref() == Some("COMPLETED")
            && observation.payload["captured_before_cancellation"] == true
    }));
}

#[tokio::test]
async fn cancellation_refuses_unresolved_external_submission() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("can_unres_{}", Uuid::now_v7());
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
    assert!(repo::begin_execution_submission(
        &pool,
        execution.uuid,
        "slurm",
        "session-cancel-race",
        None,
        1_800,
        Some("cancel-race-target"),
    )
    .await
    .unwrap()
    .is_some());

    for state in [SubmissionState::InFlight, SubmissionState::Uncertain] {
        if state == SubmissionState::Uncertain {
            repo::apply_execution_state_patch(
                &pool,
                execution.uuid,
                ExecutionStatePatch {
                    submission_state: Some(state),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        let before = repo::get_execution(&pool, execution.uuid)
            .await
            .unwrap()
            .unwrap();
        let error = repo::cancel_execution_with_correlation(
            &pool,
            execution.uuid,
            "user:test",
            Some("cancel:unresolved"),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("submission outcome"));
        let current = repo::get_execution(&pool, execution.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.status, before.status);
        assert!(!current.status_enum().unwrap().is_terminal());
        assert_eq!(current.submission_state.as_deref(), Some(state.as_str()));
        assert!(current.completed_at.is_none());
    }
}

#[tokio::test]
async fn confirmed_exact_slurm_cancellation_wins_over_a_stale_running_patch() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("can_exact_{}", Uuid::now_v7());
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
        Some(&format!("execute:cancel-exact:{}", execution.uuid)),
    )
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET status = 'awaiting_scheduler',
            control_phase = 'submission_pending',
            submission_state = 'uncertain',
            scheduler_name = 'slurm',
            scheduler_job_id = '4242',
            scheduler_state = 'running',
            scheduler_raw_state = 'RUNNING',
            daliuge_session_id = 'BeampipeExecution-cancel-exact'
        WHERE uuid = $1
        "#,
    )
    .bind(execution.uuid)
    .execute(&pool)
    .await
    .unwrap();
    let stale_running_patch = ExecutionStatePatch {
        control_phase: Some(ControlPhase::Monitoring),
        scheduler_state: Some(beampipe_domain::SchedulerState::Running),
        scheduler_raw_state: Some("RUNNING".into()),
        scheduler_reason: Some("stale poll".into()),
        ..Default::default()
    };

    let wrong_identity = repo::cancel_execution_with_confirmed_external_cancellation(
        &pool,
        execution.uuid,
        "user:test",
        Some("cancel:wrong-id"),
        repo::ConfirmedExternalCancellation::Slurm {
            scheduler_job_id: "9999".into(),
            exact_job_id: "9999".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(wrong_identity.to_string().contains("submission outcome"));
    let before = repo::get_execution(&pool, execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.status, "awaiting_scheduler");
    assert_eq!(before.scheduler_state.as_deref(), Some("running"));

    let cancelled = repo::cancel_execution_with_confirmed_external_cancellation(
        &pool,
        execution.uuid,
        "user:test",
        Some("cancel:exact"),
        repo::ConfirmedExternalCancellation::Slurm {
            scheduler_job_id: "4242".into(),
            exact_job_id: "4242".into(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(cancelled.control_phase.as_deref(), Some("terminal"));
    assert_eq!(cancelled.submission_state.as_deref(), Some("uncertain"));
    assert_eq!(cancelled.scheduler_job_id.as_deref(), Some("4242"));
    assert_eq!(cancelled.scheduler_state.as_deref(), Some("cancelled"));
    assert_eq!(cancelled.terminal_outcome.as_deref(), Some("cancelled"));

    let ignored = repo::apply_execution_state_patch_with_transition(
        &pool,
        execution.uuid,
        stale_running_patch,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ignored.row.status, "cancelled");
    assert_eq!(ignored.row.control_phase.as_deref(), Some("terminal"));
    assert_eq!(ignored.row.scheduler_state.as_deref(), Some("cancelled"));
    assert_eq!(
        ignored.row.scheduler_raw_state.as_deref(),
        Some("CANCELLED_CONFIRMED")
    );
    let cancelled_job =
        repo::get_job_by_idempotency_key(&pool, execute_job.idempotency_key.as_deref().unwrap())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(cancelled_job.status, "cancelled");

    let session_dir = "/remote/sessions/BeampipeExecution-cancel-exact";
    let late_receipt = repo::record_submission_receipt(
        &pool,
        execution.uuid,
        repo::SubmissionReceiptInput {
            scheduler_name: "slurm".into(),
            scheduler_job_id: Some("4242".into()),
            daliuge_session_id: Some("BeampipeExecution-cancel-exact".into()),
            remote_session_dir: Some(session_dir.into()),
            staging_root: Some(format!("{session_dir}/wallaby-staging")),
            workflow_manifest: json!({"late_receipt": true}),
            physical_graph: json!([{"oid": "drop-after-cancel"}]),
            next_status: ExecutionStatus::AwaitingScheduler,
            actor: "system:late-worker".into(),
            correlation_id: Some("receipt:after-cancel".into()),
            poll_job: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(late_receipt.execution.status, "cancelled");
    assert_eq!(
        late_receipt.execution.control_phase.as_deref(),
        Some("terminal")
    );
    assert_eq!(
        late_receipt.execution.submission_state.as_deref(),
        Some("submitted")
    );
    assert_eq!(
        late_receipt.execution.scheduler_state.as_deref(),
        Some("cancelled")
    );
    assert_eq!(
        late_receipt.execution.terminal_outcome.as_deref(),
        Some("cancelled")
    );
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
    let submission_deadline_at = repo::begin_execution_submission(
        pool,
        execution.uuid,
        backend,
        session_id,
        None,
        1_800,
        (backend == "slurm").then_some("integration-target-sha"),
    )
    .await
    .unwrap()
    .expect("submission intent deadline");
    assert!(repo::begin_execution_submission(
        pool,
        execution.uuid,
        backend,
        session_id,
        None,
        1_800,
        (backend == "slurm").then_some("integration-target-sha"),
    )
    .await
    .unwrap()
    .is_none());
    let execution = repo::get_execution(pool, execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        execution.submission_deadline_at,
        Some(submission_deadline_at)
    );
    let intent = repo::list_execution_observations(pool, execution.uuid, 10, 0)
        .await
        .unwrap()
        .into_iter()
        .find(|observation| observation.raw_state.as_deref() == Some("intent_persisted"))
        .expect("submission intent observation");
    assert_eq!(
        submission_deadline_at
            .signed_duration_since(intent.observed_at)
            .num_seconds(),
        1_800
    );
    assert_eq!(intent.payload["submission_timeout_seconds"], 1_800);
    assert_eq!(
        intent.payload["submission_deadline_at"],
        serde_json::to_value(submission_deadline_at).unwrap()
    );
    execution
}

async fn prepare_abandonable_slurm_submission(
    pool: &sqlx::PgPool,
    module: &str,
    session_id: &str,
) -> (
    beampipe_db::models::ExecutionRow,
    beampipe_db::models::ExecutionObservationRow,
) {
    let execution = prepare_submission_receipt_execution(pool, module, "slurm", session_id).await;
    let now = Utc::now();
    let intent_at = now - Duration::hours(26);
    let deadline_at = intent_at + Duration::minutes(30);
    let profile_sha256 = "integration-profile-sha";
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET submission_deadline_at = $2,
            deployment_profile_snapshot = $3
        WHERE uuid = $1
        "#,
    )
    .bind(execution.uuid)
    .bind(deadline_at)
    .bind(json!({
        "spec_sha256": profile_sha256,
        "deployment": {"kind": "slurm_remote", "login_node": "offline.invalid"},
    }))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE execution_observations
        SET observed_at = $2,
            payload = payload || jsonb_build_object('submission_deadline_at', to_jsonb($3::timestamptz))
        WHERE execution_id = $1
          AND kind = 'daliuge_session'
          AND raw_state = 'intent_persisted'
        "#,
    )
    .bind(execution.uuid)
    .bind(intent_at)
    .bind(deadline_at)
    .execute(pool)
    .await
    .unwrap();
    let intent = repo::latest_submission_intent_observation(pool, execution.uuid)
        .await
        .unwrap()
        .unwrap();
    for minutes_ago in [11, 6, 1] {
        let completed_at = now - Duration::minutes(minutes_ago);
        repo::record_slurm_name_lookup(
            pool,
            execution.uuid,
            repo::SlurmNameLookupRecordInput {
                lookup_id: Uuid::now_v7(),
                intent_observation_id: intent.uuid,
                daliuge_session_id: session_id.into(),
                profile_sha256: profile_sha256.into(),
                target_fingerprint: "integration-target-sha".into(),
                accounting_not_before: intent.observed_at,
                query_started_at: completed_at - Duration::seconds(1),
                query_completed_at: completed_at,
                squeue_complete: true,
                sacct_complete: true,
                outcome: repo::SlurmNameLookupOutcome::NotFound,
            },
        )
        .await
        .unwrap();
    }
    (
        repo::get_execution(pool, execution.uuid)
            .await
            .unwrap()
            .unwrap(),
        intent,
    )
}

#[tokio::test]
async fn unresolved_slurm_abandonment_is_atomic_and_late_receipt_never_reopens() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let session_id = format!("BeampipeExecution-abandon-{}", Uuid::now_v7());
    let (execution, _) = prepare_abandonable_slurm_submission(
        &pool,
        &format!("abandon_{}", Uuid::now_v7().simple()),
        &session_id,
    )
    .await;
    let deadline = execution.submission_deadline_at.unwrap();
    let queued_job = repo::enqueue_job_with_options(
        &pool,
        "execute",
        json!({"execution_id": execution.uuid, "fence": "queued"}),
        repo::JobEnqueueOptions {
            execution_id: Some(execution.uuid),
            idempotency_key: Some(format!("abandon-fence-queued:{}", execution.uuid)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let expired_job = repo::enqueue_job_with_options(
        &pool,
        "execute",
        json!({"execution_id": execution.uuid, "fence": "expired-running"}),
        repo::JobEnqueueOptions {
            execution_id: Some(execution.uuid),
            idempotency_key: Some(format!("abandon-fence-expired:{}", execution.uuid)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let old_activity = Utc::now() - Duration::hours(26);
    sqlx::query(
        r#"
        UPDATE jobs
        SET created_at = $2,
            updated_at = $2,
            next_run_at = $2
        WHERE uuid = $1
        "#,
    )
    .bind(queued_job.uuid)
    .bind(old_activity)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE jobs
        SET status = 'running',
            created_at = $2,
            updated_at = $2,
            heartbeat_at = $2,
            locked_until = $2,
            lease_expires_at = $2,
            lease_token = $3
        WHERE uuid = $1
        "#,
    )
    .bind(expired_job.uuid)
    .bind(old_activity)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
    let abandoned = repo::abandon_slurm_submission(
        &pool,
        execution.uuid,
        repo::AbandonSlurmSubmissionInput {
            actor: "user:integration-superuser".into(),
            correlation_id: Some("abandon:integration".into()),
            reason: "remote submission remains unresolved after attended review".into(),
            expected_submission_state: SubmissionState::InFlight,
            expected_daliuge_session_id: session_id.clone(),
            expected_submission_deadline_at: deadline,
            acknowledge_external_job_may_exist: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(abandoned.status, "failed");
    assert!(abandoned.execution_phase.is_none());
    assert_eq!(abandoned.control_phase.as_deref(), Some("terminal"));
    assert_eq!(abandoned.submission_state.as_deref(), Some("in_flight"));
    assert_eq!(abandoned.terminal_outcome.as_deref(), Some("inconsistent"));
    assert_eq!(
        abandoned.failure_class.as_deref(),
        Some("inconsistent_state")
    );
    assert!(abandoned.submission_abandoned_at.is_some());
    for job_id in [queued_job.uuid, expired_job.uuid] {
        let fenced: (
            String,
            Option<chrono::DateTime<Utc>>,
            Option<Uuid>,
            Option<Uuid>,
            Option<chrono::DateTime<Utc>>,
            Option<chrono::DateTime<Utc>>,
        ) = sqlx::query_as(
            r#"
            SELECT status, locked_until, lease_owner, lease_token, lease_expires_at, heartbeat_at
            FROM jobs WHERE uuid = $1
            "#,
        )
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fenced.0, "cancelled");
        assert!(fenced.1.is_none());
        assert!(fenced.2.is_none());
        assert!(fenced.3.is_none());
        assert!(fenced.4.is_none());
        assert!(fenced.5.is_none());
    }
    let abandonment_event = repo::list_provenance_events_for_execution(&pool, execution.uuid, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "execution.submission_abandoned")
        .expect("durable abandonment provenance");
    let invalidated_ids = abandonment_event.payload["invalidated_execute_job_ids"]
        .as_array()
        .unwrap();
    assert!(invalidated_ids.contains(&json!(queued_job.uuid)));
    assert!(invalidated_ids.contains(&json!(expired_job.uuid)));
    assert!(repo::begin_execution_submission(
        &pool,
        execution.uuid,
        "slurm",
        &session_id,
        None,
        1_800,
        Some("integration-target-sha"),
    )
    .await
    .unwrap()
    .is_none());
    let enqueue_after_abandonment = repo::enqueue_job_with_options(
        &pool,
        "execute",
        json!({"execution_id": execution.uuid, "fence": "after-abandonment"}),
        repo::JobEnqueueOptions {
            execution_id: Some(execution.uuid),
            idempotency_key: Some(format!("abandon-fence-late:{}", execution.uuid)),
            ..Default::default()
        },
    )
    .await
    .expect_err("operator abandonment must fence future execute work");
    assert!(enqueue_after_abandonment
        .to_string()
        .contains("terminal or operator-abandoned"));
    assert!(!repo::list_slurm_submissions_pending_reconciliation(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == execution.uuid));

    let session_dir = format!("/remote/sessions/{session_id}");
    let late_input = repo::SubmissionReceiptInput {
        scheduler_name: "slurm".into(),
        scheduler_job_id: Some("4242".into()),
        daliuge_session_id: Some(session_id),
        remote_session_dir: Some(session_dir.clone()),
        staging_root: Some(format!("{session_dir}/wallaby-staging")),
        workflow_manifest: json!({"late_after_abandonment": true}),
        physical_graph: json!([{"oid": "late-drop"}]),
        next_status: ExecutionStatus::AwaitingScheduler,
        actor: "system:late-worker".into(),
        correlation_id: Some("receipt:late-after-abandonment".into()),
        poll_job: None,
    };
    let late = repo::record_submission_receipt(&pool, execution.uuid, late_input.clone())
        .await
        .unwrap();
    assert!(late.late_after_abandonment);
    assert_eq!(late.execution.status, "failed");
    assert_eq!(late.execution.control_phase.as_deref(), Some("terminal"));
    assert_eq!(
        late.execution.submission_state.as_deref(),
        Some("in_flight")
    );
    assert_eq!(
        late.execution.terminal_outcome.as_deref(),
        Some("inconsistent")
    );
    assert_eq!(late.execution.scheduler_job_id.as_deref(), Some("4242"));
    assert!(late.execution.physical_graph_sha256.is_some());
    let events_before_replay =
        repo::list_provenance_events_for_execution(&pool, execution.uuid, 100)
            .await
            .unwrap();
    assert!(events_before_replay
        .iter()
        .any(|event| event.event_type == "execution.submission_detected_after_abandonment"));
    let replay = repo::record_submission_receipt(&pool, execution.uuid, late_input)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert!(replay.late_after_abandonment);
    assert_eq!(
        replay.physical_graph_artifact.uuid,
        late.physical_graph_artifact.uuid
    );
    assert_eq!(
        repo::list_execution_artifacts(&pool, execution.uuid)
            .await
            .unwrap()
            .iter()
            .filter(|artifact| artifact.kind == "physical_graph")
            .count(),
        1
    );
    assert_eq!(
        repo::list_provenance_events_for_execution(&pool, execution.uuid, 100)
            .await
            .unwrap()
            .len(),
        events_before_replay.len()
    );
}

#[tokio::test]
async fn abandonment_rejects_an_active_execute_lease() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let queue = format!("abandon_queue_{}", Uuid::now_v7().simple());
    let worker = Uuid::now_v7();
    repo::register_worker_instance(
        &pool,
        &worker_registration(worker, &queue, &["daliuge-deployment"]),
    )
    .await
    .unwrap();
    let session_id = format!("BeampipeExecution-abandon-lease-{}", Uuid::now_v7());
    let (execution, intent) = prepare_abandonable_slurm_submission(
        &pool,
        &format!("abandon_lease_{}", Uuid::now_v7().simple()),
        &session_id,
    )
    .await;
    repo::enqueue_job_with_options(
        &pool,
        "execute",
        json!({"execution_id": execution.uuid}),
        repo::JobEnqueueOptions {
            execution_id: Some(execution.uuid),
            idempotency_key: Some(format!("abandon-active:{}", execution.uuid)),
            pool: Some(queue.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    repo::claim_next_job_for_worker(&pool, worker, &queue, &["daliuge-deployment".into()], 60)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        r#"
        UPDATE batch_execution_record
        SET scheduler_state = 'pending',
            failure_class = 'timeout',
            last_error = 'live submitter context'
        WHERE uuid = $1
        "#,
    )
    .bind(execution.uuid)
    .execute(&pool)
    .await
    .unwrap();
    let lookup_at = Utc::now();
    let raced_lookup = repo::record_slurm_name_lookup(
        &pool,
        execution.uuid,
        repo::SlurmNameLookupRecordInput {
            lookup_id: Uuid::now_v7(),
            intent_observation_id: intent.uuid,
            daliuge_session_id: session_id.clone(),
            profile_sha256: "integration-profile-sha".into(),
            target_fingerprint: "integration-target-sha".into(),
            accounting_not_before: intent.observed_at,
            query_started_at: lookup_at - Duration::seconds(1),
            query_completed_at: lookup_at,
            squeue_complete: true,
            sacct_complete: true,
            outcome: repo::SlurmNameLookupOutcome::NotFound,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        raced_lookup.execution.scheduler_state.as_deref(),
        Some("pending")
    );
    assert_eq!(
        raced_lookup.execution.failure_class.as_deref(),
        Some("timeout")
    );
    assert_eq!(
        raced_lookup.execution.last_error.as_deref(),
        Some("live submitter context")
    );
    assert_eq!(
        raced_lookup.observation.payload["eligible_for_abandonment"],
        false
    );
    let error = repo::abandon_slurm_submission(
        &pool,
        execution.uuid,
        repo::AbandonSlurmSubmissionInput {
            actor: "user:integration-superuser".into(),
            correlation_id: None,
            reason: "must not win the worker lease race".into(),
            expected_submission_state: SubmissionState::InFlight,
            expected_daliuge_session_id: session_id,
            expected_submission_deadline_at: execution.submission_deadline_at.unwrap(),
            acknowledge_external_job_may_exist: true,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "submission_abandonment_active_execute_lease");
    assert!(repo::get_execution(&pool, execution.uuid)
        .await
        .unwrap()
        .unwrap()
        .submission_abandoned_at
        .is_none());
}

#[tokio::test]
async fn abandonment_rejects_a_scheduler_match_after_negative_evidence() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let session_id = format!("BeampipeExecution-abandon-ambiguous-{}", Uuid::now_v7());
    let (execution, intent) = prepare_abandonable_slurm_submission(
        &pool,
        &format!("abandon_ambiguous_{}", Uuid::now_v7().simple()),
        &session_id,
    )
    .await;
    let lookup_at = Utc::now();
    repo::record_slurm_name_lookup(
        &pool,
        execution.uuid,
        repo::SlurmNameLookupRecordInput {
            lookup_id: Uuid::now_v7(),
            intent_observation_id: intent.uuid,
            daliuge_session_id: session_id.clone(),
            profile_sha256: "integration-profile-sha".into(),
            target_fingerprint: "integration-target-sha".into(),
            accounting_not_before: intent.observed_at,
            query_started_at: lookup_at - Duration::seconds(1),
            query_completed_at: lookup_at,
            squeue_complete: true,
            sacct_complete: true,
            outcome: repo::SlurmNameLookupOutcome::Ambiguous {
                scheduler_job_ids: vec!["6101".into(), "6102".into()],
            },
        },
    )
    .await
    .unwrap();
    let error = repo::abandon_slurm_submission(
        &pool,
        execution.uuid,
        repo::AbandonSlurmSubmissionInput {
            actor: "user:integration-superuser".into(),
            correlation_id: None,
            reason: "ambiguous scheduler evidence must prevent abandonment".into(),
            expected_submission_state: SubmissionState::InFlight,
            expected_daliuge_session_id: session_id,
            expected_submission_deadline_at: execution.submission_deadline_at.unwrap(),
            acknowledge_external_job_may_exist: true,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.code(),
        "submission_abandonment_scheduler_match_observed"
    );
}

#[tokio::test]
async fn submission_receipt_winning_the_row_lock_prevents_abandonment() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let session_id = format!("BeampipeExecution-receipt-first-{}", Uuid::now_v7());
    let execution = prepare_submission_receipt_execution(
        &pool,
        &format!("receipt_first_{}", Uuid::now_v7().simple()),
        "slurm",
        &session_id,
    )
    .await;
    let intent = repo::latest_submission_intent_observation(&pool, execution.uuid)
        .await
        .unwrap()
        .unwrap();
    let mismatch_at = Utc::now();
    let mismatch = repo::record_slurm_name_lookup(
        &pool,
        execution.uuid,
        repo::SlurmNameLookupRecordInput {
            lookup_id: Uuid::now_v7(),
            intent_observation_id: intent.uuid,
            daliuge_session_id: session_id.clone(),
            profile_sha256: "integration-profile-sha".into(),
            target_fingerprint: "different-resolved-user".into(),
            accounting_not_before: intent.observed_at,
            query_started_at: mismatch_at - Duration::seconds(1),
            query_completed_at: mismatch_at,
            squeue_complete: true,
            sacct_complete: true,
            outcome: repo::SlurmNameLookupOutcome::NotFound,
        },
    )
    .await
    .unwrap_err();
    assert!(mismatch.to_string().contains("resolved target"));
    let session_dir = format!("/remote/sessions/{session_id}");
    let receipt = repo::record_submission_receipt(
        &pool,
        execution.uuid,
        repo::SubmissionReceiptInput {
            scheduler_name: "slurm".into(),
            scheduler_job_id: Some("5252".into()),
            daliuge_session_id: Some(session_id.clone()),
            remote_session_dir: Some(session_dir.clone()),
            staging_root: Some(format!("{session_dir}/wallaby-staging")),
            workflow_manifest: json!({"receipt_first": true}),
            physical_graph: json!([{"oid": "receipt-first-drop"}]),
            next_status: ExecutionStatus::AwaitingScheduler,
            actor: "system:receipt-first".into(),
            correlation_id: None,
            poll_job: None,
        },
    )
    .await
    .unwrap();
    assert!(!receipt.late_after_abandonment);
    let lookup_at = Utc::now();
    let stale_lookup = repo::record_slurm_name_lookup(
        &pool,
        execution.uuid,
        repo::SlurmNameLookupRecordInput {
            lookup_id: Uuid::now_v7(),
            intent_observation_id: intent.uuid,
            daliuge_session_id: session_id.clone(),
            profile_sha256: "integration-profile-sha".into(),
            target_fingerprint: "integration-target-sha".into(),
            accounting_not_before: intent.observed_at,
            query_started_at: lookup_at - Duration::seconds(1),
            query_completed_at: lookup_at,
            squeue_complete: true,
            sacct_complete: true,
            outcome: repo::SlurmNameLookupOutcome::NotFound,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        stale_lookup.observation.payload["canonical_state_mutation_allowed"],
        false
    );
    assert_eq!(
        stale_lookup.observation.payload["eligible_for_abandonment"],
        false
    );
    assert_eq!(
        stale_lookup.execution.submission_state.as_deref(),
        Some("submitted")
    );
    assert_eq!(
        stale_lookup.execution.scheduler_job_id.as_deref(),
        Some("5252")
    );
    let error = repo::abandon_slurm_submission(
        &pool,
        execution.uuid,
        repo::AbandonSlurmSubmissionInput {
            actor: "user:integration-superuser".into(),
            correlation_id: None,
            reason: "stale operator review must lose to the receipt".into(),
            expected_submission_state: SubmissionState::InFlight,
            expected_daliuge_session_id: session_id,
            expected_submission_deadline_at: execution.submission_deadline_at.unwrap(),
            acknowledge_external_job_may_exist: true,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "submission_abandonment_cas_mismatch");
    let current = repo::get_execution(&pool, execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.status, "awaiting_scheduler");
    assert_eq!(current.submission_state.as_deref(), Some("submitted"));
    assert_eq!(current.scheduler_job_id.as_deref(), Some("5252"));
    assert!(current.submission_abandoned_at.is_none());
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
    assert_eq!(
        recorded.execution.control_phase.as_deref(),
        Some("submitted")
    );
    assert_eq!(
        recorded.execution.submission_state.as_deref(),
        Some("submitted")
    );
    assert_eq!(
        recorded.execution.scheduler_state.as_deref(),
        Some("pending")
    );
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
    assert!(
        recorded.physical_graph_artifact.metadata["submission_receipt_sha256"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );

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
    assert_eq!(
        unchanged.physical_graph_sha256,
        recorded.execution.physical_graph_sha256
    );
}

#[tokio::test]
async fn recovered_exact_slurm_id_stays_uncertain_until_the_receipt_commits() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };
    let module = format!("receipt_recovered_{}", Uuid::now_v7().simple());
    let session_id = format!("BeampipeExecution-{}", Uuid::now_v7());
    let execution =
        prepare_submission_receipt_execution(&pool, &module, "slurm", &session_id).await;
    repo::apply_execution_state_patch(
        &pool,
        execution.uuid,
        ExecutionStatePatch {
            submission_state: Some(SubmissionState::Uncertain),
            scheduler_job_id: Some("4242".into()),
            scheduler_state: Some(beampipe_domain::SchedulerState::Running),
            scheduler_raw_state: Some("RUNNING".into()),
            scheduler_reason: Some("Resources".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let recovered = repo::get_execution(&pool, execution.uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.status, "awaiting_scheduler");
    assert_eq!(
        recovered.control_phase.as_deref(),
        Some("submission_pending")
    );
    assert_eq!(recovered.submission_state.as_deref(), Some("uncertain"));
    assert_eq!(recovered.scheduler_job_id.as_deref(), Some("4242"));
    assert_eq!(recovered.scheduler_state.as_deref(), Some("running"));
    assert!(recovered.physical_graph_sha256.is_none());
    assert!(repo::begin_execution_submission(
        &pool,
        execution.uuid,
        "slurm",
        &session_id,
        None,
        1_800,
        Some("recovered-target"),
    )
    .await
    .unwrap()
    .is_none());
    assert!(!repo::list_slurm_submissions_pending_reconciliation(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == execution.uuid));
    assert!(repo::list_slurm_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == execution.uuid));

    let session_dir = format!("/remote/sessions/{session_id}");
    let recorded = repo::record_submission_receipt(
        &pool,
        execution.uuid,
        repo::SubmissionReceiptInput {
            scheduler_name: "slurm".into(),
            scheduler_job_id: Some("4242".into()),
            daliuge_session_id: Some(session_id),
            remote_session_dir: Some(session_dir.clone()),
            staging_root: Some(format!("{session_dir}/wallaby-staging")),
            workflow_manifest: json!({
                "sources": [],
                "beampipe_run_record": {"slurm": {"job_id": "4242"}},
            }),
            physical_graph: json!([{"oid": "drop-recovered"}]),
            next_status: ExecutionStatus::AwaitingScheduler,
            actor: "system:test".into(),
            correlation_id: Some("receipt-recovered".into()),
            poll_job: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(recorded.execution.status, "running");
    assert_eq!(
        recorded.execution.control_phase.as_deref(),
        Some("submitted")
    );
    assert_eq!(
        recorded.execution.submission_state.as_deref(),
        Some("submitted")
    );
    assert_eq!(
        recorded.execution.scheduler_state.as_deref(),
        Some("running")
    );
    assert!(recorded.execution.physical_graph_sha256.is_some());
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
async fn reconciliation_selectors_wait_for_the_active_execute_lease() {
    let Some(pool) = test_pool().await else {
        eprintln!("DATABASE_URL not set; skipping integration test");
        return;
    };

    let slurm_queue = format!("selector_slurm_{}", Uuid::now_v7().simple());
    let slurm_worker = Uuid::now_v7();
    repo::register_worker_instance(
        &pool,
        &worker_registration(slurm_worker, &slurm_queue, &["daliuge-deployment"]),
    )
    .await
    .unwrap();
    let slurm_execution = repo::create_execution(
        &pool,
        &format!("sel_slurm_{}", Uuid::now_v7().simple()),
        json!([{"source_identifier": "source-slurm"}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::begin_execution_submission(
        &pool,
        slurm_execution.uuid,
        "slurm",
        "BeampipeExecution-selector-slurm",
        None,
        1_800,
        Some("selector-slurm-target"),
    )
    .await
    .unwrap()
    .is_some());
    let slurm_execute = repo::enqueue_job_with_options(
        &pool,
        "execute",
        json!({"execution_id": slurm_execution.uuid}),
        repo::JobEnqueueOptions {
            execution_id: Some(slurm_execution.uuid),
            idempotency_key: Some(format!("selector:slurm:{}", slurm_execution.uuid)),
            pool: Some(slurm_queue.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    repo::claim_next_job_for_worker(
        &pool,
        slurm_worker,
        &slurm_queue,
        &["daliuge-deployment".into()],
        60,
    )
    .await
    .unwrap()
    .expect("execute job must be actively leased");
    assert!(!repo::list_slurm_submissions_pending_reconciliation(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == slurm_execution.uuid));

    set_job_lease_expired(&pool, slurm_execute.uuid).await;
    assert!(repo::list_slurm_submissions_pending_reconciliation(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == slurm_execution.uuid));

    repo::apply_execution_state_patch(
        &pool,
        slurm_execution.uuid,
        ExecutionStatePatch {
            submission_state: Some(SubmissionState::Uncertain),
            scheduler_job_id: Some("4242".into()),
            scheduler_state: Some(beampipe_domain::SchedulerState::Running),
            scheduler_raw_state: Some("RUNNING".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    set_job_lease_active(&pool, slurm_execute.uuid).await;
    assert!(!repo::list_slurm_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == slurm_execution.uuid));
    set_job_lease_partially_active(&pool, slurm_execute.uuid).await;
    assert!(!repo::list_slurm_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == slurm_execution.uuid));
    set_job_lease_expired(&pool, slurm_execute.uuid).await;
    assert!(repo::list_slurm_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == slurm_execution.uuid));
    repo::complete_job(&pool, slurm_execute.uuid).await.unwrap();

    let rest_queue = format!("selector_rest_{}", Uuid::now_v7().simple());
    let rest_worker = Uuid::now_v7();
    repo::register_worker_instance(
        &pool,
        &worker_registration(rest_worker, &rest_queue, &["daliuge-deployment"]),
    )
    .await
    .unwrap();
    let rest_execution = repo::create_execution(
        &pool,
        &format!("sel_rest_{}", Uuid::now_v7().simple()),
        json!([{"source_identifier": "source-rest"}]),
        "casda",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(repo::begin_execution_submission(
        &pool,
        rest_execution.uuid,
        "daliuge",
        "BeampipeExecution-selector-rest",
        Some("http://dim.invalid"),
        1_800,
        None,
    )
    .await
    .unwrap()
    .is_some());
    let rest_execute = repo::enqueue_job_with_options(
        &pool,
        "execute",
        json!({"execution_id": rest_execution.uuid}),
        repo::JobEnqueueOptions {
            execution_id: Some(rest_execution.uuid),
            idempotency_key: Some(format!("selector:rest:{}", rest_execution.uuid)),
            pool: Some(rest_queue.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    repo::claim_next_job_for_worker(
        &pool,
        rest_worker,
        &rest_queue,
        &["daliuge-deployment".into()],
        60,
    )
    .await
    .unwrap()
    .expect("REST execute job must be actively leased");
    assert!(!repo::list_rest_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == rest_execution.uuid));
    set_job_lease_incompletely_fenced(&pool, rest_execute.uuid).await;
    assert!(!repo::list_rest_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == rest_execution.uuid));
    set_job_lease_expired(&pool, rest_execute.uuid).await;
    assert!(repo::list_rest_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == rest_execution.uuid));
    repo::complete_job(&pool, rest_execute.uuid).await.unwrap();

    let dim_poll = repo::enqueue_job_with_options(
        &pool,
        "dim_poll",
        json!({"execution_id": rest_execution.uuid}),
        repo::JobEnqueueOptions {
            execution_id: Some(rest_execution.uuid),
            idempotency_key: Some(format!("selector:dim-poll:{}", rest_execution.uuid)),
            pool: Some(rest_queue.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let claimed_dim = repo::claim_next_job_for_worker(
        &pool,
        rest_worker,
        &rest_queue,
        &["daliuge-deployment".into()],
        60,
    )
    .await
    .unwrap()
    .expect("DIM poll job must be actively leased");
    assert_eq!(claimed_dim.uuid, dim_poll.uuid);
    assert!(repo::list_rest_executions_pending_poll(&pool)
        .await
        .unwrap()
        .iter()
        .any(|row| row.uuid == rest_execution.uuid));
}

async fn set_job_lease_active(pool: &sqlx::PgPool, job_id: Uuid) {
    sqlx::query(
        "UPDATE jobs SET status = 'running', lease_expires_at = now() + interval '60 seconds', locked_until = now() + interval '60 seconds' WHERE uuid = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn set_job_lease_partially_active(pool: &sqlx::PgPool, job_id: Uuid) {
    sqlx::query(
        "UPDATE jobs SET status = 'running', lease_expires_at = now() - interval '1 second', locked_until = now() + interval '60 seconds' WHERE uuid = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn set_job_lease_incompletely_fenced(pool: &sqlx::PgPool, job_id: Uuid) {
    sqlx::query(
        "UPDATE jobs SET status = 'running', lease_expires_at = NULL, locked_until = now() - interval '1 second' WHERE uuid = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn set_job_lease_expired(pool: &sqlx::PgPool, job_id: Uuid) {
    sqlx::query(
        "UPDATE jobs SET lease_expires_at = now() - interval '1 second', locked_until = now() - interval '1 second' WHERE uuid = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await
    .unwrap();
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

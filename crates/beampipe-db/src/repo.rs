use crate::models::{
    ArchiveMetadataRow, DeploymentProfileRow, ExecutionArtifactInput, ExecutionArtifactRow,
    ExecutionObservationInput, ExecutionObservationRow, ExecutionProvenancePatch, ExecutionRow,
    ExecutionStatePatch, JobClaimHistoryRow, JobRow, OperatorOverviewCounts, ProjectConfigRow,
    SourceRegistryRow, WorkerInstanceRow, WorkerPoolSummary, WorkerRegistration,
};
use beampipe_domain::{
    discovery::{
        discovery_signature, existing_signature_from_records, group_metadata_by_sbid,
        metadata_payload_by_sbid, no_datasets_payload, no_datasets_signature,
        validate_prepared_metadata_records, DiscoveryBatchStats, DiscoverySourceResult,
        SignatureOptions,
    },
    plan_execution_retry,
    readiness::{
        parsed_source_readiness_error, ArchiveMetadataReadiness, RegisteredSourceReadiness,
    },
    DaliugeState, ExecutionPhase, ExecutionRetryContext, ExecutionRetryPlan, ExecutionStatus,
    LedgerPatch, LedgerState, SchedulerState, SubmissionState, TerminalOutcome,
};
use beampipe_project::SignatureConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder};
use std::collections::BTreeMap;
use tracing::{debug, info};
use uuid::Uuid;

pub async fn upsert_source(
    pool: &PgPool,
    project_module: &str,
    source_identifier: &str,
    enabled: bool,
) -> Result<SourceRegistryRow, sqlx::Error> {
    sqlx::query_as::<_, SourceRegistryRow>(
        r#"
        INSERT INTO source_registry (uuid, project_module, source_identifier, enabled)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (project_module, source_identifier)
        DO UPDATE SET enabled = EXCLUDED.enabled
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_module)
    .bind(source_identifier)
    .bind(enabled)
    .fetch_one(pool)
    .await
}

pub async fn create_user(
    pool: &PgPool,
    name: &str,
    username: &str,
    email: &str,
    hashed_password: &str,
    is_superuser: bool,
) -> Result<crate::models::UserRow, sqlx::Error> {
    sqlx::query_as::<_, crate::models::UserRow>(
        r#"
        INSERT INTO users (uuid, name, username, email, hashed_password, is_superuser)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(username)
    .bind(email)
    .bind(hashed_password)
    .bind(is_superuser)
    .fetch_one(pool)
    .await
}

pub async fn get_user_by_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<crate::models::UserRow>, sqlx::Error> {
    sqlx::query_as::<_, crate::models::UserRow>(
        "SELECT * FROM users WHERE username = $1 AND is_deleted = false",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

pub async fn list_sources(
    pool: &PgPool,
    project_module: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<SourceRegistryRow>, sqlx::Error> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT * FROM source_registry");
    if let Some(module) = project_module {
        qb.push(" WHERE project_module = ").push_bind(module);
    }
    qb.push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    qb.build_query_as().fetch_all(pool).await
}

pub async fn get_source(pool: &PgPool, id: Uuid) -> Result<Option<SourceRegistryRow>, sqlx::Error> {
    sqlx::query_as::<_, SourceRegistryRow>("SELECT * FROM source_registry WHERE uuid = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn get_source_by_identifier(
    pool: &PgPool,
    project_module: &str,
    source_identifier: &str,
) -> Result<Option<SourceRegistryRow>, sqlx::Error> {
    sqlx::query_as::<_, SourceRegistryRow>(
        "SELECT * FROM source_registry WHERE project_module = $1 AND source_identifier = $2",
    )
    .bind(project_module)
    .bind(source_identifier)
    .fetch_optional(pool)
    .await
}

pub async fn update_source(
    pool: &PgPool,
    id: Uuid,
    enabled: Option<bool>,
    stale_after_hours: Option<i32>,
) -> Result<Option<SourceRegistryRow>, sqlx::Error> {
    sqlx::query_as::<_, SourceRegistryRow>(
        r#"
        UPDATE source_registry
        SET enabled = COALESCE($2, enabled),
            stale_after_hours = COALESCE($3, stale_after_hours)
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(enabled)
    .bind(stale_after_hours)
    .fetch_optional(pool)
    .await
}

pub async fn delete_source(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM source_registry WHERE uuid = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_source_metadata(
    pool: &PgPool,
    source: &SourceRegistryRow,
) -> Result<Vec<ArchiveMetadataRow>, sqlx::Error> {
    sqlx::query_as::<_, ArchiveMetadataRow>(
        r#"
        SELECT *
        FROM archive_metadata
        WHERE project_module = $1 AND source_identifier = $2
        ORDER BY sbid ASC, created_at ASC
        "#,
    )
    .bind(&source.project_module)
    .bind(&source.source_identifier)
    .fetch_all(pool)
    .await
}

pub async fn mark_sources_for_rediscovery(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: Option<&[String]>,
) -> Result<Vec<String>, sqlx::Error> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        r#"
        UPDATE source_registry
        SET last_checked_at = NULL,
            last_attempted_at = NULL,
            discovery_claim_token = NULL,
            discovery_claim_expires_at = NULL,
            workflow_claim_token = NULL,
            workflow_claimed_at = NULL,
            workflow_claim_expires_at = NULL
        WHERE project_module =
        "#,
    );
    qb.push_bind(project_module);
    qb.push(" AND enabled = true");
    if let Some(ids) = source_identifiers.filter(|ids| !ids.is_empty()) {
        qb.push(" AND source_identifier = ANY(")
            .push_bind(ids)
            .push(")");
    }
    qb.push(" RETURNING source_identifier");
    let rows = qb.build_query_scalar::<String>().fetch_all(pool).await?;
    Ok(rows)
}

pub async fn claim_source_rows_for_discovery(
    pool: &PgPool,
    project_module: Option<&str>,
    stale_after_hours: i32,
    limit: i64,
    lease_ttl_minutes: i64,
) -> Result<(Option<String>, Vec<(String, String)>), sqlx::Error> {
    if limit <= 0 {
        return Ok((None, Vec::new()));
    }
    let claim_token = Uuid::now_v7().to_string();
    let mut tx = pool.begin().await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        r#"
        SELECT uuid, project_module, source_identifier
        FROM source_registry
        WHERE enabled = true
          AND (discovery_claim_expires_at IS NULL OR discovery_claim_expires_at <= now())
          AND (
              last_checked_at IS NULL
              OR last_checked_at <= now() - (
        "#,
    );
    qb.push_bind(stale_after_hours);
    qb.push("::text || ' hours')::interval)");
    if let Some(module) = project_module {
        qb.push(" AND project_module = ").push_bind(module);
    }
    qb.push(" ORDER BY created_at ASC LIMIT ")
        .push_bind(limit)
        .push(" FOR UPDATE SKIP LOCKED");
    let rows: Vec<(Uuid, String, String)> = qb.build_query_as().fetch_all(&mut *tx).await?;
    if rows.is_empty() {
        tx.commit().await?;
        return Ok((None, Vec::new()));
    }
    let ids: Vec<Uuid> = rows.iter().map(|(id, _, _)| *id).collect();
    sqlx::query(
        r#"
        UPDATE source_registry
        SET discovery_claim_token = $1,
            discovery_claim_expires_at = now() + ($2::text || ' minutes')::interval
        WHERE uuid = ANY($3)
        "#,
    )
    .bind(&claim_token)
    .bind(lease_ttl_minutes)
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        Some(claim_token),
        rows.into_iter()
            .map(|(_, module, source)| (module, source))
            .collect(),
    ))
}

pub async fn release_discovery_claim(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
    claim_token: &str,
) -> Result<u64, sqlx::Error> {
    if source_identifiers.is_empty() || claim_token.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        UPDATE source_registry
        SET discovery_claim_token = NULL,
            discovery_claim_expires_at = NULL
        WHERE project_module = $1
          AND source_identifier = ANY($2)
          AND discovery_claim_token = $3
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .bind(claim_token)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn mark_sources_checked_if_claimed(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
    claim_token: &str,
) -> Result<u64, sqlx::Error> {
    if source_identifiers.is_empty() || claim_token.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        UPDATE source_registry
        SET last_checked_at = now()
        WHERE project_module = $1
          AND source_identifier = ANY($2)
          AND discovery_claim_token = $3
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .bind(claim_token)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn mark_sources_attempted_if_claimed(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
    claim_token: &str,
) -> Result<u64, sqlx::Error> {
    if source_identifiers.is_empty() || claim_token.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        UPDATE source_registry
        SET last_attempted_at = now()
        WHERE project_module = $1
          AND source_identifier = ANY($2)
          AND discovery_claim_token = $3
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .bind(claim_token)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn claim_pending_sources_for_workflow_run(
    pool: &PgPool,
    project_module: &str,
    limit: i64,
    lease_ttl_minutes: i64,
) -> Result<(Option<String>, Vec<String>), sqlx::Error> {
    if limit <= 0 {
        return Ok((None, Vec::new()));
    }
    let claim_token = Uuid::now_v7().to_string();
    let mut tx = pool.begin().await?;
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT uuid, source_identifier
        FROM source_registry
        WHERE project_module = $1
          AND enabled = true
          AND workflow_run_pending = true
          AND (workflow_claim_expires_at IS NULL OR workflow_claim_expires_at <= now())
        ORDER BY workflow_run_pending_at ASC NULLS LAST, created_at ASC
        LIMIT $2
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(project_module)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        tx.commit().await?;
        return Ok((None, Vec::new()));
    }
    let ids: Vec<Uuid> = rows.iter().map(|(id, _)| *id).collect();
    sqlx::query(
        r#"
        UPDATE source_registry
        SET workflow_claim_token = $1,
            workflow_claimed_at = now(),
            workflow_claim_expires_at = now() + ($2::text || ' minutes')::interval
        WHERE uuid = ANY($3)
        "#,
    )
    .bind(&claim_token)
    .bind(lease_ttl_minutes)
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        Some(claim_token),
        rows.into_iter().map(|(_, sid)| sid).collect(),
    ))
}

pub async fn release_workflow_claim(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
    claim_token: &str,
) -> Result<u64, sqlx::Error> {
    if source_identifiers.is_empty() || claim_token.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        UPDATE source_registry
        SET workflow_claim_token = NULL,
            workflow_claimed_at = NULL,
            workflow_claim_expires_at = NULL
        WHERE project_module = $1
          AND source_identifier = ANY($2)
          AND workflow_claim_token = $3
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .bind(claim_token)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn clear_workflow_pending_for_sources(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
) -> Result<u64, sqlx::Error> {
    if source_identifiers.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        UPDATE source_registry
        SET workflow_run_pending = false,
            workflow_run_pending_at = NULL,
            workflow_claim_token = NULL,
            workflow_claimed_at = NULL,
            workflow_claim_expires_at = NULL
        WHERE project_module = $1 AND source_identifier = ANY($2)
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn mark_sources_pending_workflow_run(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
) -> Result<u64, sqlx::Error> {
    if source_identifiers.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        UPDATE source_registry
        SET workflow_run_pending = true,
            workflow_run_pending_at = now(),
            workflow_claim_token = NULL,
            workflow_claimed_at = NULL,
            workflow_claim_expires_at = NULL
        WHERE project_module = $1 AND source_identifier = ANY($2)
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn set_last_executed_discovery_signature_for_sources(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
) -> Result<u64, sqlx::Error> {
    if source_identifiers.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        UPDATE source_registry
        SET last_executed_discovery_signature = discovery_signature
        WHERE project_module = $1
          AND source_identifier = ANY($2)
          AND discovery_signature IS NOT NULL
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn queue_depth(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'queued'")
        .fetch_one(pool)
        .await
}

pub async fn queue_depth_by_kind(pool: &PgPool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT kind, COUNT(*)::bigint
        FROM jobs
        WHERE status = 'queued'
        GROUP BY kind
        ORDER BY kind ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn oldest_queued_job_age_by_kind(
    pool: &PgPool,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT kind,
               COALESCE(EXTRACT(EPOCH FROM (now() - MIN(created_at)))::bigint, 0)
        FROM jobs
        WHERE status = 'queued'
        GROUP BY kind
        ORDER BY kind ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn jobs_running_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'running'")
        .fetch_one(pool)
        .await
}

/// Non-terminal executions grouped by ledger status.
pub async fn execution_counts_by_status(pool: &PgPool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT status, COUNT(*)::bigint
        FROM batch_execution_record
        WHERE status IN ('pending', 'running', 'awaiting_scheduler', 'retrying', 'not_submitted')
        GROUP BY status
        ORDER BY status ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn oldest_active_execution_age_by_status(
    pool: &PgPool,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT status,
               COALESCE(EXTRACT(EPOCH FROM (now() - MIN(created_at)))::bigint, 0)
        FROM batch_execution_record
        WHERE status IN ('pending', 'running', 'awaiting_scheduler', 'retrying', 'not_submitted')
        GROUP BY status
        ORDER BY status ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

/// Non-terminal executions grouped by scheduler backend.
pub async fn execution_counts_by_scheduler_name(
    pool: &PgPool,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT COALESCE(NULLIF(scheduler_name, ''), 'none'), COUNT(*)::bigint
        FROM batch_execution_record
        WHERE status IN ('pending', 'running', 'awaiting_scheduler', 'retrying', 'not_submitted')
        GROUP BY COALESCE(NULLIF(scheduler_name, ''), 'none')
        ORDER BY 1 ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn execution_counts_by_external_axis(
    pool: &PgPool,
) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT axis, state, COUNT(*)::BIGINT
        FROM (
            SELECT 'submission'::TEXT AS axis, COALESCE(submission_state, 'unknown')::TEXT AS state
            FROM batch_execution_record
            UNION ALL
            SELECT 'scheduler', COALESCE(scheduler_state, 'unknown')
            FROM batch_execution_record
            UNION ALL
            SELECT 'daliuge', COALESCE(daliuge_state, 'unknown')
            FROM batch_execution_record
            UNION ALL
            SELECT 'output', COALESCE(output_state, 'unknown')
            FROM batch_execution_record
        ) states
        GROUP BY axis, state
        ORDER BY axis, state
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn reconciliation_risk_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM batch_execution_record
        WHERE terminal_outcome = 'inconsistent'
           OR failure_class = 'inconsistent_state'
           OR submission_state = 'uncertain'
        "#,
    )
    .fetch_one(pool)
    .await
}

pub async fn execution_retry_total(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(SUM(retry_count), 0)::BIGINT FROM batch_execution_record")
        .fetch_one(pool)
        .await
}

pub async fn get_workflow_pending_stats(
    pool: &PgPool,
    project_module: &str,
) -> Result<(i64, Option<DateTime<Utc>>), sqlx::Error> {
    let count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM source_registry
        WHERE project_module = $1
          AND enabled = true
          AND workflow_run_pending = true
        "#,
    )
    .bind(project_module)
    .fetch_one(pool)
    .await?;
    let oldest = sqlx::query_scalar::<_, DateTime<Utc>>(
        r#"
        SELECT workflow_run_pending_at
        FROM source_registry
        WHERE project_module = $1
          AND enabled = true
          AND workflow_run_pending = true
        ORDER BY workflow_run_pending_at ASC NULLS LAST
        LIMIT 1
        "#,
    )
    .bind(project_module)
    .fetch_optional(pool)
    .await?;
    Ok((count, oldest))
}

pub async fn workflow_pending_counts_by_module(
    pool: &PgPool,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT project_module, COUNT(*)
        FROM source_registry
        WHERE enabled = true AND workflow_run_pending = true
        GROUP BY project_module
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn count_active_executions_with_different_spec(
    pool: &PgPool,
    project_module: &str,
    new_spec_sha256: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM batch_execution_record e
        JOIN project_configs pc ON pc.uuid = e.project_config_id
        WHERE e.project_module = $1
          AND e.status IN ('pending', 'running', 'retrying', 'awaiting_scheduler')
          AND pc.spec_sha256 <> $2
        "#,
    )
    .bind(project_module)
    .bind(new_spec_sha256)
    .fetch_one(pool)
    .await
}

pub async fn count_execute_in_flight_runs(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM batch_execution_record
        WHERE status IN ('pending', 'running', 'retrying', 'awaiting_scheduler')
        "#,
    )
    .fetch_one(pool)
    .await
}

pub async fn count_auto_in_flight_for_module(
    pool: &PgPool,
    project_module: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM batch_execution_record
        WHERE project_module = $1
          AND status IN ('pending', 'running', 'retrying', 'awaiting_scheduler')
        "#,
    )
    .bind(project_module)
    .fetch_one(pool)
    .await
}

pub async fn count_in_flight_for_profile(
    pool: &PgPool,
    profile_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM batch_execution_record
        WHERE deployment_profile_id = $1
          AND status IN ('pending', 'running', 'retrying', 'awaiting_scheduler')
        "#,
    )
    .bind(profile_id)
    .fetch_one(pool)
    .await
}

pub async fn list_active_project_configs(
    pool: &PgPool,
) -> Result<Vec<ProjectConfigRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectConfigRow>(
        "SELECT * FROM project_configs WHERE active = true ORDER BY project_id ASC",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_deployment_profile_by_name(
    pool: &PgPool,
    name: &str,
) -> Result<Option<DeploymentProfileRow>, sqlx::Error> {
    sqlx::query_as::<_, DeploymentProfileRow>(
        "SELECT * FROM daliuge_deployment_profile WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
}

pub async fn get_default_deployment_profile(
    pool: &PgPool,
    project_module: &str,
) -> Result<Option<DeploymentProfileRow>, sqlx::Error> {
    sqlx::query_as::<_, DeploymentProfileRow>(
        r#"
        SELECT *
        FROM daliuge_deployment_profile
        WHERE is_default = true
          AND (project_module = $1 OR project_module IS NULL)
        ORDER BY project_module IS NULL ASC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(project_module)
    .fetch_optional(pool)
    .await
}

pub async fn get_deployment_profile(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<DeploymentProfileRow>, sqlx::Error> {
    sqlx::query_as::<_, DeploymentProfileRow>(
        "SELECT * FROM daliuge_deployment_profile WHERE uuid = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn partition_sources_ready_for_execution(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
) -> Result<(Vec<String>, Vec<(String, String)>), sqlx::Error> {
    if source_identifiers.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let registry_rows: Vec<SourceRegistryRow> = sqlx::query_as(
        r#"
        SELECT *
        FROM source_registry
        WHERE project_module = $1 AND source_identifier = ANY($2)
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .fetch_all(pool)
    .await?;
    let metadata_rows =
        list_archive_metadata_for_sources(pool, project_module, source_identifiers).await?;
    let mut valid = Vec::new();
    let mut skipped = Vec::new();
    let mut already_executed = Vec::new();
    const ALREADY_EXECUTED: &str = "already executed for current discovery signature";
    for sid in source_identifiers {
        let reg = registry_rows.iter().find(|r| &r.source_identifier == sid);
        if let Some(r) = reg {
            if r.discovery_signature.is_some()
                && r.discovery_signature == r.last_executed_discovery_signature
            {
                skipped.push((sid.clone(), ALREADY_EXECUTED.into()));
                already_executed.push(sid.clone());
                continue;
            }
        }
        let readiness = reg.map(|r| RegisteredSourceReadiness {
            enabled: r.enabled,
            last_checked_at_present: r.last_checked_at.is_some(),
            discovery_signature: r.discovery_signature.clone(),
            discovery_claim_token: r.discovery_claim_token.clone(),
        });
        let metadata: Vec<ArchiveMetadataReadiness> = metadata_rows
            .iter()
            .filter(|r| &r.source_identifier == sid)
            .map(|r| ArchiveMetadataReadiness {
                sbid: r.sbid.clone(),
                metadata_json: r.metadata_json.clone(),
            })
            .collect();
        if let Some(err) = parsed_source_readiness_error(sid, None, readiness.as_ref(), &metadata) {
            skipped.push((sid.clone(), err));
        } else {
            valid.push(sid.clone());
        }
    }
    if !already_executed.is_empty() {
        clear_workflow_pending_for_sources(pool, project_module, &already_executed).await?;
    }
    Ok((valid, skipped))
}

fn signature_options_from_config(config: Option<&SignatureConfig>) -> SignatureOptions {
    config
        .map(|c| SignatureOptions {
            exclude_fields: c.exclude_fields.clone(),
            include_discovery_flags: c.include_discovery_flags,
        })
        .unwrap_or_default()
}

pub async fn persist_discovery_results(
    pool: &PgPool,
    project_module: &str,
    claim_token: &str,
    results: &[DiscoverySourceResult],
    signature_config: Option<&SignatureConfig>,
) -> Result<DiscoveryBatchStats, sqlx::Error> {
    let signature = signature_options_from_config(signature_config);
    let mut stats = DiscoveryBatchStats {
        total_sources: results.len(),
        ..DiscoveryBatchStats::default()
    };
    let mut checked = Vec::new();
    let mut attempted = Vec::new();

    for result in results {
        match result {
            DiscoverySourceResult::HasMetadata {
                source_identifier,
                metadata,
                discovery_flags,
                ..
            } => {
                match persist_changed_or_unchanged(
                    pool,
                    project_module,
                    source_identifier,
                    claim_token,
                    metadata,
                    discovery_flags,
                    &signature,
                )
                .await?
                {
                    PersistOutcome::Changed {
                        sbids, datasets, ..
                    } => {
                        stats.changed_count += 1;
                        stats.total_sbids += sbids;
                        stats.total_datasets += datasets;
                    }
                    PersistOutcome::Unchanged {
                        sbids, datasets, ..
                    } => {
                        stats.unchanged_count += 1;
                        stats.total_sbids += sbids;
                        stats.total_datasets += datasets;
                        checked.push(source_identifier.clone());
                    }
                    PersistOutcome::MissingRegistry => {
                        stats.missing_registry_count += 1;
                    }
                }
            }
            DiscoverySourceResult::NoDatasets {
                source_identifier, ..
            } => {
                if persist_no_datasets(pool, project_module, source_identifier, claim_token).await?
                {
                    stats.changed_count += 1;
                } else {
                    stats.unchanged_count += 1;
                    checked.push(source_identifier.clone());
                }
                stats.no_datasets_count += 1;
            }
            DiscoverySourceResult::Unchanged {
                source_identifier, ..
            } => {
                stats.unchanged_count += 1;
                checked.push(source_identifier.clone());
            }
            DiscoverySourceResult::Timeout {
                source_identifier,
                error,
                duration_ms,
                ..
            } => {
                stats.timeout_count += 1;
                stats.failed_sources.push(source_identifier.clone());
                attempted.push(source_identifier.clone());
                let payload = serde_json::json!({
                    "error": error,
                    "duration_ms": duration_ms,
                    "claim_token": claim_token,
                });
                crate::provenance::record_provenance_event(
                    pool,
                    beampipe_domain::provenance::ProvenanceEventType::DiscoveryTimeout.as_str(),
                    project_module,
                    Some(source_identifier.as_str()),
                    None,
                    Some("system:discovery"),
                    Some(claim_token),
                    &payload,
                )
                .await;
            }
            DiscoverySourceResult::Error {
                source_identifier,
                error,
                duration_ms,
                ..
            } => {
                stats.error_count += 1;
                stats.failed_sources.push(source_identifier.clone());
                attempted.push(source_identifier.clone());
                let payload = serde_json::json!({
                    "error": error,
                    "duration_ms": duration_ms,
                    "claim_token": claim_token,
                });
                crate::provenance::record_provenance_event(
                    pool,
                    beampipe_domain::provenance::ProvenanceEventType::DiscoveryError.as_str(),
                    project_module,
                    Some(source_identifier.as_str()),
                    None,
                    Some("system:discovery"),
                    Some(claim_token),
                    &payload,
                )
                .await;
            }
        }
    }

    mark_sources_checked_if_claimed(pool, project_module, &checked, claim_token).await?;
    mark_sources_attempted_if_claimed(pool, project_module, &attempted, claim_token).await?;
    let all_source_ids: Vec<String> = results
        .iter()
        .map(|r| match r {
            DiscoverySourceResult::HasMetadata {
                source_identifier, ..
            }
            | DiscoverySourceResult::NoDatasets {
                source_identifier, ..
            }
            | DiscoverySourceResult::Unchanged {
                source_identifier, ..
            }
            | DiscoverySourceResult::Timeout {
                source_identifier, ..
            }
            | DiscoverySourceResult::Error {
                source_identifier, ..
            } => source_identifier.clone(),
        })
        .collect();
    release_discovery_claim(pool, project_module, &all_source_ids, claim_token).await?;
    Ok(stats)
}

enum PersistOutcome {
    Changed { sbids: usize, datasets: usize },
    Unchanged { sbids: usize, datasets: usize },
    MissingRegistry,
}

async fn persist_changed_or_unchanged(
    pool: &PgPool,
    project_module: &str,
    source_identifier: &str,
    claim_token: &str,
    metadata: &[Value],
    discovery_flags: &Value,
    signature: &SignatureOptions,
) -> Result<PersistOutcome, sqlx::Error> {
    validate_prepared_metadata_records(metadata)
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let grouped = group_metadata_by_sbid(metadata);
    let payload = metadata_payload_by_sbid(&grouped, Some(discovery_flags), Some(signature));
    let new_sig = discovery_signature(&payload);
    let mut tx = pool.begin().await?;
    let source: Option<(Uuid, Option<String>)> = sqlx::query_as(
        r#"
        SELECT uuid, discovery_signature
        FROM source_registry
        WHERE project_module = $1
          AND source_identifier = $2
          AND discovery_claim_token = $3
        FOR UPDATE
        "#,
    )
    .bind(project_module)
    .bind(source_identifier)
    .bind(claim_token)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((source_id, stored_sig)) = source else {
        tx.rollback().await?;
        return Ok(PersistOutcome::MissingRegistry);
    };
    let existing_sig = if let Some(sig) = stored_sig {
        sig
    } else {
        let records: Vec<(String, Value)> = sqlx::query_as(
            r#"
            SELECT sbid, COALESCE(metadata_json, '{}'::jsonb)
            FROM archive_metadata
            WHERE project_module = $1 AND source_identifier = $2
            "#,
        )
        .bind(project_module)
        .bind(source_identifier)
        .fetch_all(&mut *tx)
        .await?;
        existing_signature_from_records(&records, Some(signature))
    };
    let sbids = payload.len();
    let datasets = metadata.len();
    if existing_sig == new_sig {
        debug!(
            project_module,
            source_identifier,
            signature_prefix = &new_sig[..16.min(new_sig.len())],
            "event=discover_signature_unchanged"
        );
        sqlx::query(
            r#"
            UPDATE source_registry
            SET last_checked_at = now()
            WHERE uuid = $1 AND discovery_claim_token = $2
            "#,
        )
        .bind(source_id)
        .bind(claim_token)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        crate::provenance::record_provenance_event(
            pool,
            "discovery.unchanged",
            project_module,
            Some(source_identifier),
            None,
            Some("system:discovery"),
            Some(claim_token),
            &serde_json::json!({"signature_prefix": &new_sig[..16.min(new_sig.len())]}),
        )
        .await;
        return Ok(PersistOutcome::Unchanged { sbids, datasets });
    }
    info!(
        project_module,
        source_identifier,
        existing_prefix = &existing_sig[..16.min(existing_sig.len())],
        new_prefix = &new_sig[..16.min(new_sig.len())],
        "event=discover_signature_changed"
    );
    let keep_sbids: Vec<String> = payload.keys().cloned().collect();
    sqlx::query(
        r#"
        DELETE FROM archive_metadata
        WHERE project_module = $1
          AND source_identifier = $2
          AND NOT (sbid = ANY($3))
        "#,
    )
    .bind(project_module)
    .bind(source_identifier)
    .bind(&keep_sbids)
    .execute(&mut *tx)
    .await?;
    for (sbid, metadata_json) in payload {
        sqlx::query(
            r#"
            INSERT INTO archive_metadata (uuid, project_module, source_identifier, sbid, metadata_json)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (project_module, source_identifier, sbid)
            DO UPDATE SET metadata_json = EXCLUDED.metadata_json, updated_at = now()
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_module)
        .bind(source_identifier)
        .bind(sbid)
        .bind(metadata_json)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        r#"
        UPDATE source_registry
        SET last_checked_at = now(),
            discovery_signature = $2,
            workflow_run_pending = true,
            workflow_run_pending_at = now()
        WHERE uuid = $1 AND discovery_claim_token = $3
        "#,
    )
    .bind(source_id)
    .bind(&new_sig)
    .bind(claim_token)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    crate::provenance::record_provenance_event(
        pool,
        "discovery.changed",
        project_module,
        Some(source_identifier),
        None,
        Some("system:discovery"),
        Some(claim_token),
        &serde_json::json!({
            "signature": new_sig,
            "sbids": sbids,
            "datasets": datasets,
        }),
    )
    .await;
    Ok(PersistOutcome::Changed { sbids, datasets })
}

async fn persist_no_datasets(
    pool: &PgPool,
    project_module: &str,
    source_identifier: &str,
    claim_token: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let source: Option<(Uuid, Option<String>)> = sqlx::query_as(
        r#"
        SELECT uuid, discovery_signature
        FROM source_registry
        WHERE project_module = $1
          AND source_identifier = $2
          AND discovery_claim_token = $3
        FOR UPDATE
        "#,
    )
    .bind(project_module)
    .bind(source_identifier)
    .bind(claim_token)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((source_id, stored_sig)) = source else {
        tx.rollback().await?;
        return Ok(false);
    };
    let sig = no_datasets_signature();
    if stored_sig.as_deref() == Some(&sig) {
        sqlx::query("UPDATE source_registry SET last_checked_at = now() WHERE uuid = $1")
            .bind(source_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(false);
    }
    sqlx::query(
        r#"
        DELETE FROM archive_metadata
        WHERE project_module = $1 AND source_identifier = $2 AND sbid <> '0'
        "#,
    )
    .bind(project_module)
    .bind(source_identifier)
    .execute(&mut *tx)
    .await?;
    for (sbid, metadata_json) in no_datasets_payload() {
        sqlx::query(
            r#"
            INSERT INTO archive_metadata (uuid, project_module, source_identifier, sbid, metadata_json)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (project_module, source_identifier, sbid)
            DO UPDATE SET metadata_json = EXCLUDED.metadata_json, updated_at = now()
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_module)
        .bind(source_identifier)
        .bind(sbid)
        .bind(metadata_json)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        r#"
        UPDATE source_registry
        SET last_checked_at = now(),
            discovery_signature = $2
        WHERE uuid = $1
        "#,
    )
    .bind(source_id)
    .bind(sig)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn list_executions_for_source(
    pool: &PgPool,
    source: &SourceRegistryRow,
    limit: i64,
    offset: i64,
) -> Result<Vec<ExecutionRow>, sqlx::Error> {
    sqlx::query_as::<_, ExecutionRow>(
        r#"
        SELECT *
        FROM batch_execution_record
        WHERE project_module = $1
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements(sources) AS elem
              WHERE elem->>'source_identifier' = $2
          )
        ORDER BY created_at DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(&source.project_module)
    .bind(&source.source_identifier)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn create_execution(
    pool: &PgPool,
    project_module: &str,
    sources: Value,
    archive_name: &str,
    deployment_profile_id: Option<Uuid>,
    project_config_id: Option<Uuid>,
    created_by_id: Option<i32>,
) -> Result<ExecutionRow, sqlx::Error> {
    create_execution_with_correlation(
        pool,
        project_module,
        sources,
        archive_name,
        deployment_profile_id,
        project_config_id,
        created_by_id,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn create_execution_with_correlation(
    pool: &PgPool,
    project_module: &str,
    sources: Value,
    archive_name: &str,
    deployment_profile_id: Option<Uuid>,
    project_config_id: Option<Uuid>,
    created_by_id: Option<i32>,
    correlation_id: Option<&str>,
) -> Result<ExecutionRow, sqlx::Error> {
    let resolved_profile = match deployment_profile_id {
        Some(id) => get_deployment_profile(pool, id).await?,
        None => get_default_deployment_profile(pool, project_module).await?,
    };
    if deployment_profile_id.is_some() && resolved_profile.is_none() {
        return Err(sqlx::Error::Protocol(
            "deployment profile does not exist".into(),
        ));
    }
    let resolved_profile_id = resolved_profile.as_ref().map(|profile| profile.uuid);
    let profile_revision = resolved_profile.as_ref().map(|profile| profile.revision);
    let profile_snapshot = resolved_profile.as_ref().map(|profile| {
        let spec_sha256 = profile.spec_sha256.clone().unwrap_or_else(|| {
            deployment_profile_spec_sha256(
                &profile.name,
                profile.description.as_deref(),
                profile.project_module.as_deref(),
                profile.is_default,
                profile.max_concurrent_executions,
                &profile.translation,
                &profile.deployment,
            )
        });
        serde_json::json!({
            "id": profile.uuid,
            "name": profile.name,
            "description": profile.description,
            "project_module": profile.project_module,
            "is_default": profile.is_default,
            "max_concurrent_executions": profile.max_concurrent_executions,
            "revision": profile.revision,
            "spec_sha256": spec_sha256,
            "translation": profile.translation,
            "deployment": profile.deployment,
        })
    });

    let mut tx = pool.begin().await?;
    if let Some(profile) = resolved_profile
        .as_ref()
        .filter(|profile| profile.max_concurrent_executions.is_some())
    {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("beampipe:profile-admission:{}", profile.uuid))
            .execute(&mut *tx)
            .await?;
        let in_flight: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM batch_execution_record
            WHERE deployment_profile_id = $1
              AND status IN ('pending', 'running', 'retrying', 'awaiting_scheduler')
            "#,
        )
        .bind(profile.uuid)
        .fetch_one(&mut *tx)
        .await?;
        if in_flight >= i64::from(profile.max_concurrent_executions.unwrap_or(i32::MAX)) {
            tx.rollback().await?;
            return Err(sqlx::Error::Protocol(format!(
                "deployment profile '{}' concurrency limit reached ({in_flight}/{})",
                profile.name,
                profile.max_concurrent_executions.unwrap_or_default()
            )));
        }
    }
    let id = Uuid::now_v7();
    let source_identifiers: Vec<String> = sources
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|source| {
            source
                .get("source_identifier")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let signatures: Vec<(String, Option<String>)> = if source_identifiers.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            r#"
            SELECT source_identifier, discovery_signature
            FROM source_registry
            WHERE project_module = $1 AND source_identifier = ANY($2)
            ORDER BY source_identifier
            "#,
        )
        .bind(project_module)
        .bind(&source_identifiers)
        .fetch_all(&mut *tx)
        .await?
    };
    let combined_discovery_signature = if signatures.is_empty() {
        None
    } else {
        let values: std::collections::BTreeMap<String, Value> = signatures
            .into_iter()
            .map(|(source, signature)| (source, Value::String(signature.unwrap_or_default())))
            .collect();
        Some(discovery_signature(&values))
    };
    let row = sqlx::query_as::<_, ExecutionRow>(
        r#"
        INSERT INTO batch_execution_record (
            uuid, project_module, sources, archive_name, deployment_profile_id,
            deployment_profile_revision, deployment_profile_snapshot,
            project_config_id, discovery_signature, created_by_id, status,
            control_phase, submission_state, scheduler_state, daliuge_state, output_state
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending',
            'discovered', 'not_started', 'not_submitted', 'not_created', 'not_started'
        )
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(project_module)
    .bind(sources.clone())
    .bind(archive_name)
    .bind(resolved_profile_id)
    .bind(profile_revision)
    .bind(profile_snapshot)
    .bind(project_config_id)
    .bind(combined_discovery_signature)
    .bind(created_by_id)
    .fetch_one(&mut *tx)
    .await?;
    let payload = serde_json::json!({"archive_name": archive_name, "sources": sources});
    insert_provenance_event(
        &mut *tx,
        "execution.created",
        project_module,
        None,
        Some(id),
        created_by_id.map(|_| "system:api"),
        correlation_id,
        &payload,
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn purge_provenance_events_older_than(
    pool: &PgPool,
    retention_days: i32,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        DELETE FROM provenance_events
        WHERE occurred_at < now() - ($1::int * interval '1 day')
        "#,
    )
    .bind(retention_days)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn get_execution(pool: &PgPool, id: Uuid) -> Result<Option<ExecutionRow>, sqlx::Error> {
    sqlx::query_as::<_, ExecutionRow>("SELECT * FROM batch_execution_record WHERE uuid = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Resolve the immutable deployment pinned to an execution, with a legacy fallback for
/// executions created before profile snapshots were introduced.
pub async fn resolve_execution_deployment(
    pool: &PgPool,
    execution: &ExecutionRow,
) -> Result<Option<Value>, sqlx::Error> {
    if let Some(deployment) = execution
        .deployment_profile_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get("deployment"))
    {
        return Ok(Some(deployment.clone()));
    }
    let profile = match execution.deployment_profile_id {
        Some(id) => get_deployment_profile(pool, id).await?,
        None => get_default_deployment_profile(pool, &execution.project_module).await?,
    };
    Ok(profile.map(|profile| profile.deployment))
}

pub async fn resolve_execution_translation(
    pool: &PgPool,
    execution: &ExecutionRow,
) -> Result<Option<Value>, sqlx::Error> {
    if let Some(translation) = execution
        .deployment_profile_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get("translation"))
    {
        return Ok(Some(translation.clone()));
    }
    let profile = match execution.deployment_profile_id {
        Some(id) => get_deployment_profile(pool, id).await?,
        None => get_default_deployment_profile(pool, &execution.project_module).await?,
    };
    Ok(profile.map(|profile| profile.translation))
}

#[derive(Debug, thiserror::Error)]
pub enum RetryExecutionError {
    #[error("execution not found")]
    NotFound,
    #[error("{code}: {message}")]
    Unsafe { code: String, message: String },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl RetryExecutionError {
    pub fn code(&self) -> &str {
        match self {
            Self::NotFound => "execution_not_found",
            Self::Unsafe { code, .. } => code,
            Self::Database(_) => "database_error",
        }
    }
}

#[derive(Debug)]
pub struct ExecutionRetryResult {
    pub execution: ExecutionRow,
    pub job: JobRow,
    pub plan: ExecutionRetryPlan,
}

pub async fn retry_execution(
    pool: &PgPool,
    id: Uuid,
    actor: &str,
    reason: &str,
    correlation_id: Option<&str>,
) -> Result<ExecutionRetryResult, RetryExecutionError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(RetryExecutionError::Unsafe {
            code: "retry_reason_required".into(),
            message: "a recovery reason is required for the audit trail".into(),
        });
    }
    if reason.len() > 1_000 {
        return Err(RetryExecutionError::Unsafe {
            code: "retry_reason_too_long".into(),
            message: "the recovery reason must be at most 1000 bytes".into(),
        });
    }

    let mut tx = pool.begin().await?;
    let execution = sqlx::query_as::<_, ExecutionRow>(
        "SELECT * FROM batch_execution_record WHERE uuid = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(RetryExecutionError::NotFound)?;
    let plan = plan_execution_retry(ExecutionRetryContext {
        status: execution.status_enum().unwrap_or(ExecutionStatus::Pending),
        phase: execution.phase_enum(),
        submission: execution
            .submission_state
            .as_deref()
            .and_then(SubmissionState::parse)
            .unwrap_or_default(),
        scheduler: execution
            .scheduler_state
            .as_deref()
            .and_then(SchedulerState::parse)
            .unwrap_or_default(),
        daliuge: execution
            .daliuge_state
            .as_deref()
            .and_then(DaliugeState::parse)
            .unwrap_or_default(),
        terminal_outcome: execution
            .terminal_outcome
            .as_deref()
            .and_then(TerminalOutcome::parse),
        has_manifest: execution.workflow_manifest.is_some(),
        has_scheduler_job_id: execution.scheduler_job_id.is_some(),
    })
    .map_err(|rejection| RetryExecutionError::Unsafe {
        code: rejection.code,
        message: rejection.message,
    })?;
    let active_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM jobs WHERE execution_id = $1 AND kind = 'execute' AND status IN ('queued', 'running')",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if active_jobs > 0 {
        return Err(RetryExecutionError::Unsafe {
            code: "execution_job_active".into(),
            message: "the previous execution job still owns active work; wait for it to finish before retrying"
                .into(),
        });
    }

    let source_identifiers: Vec<String> = execution
        .sources
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|source| source.get("source_identifier").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if !source_identifiers.is_empty() {
        let claims: Vec<(String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT source_identifier, workflow_claim_token
            FROM source_registry
            WHERE project_module = $1 AND source_identifier = ANY($2)
            FOR UPDATE
            "#,
        )
        .bind(&execution.project_module)
        .bind(&source_identifiers)
        .fetch_all(&mut *tx)
        .await?;
        if let Some((source, _)) = claims.iter().find(|(_, token)| token.is_some()) {
            return Err(RetryExecutionError::Unsafe {
                code: "source_admission_claimed".into(),
                message: format!(
                    "source {source} is already claimed for admission; let that claim finish before retrying"
                ),
            });
        }
        sqlx::query(
            r#"
            UPDATE source_registry
            SET workflow_run_pending = false,
                workflow_run_pending_at = NULL,
                workflow_claim_token = NULL,
                workflow_claimed_at = NULL,
                workflow_claim_expires_at = NULL
            WHERE project_module = $1 AND source_identifier = ANY($2)
            "#,
        )
        .bind(&execution.project_module)
        .bind(&source_identifiers)
        .execute(&mut *tx)
        .await?;
    }

    let (worker_pool, required_labels, priority): (String, Value, i32) = sqlx::query_as(
        r#"
        SELECT pool, required_labels, priority
        FROM jobs
        WHERE execution_id = $1 AND kind = 'execute'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or_else(|| ("default".into(), json!({}), 0));
    let retry_count = execution.retry_count.saturating_add(1);
    let phase = plan.stage.as_str();
    let retry_timestamp_key = format!("retry_{retry_count}");
    let updated = sqlx::query_as::<_, ExecutionRow>(
        r#"
        UPDATE batch_execution_record
        SET status = 'retrying',
            retry_count = $2,
            execution_phase = $3,
            control_phase = CASE WHEN $3 = 'submit' THEN 'graph_patched' ELSE 'admitted' END,
            submission_state = 'not_started',
            scheduler_name = NULL,
            scheduler_job_id = NULL,
            scheduler_state = 'not_submitted',
            scheduler_raw_state = NULL,
            scheduler_reason = NULL,
            daliuge_session_id = NULL,
            daliuge_manager_url = NULL,
            daliuge_state = 'not_created',
            daliuge_raw_status = NULL,
            remote_session_dir = NULL,
            output_state = 'not_started',
            terminal_outcome = NULL,
            failure_class = NULL,
            last_error = NULL,
            last_reconciled_at = NULL,
            completed_at = NULL,
            phase_timestamps = phase_timestamps || jsonb_build_object($4::TEXT, to_jsonb(now())),
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(retry_count)
    .bind(phase)
    .bind(&retry_timestamp_key)
    .fetch_one(&mut *tx)
    .await?;
    let payload = json!({
        "execution_id": id,
        "do_stage": plan.do_stage,
        "do_submit": plan.do_submit,
        "retry_count": retry_count,
        "recovery_reason": reason,
        "correlation_id": correlation_id,
    });
    let job = sqlx::query_as::<_, JobRow>(
        r#"
        INSERT INTO jobs (
            uuid, kind, payload, execution_id, idempotency_key, next_run_at,
            pool, required_capability, required_labels, priority
        )
        VALUES ($1, 'execute', $2, $3, $4, now(), $5, 'daliuge-deployment', $6, $7)
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(payload)
    .bind(id)
    .bind(format!("execute:{id}:retry:{retry_count}"))
    .bind(worker_pool)
    .bind(required_labels)
    .bind(priority)
    .fetch_one(&mut *tx)
    .await?;
    let default_correlation = id.to_string();
    insert_provenance_event(
        &mut *tx,
        "execution.retry_requested",
        &execution.project_module,
        source_identifiers.first().map(String::as_str),
        Some(id),
        Some(actor),
        correlation_id.or(Some(default_correlation.as_str())),
        &json!({
            "retry_count": retry_count,
            "stage": plan.stage,
            "reason": reason,
            "job_id": job.uuid,
            "system_action": "resume only the last safe Beampipe stage",
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(ExecutionRetryResult {
        execution: updated,
        job,
        plan,
    })
}

pub async fn apply_execution_state_patch(
    pool: &PgPool,
    id: Uuid,
    patch: ExecutionStatePatch,
) -> Result<Option<ExecutionRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let Some(locked) = sqlx::query_as::<_, ExecutionRow>(
        "SELECT * FROM batch_execution_record WHERE uuid = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.rollback().await?;
        return Ok(None);
    };

    let previous_phase = locked.control_phase.clone();
    let control_phase = patch.control_phase.map(|value| value.as_str());
    let submission_state = patch.submission_state.map(|value| value.as_str());
    let scheduler_state = patch.scheduler_state.map(|value| value.as_str());
    let daliuge_state = patch.daliuge_state.map(|value| value.as_str());
    let output_state = patch.output_state.map(|value| value.as_str());
    let terminal_outcome = patch.terminal_outcome.map(|value| value.as_str());
    let failure_class = patch.failure_class.map(|value| value.as_str());

    let projected = sqlx::query_as::<_, ExecutionRow>(
        r#"
        UPDATE batch_execution_record
        SET control_phase = COALESCE($2, control_phase),
            submission_state = COALESCE($3, submission_state),
            scheduler_name = COALESCE($4, scheduler_name),
            scheduler_job_id = COALESCE($5, scheduler_job_id),
            scheduler_state = COALESCE($6, scheduler_state),
            scheduler_raw_state = COALESCE($7, scheduler_raw_state),
            scheduler_reason = COALESCE($8, scheduler_reason),
            daliuge_session_id = COALESCE($9, daliuge_session_id),
            daliuge_manager_url = COALESCE($10, daliuge_manager_url),
            daliuge_state = COALESCE($11, daliuge_state),
            daliuge_raw_status = COALESCE($12, daliuge_raw_status),
            remote_session_dir = COALESCE($13, remote_session_dir),
            output_state = COALESCE($14, output_state),
            terminal_outcome = COALESCE($15, terminal_outcome),
            failure_class = COALESCE($16, failure_class),
            last_error = COALESCE($17, last_error),
            last_reconciled_at = COALESCE($18, last_reconciled_at),
            phase_timestamps = CASE
                WHEN $2::text IS NOT NULL AND $2::text IS DISTINCT FROM control_phase
                THEN phase_timestamps || jsonb_build_object($2::text, to_jsonb(now()))
                ELSE phase_timestamps
            END,
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(control_phase)
    .bind(submission_state)
    .bind(patch.scheduler_name)
    .bind(patch.scheduler_job_id)
    .bind(scheduler_state)
    .bind(patch.scheduler_raw_state)
    .bind(patch.scheduler_reason)
    .bind(patch.daliuge_session_id)
    .bind(patch.daliuge_manager_url)
    .bind(daliuge_state)
    .bind(patch.daliuge_raw_status)
    .bind(patch.remote_session_dir)
    .bind(output_state)
    .bind(terminal_outcome)
    .bind(failure_class)
    .bind(patch.last_error)
    .bind(patch.last_reconciled_at)
    .fetch_one(&mut *tx)
    .await?;

    let decision = projected.axes().reconcile();
    let current_status = locked.status_enum().unwrap_or(ExecutionStatus::Pending);
    let aggregate_status = if current_status.is_locked_terminal() {
        current_status
    } else {
        decision.status
    };
    let decision_outcome = decision.terminal_outcome.map(|value| value.as_str());
    let terminal = decision.terminal_outcome.is_some();
    let row = sqlx::query_as::<_, ExecutionRow>(
        r#"
        UPDATE batch_execution_record
        SET status = $2,
            terminal_outcome = COALESCE($3, terminal_outcome),
            control_phase = CASE WHEN $4 THEN 'terminal' ELSE control_phase END,
            phase_timestamps = CASE
                WHEN $4 AND NOT (phase_timestamps ? 'terminal')
                THEN phase_timestamps || jsonb_build_object('terminal', to_jsonb(now()))
                ELSE phase_timestamps
            END,
            completed_at = CASE
                WHEN $4 THEN COALESCE(completed_at, now())
                ELSE completed_at
            END,
            last_reconciled_at = now(),
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(status_str(aggregate_status))
    .bind(decision_outcome)
    .bind(terminal)
    .fetch_one(&mut *tx)
    .await?;

    if let Some(mismatch) = decision.mismatch {
        insert_provenance_event(
            &mut *tx,
            "execution.reconciliation_mismatch",
            &row.project_module,
            None,
            Some(id),
            Some("system:reconciler"),
            Some(&id.to_string()),
            &serde_json::json!({
                "code": mismatch.code,
                "message": mismatch.message,
                "requires_operator": mismatch.requires_operator,
                "previous_control_phase": previous_phase,
                "scheduler_state": row.scheduler_state,
                "daliuge_state": row.daliuge_state,
            }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Some(row))
}

pub async fn apply_execution_provenance_patch(
    pool: &PgPool,
    id: Uuid,
    patch: ExecutionProvenancePatch,
) -> Result<Option<ExecutionRow>, sqlx::Error> {
    sqlx::query_as::<_, ExecutionRow>(
        r#"
        UPDATE batch_execution_record
        SET deployment_profile_revision = COALESCE($2, deployment_profile_revision),
            deployment_profile_snapshot = COALESCE($3, deployment_profile_snapshot),
            discovery_signature = COALESCE($4, discovery_signature),
            manifest_sha256 = COALESCE($5, manifest_sha256),
            source_graph_sha256 = COALESCE($6, source_graph_sha256),
            patched_graph_sha256 = COALESCE($7, patched_graph_sha256),
            physical_graph_sha256 = COALESCE($8, physical_graph_sha256),
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(patch.deployment_profile_revision)
    .bind(patch.deployment_profile_snapshot)
    .bind(patch.discovery_signature)
    .bind(patch.manifest_sha256)
    .bind(patch.source_graph_sha256)
    .bind(patch.patched_graph_sha256)
    .bind(patch.physical_graph_sha256)
    .fetch_optional(pool)
    .await
}

pub async fn record_execution_observation(
    pool: &PgPool,
    execution_id: Uuid,
    observation: ExecutionObservationInput,
) -> Result<ExecutionObservationRow, sqlx::Error> {
    if !matches!(
        observation.kind.as_str(),
        "scheduler" | "daliuge_session" | "daliuge_graph" | "output"
    ) {
        return Err(sqlx::Error::Protocol(format!(
            "unsupported execution observation kind: {}",
            observation.kind
        )));
    }
    sqlx::query_as::<_, ExecutionObservationRow>(
        r#"
        INSERT INTO execution_observations (
            uuid, execution_id, kind, normalized_state, raw_state, reason,
            payload, source_version, observed_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, now()))
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(execution_id)
    .bind(observation.kind)
    .bind(observation.normalized_state)
    .bind(observation.raw_state)
    .bind(observation.reason)
    .bind(observation.payload)
    .bind(observation.source_version)
    .bind(observation.observed_at)
    .fetch_one(pool)
    .await
}

pub async fn list_execution_observations(
    pool: &PgPool,
    execution_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<ExecutionObservationRow>, sqlx::Error> {
    sqlx::query_as::<_, ExecutionObservationRow>(
        r#"
        SELECT *
        FROM execution_observations
        WHERE execution_id = $1
        ORDER BY observed_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(execution_id)
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
}

pub async fn store_execution_artifact(
    pool: &PgPool,
    execution_id: Uuid,
    artifact: ExecutionArtifactInput,
) -> Result<ExecutionArtifactRow, sqlx::Error> {
    validate_artifact(&artifact)?;
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query_as::<_, ExecutionArtifactRow>(
        r#"
        INSERT INTO execution_artifacts (
            uuid, execution_id, kind, storage_kind, uri, inline_json, media_type,
            sha256, size_bytes, producer_phase, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (execution_id, kind, sha256) DO NOTHING
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(execution_id)
    .bind(&artifact.kind)
    .bind(&artifact.storage_kind)
    .bind(&artifact.uri)
    .bind(&artifact.inline_json)
    .bind(&artifact.media_type)
    .bind(&artifact.sha256)
    .bind(artifact.size_bytes)
    .bind(&artifact.producer_phase)
    .bind(&artifact.metadata)
    .fetch_optional(&mut *tx)
    .await?;
    let row = match inserted {
        Some(row) => row,
        None => {
            sqlx::query_as::<_, ExecutionArtifactRow>(
                r#"
                SELECT * FROM execution_artifacts
                WHERE execution_id = $1 AND kind = $2 AND sha256 = $3
                "#,
            )
            .bind(execution_id)
            .bind(&artifact.kind)
            .bind(&artifact.sha256)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    Ok(row)
}

fn validate_artifact(artifact: &ExecutionArtifactInput) -> Result<(), sqlx::Error> {
    if !matches!(
        artifact.storage_kind.as_str(),
        "database" | "file" | "remote" | "http"
    ) {
        return Err(sqlx::Error::Protocol(
            "artifact storage_kind must be database, file, remote, or http".into(),
        ));
    }
    if artifact.uri.is_none() && artifact.inline_json.is_none() {
        return Err(sqlx::Error::Protocol(
            "artifact requires uri or inline_json".into(),
        ));
    }
    if artifact.sha256.len() != 64
        || !artifact
            .sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(sqlx::Error::Protocol(
            "artifact sha256 must be 64 hexadecimal characters".into(),
        ));
    }
    Ok(())
}

pub async fn list_execution_artifacts(
    pool: &PgPool,
    execution_id: Uuid,
) -> Result<Vec<ExecutionArtifactRow>, sqlx::Error> {
    sqlx::query_as::<_, ExecutionArtifactRow>(
        r#"
        SELECT *
        FROM execution_artifacts
        WHERE execution_id = $1
        ORDER BY created_at ASC, kind ASC
        "#,
    )
    .bind(execution_id)
    .fetch_all(pool)
    .await
}

pub async fn list_archive_metadata_for_sources(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: &[String],
) -> Result<Vec<ArchiveMetadataRow>, sqlx::Error> {
    sqlx::query_as::<_, ArchiveMetadataRow>(
        r#"
        SELECT *
        FROM archive_metadata
        WHERE project_module = $1
          AND source_identifier = ANY($2)
        ORDER BY source_identifier ASC, sbid ASC
        "#,
    )
    .bind(project_module)
    .bind(source_identifiers)
    .fetch_all(pool)
    .await
}

pub async fn apply_execution_patch(
    pool: &PgPool,
    id: Uuid,
    patch: LedgerPatch,
) -> Result<Option<ExecutionRow>, sqlx::Error> {
    apply_execution_patch_with_correlation(pool, id, patch, None).await
}

pub async fn apply_execution_patch_with_correlation(
    pool: &PgPool,
    id: Uuid,
    patch: LedgerPatch,
    correlation_id: Option<&str>,
) -> Result<Option<ExecutionRow>, sqlx::Error> {
    let Some(row) = get_execution(pool, id).await? else {
        return Ok(None);
    };
    let prev_status = row.status_enum().unwrap_or(ExecutionStatus::Pending);
    let prev_phase = row.phase_enum();
    let project_module = row.project_module.clone();
    let patch_status = patch.status;
    let patch_phase = patch.execution_phase;
    let mut state = LedgerState {
        status: prev_status,
        execution_phase: prev_phase,
        retry_count: row.retry_count,
        scheduler_name: row.scheduler_name,
        scheduler_job_id: row.scheduler_job_id,
        workflow_manifest: row.workflow_manifest,
        last_error: row.last_error,
        started_at: row.started_at,
        completed_at: row.completed_at,
        updated_at: row.updated_at,
    };
    state
        .apply_patch(patch, Utc::now())
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

    let correlation = correlation_id
        .map(str::to_string)
        .or_else(|| Some(id.to_string()));

    if let Some(next) = patch_status {
        if next != prev_status {
            let event_type = match next {
                ExecutionStatus::Running => {
                    Some(beampipe_domain::provenance::ProvenanceEventType::ExecutionRunning)
                }
                ExecutionStatus::AwaitingScheduler => Some(
                    beampipe_domain::provenance::ProvenanceEventType::ExecutionAwaitingScheduler,
                ),
                _ => None,
            };
            if let Some(ev) = event_type {
                let payload = serde_json::json!({
                    "from_status": prev_status.as_str(),
                    "to_status": next.as_str(),
                });
                crate::provenance::record_provenance_event(
                    pool,
                    ev.as_str(),
                    &project_module,
                    None,
                    Some(id),
                    Some("system:execution"),
                    correlation.as_deref(),
                    &payload,
                )
                .await;
            }
        }
    }
    if patch_phase.is_some() {
        let new_phase = state.execution_phase;
        if new_phase != prev_phase && matches!(new_phase, Some(ExecutionPhase::Submit)) {
            let payload = serde_json::json!({
                "execution_phase": "submit",
            });
            crate::provenance::record_provenance_event(
                pool,
                beampipe_domain::provenance::ProvenanceEventType::ExecutionExecuteStarted.as_str(),
                &project_module,
                None,
                Some(id),
                Some("system:execution"),
                correlation.as_deref(),
                &payload,
            )
            .await;
        }
    }

    sqlx::query_as::<_, ExecutionRow>(
        r#"
        UPDATE batch_execution_record
        SET status = $2,
            execution_phase = $3,
            retry_count = $4,
            scheduler_name = $5,
            scheduler_job_id = $6,
            workflow_manifest = $7,
            last_error = $8,
            started_at = $9,
            completed_at = $10,
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(status_str(state.status))
    .bind(state.execution_phase.map(phase_str))
    .bind(state.retry_count)
    .bind(state.scheduler_name)
    .bind(state.scheduler_job_id)
    .bind(state.workflow_manifest)
    .bind(state.last_error)
    .bind(state.started_at)
    .bind(state.completed_at)
    .fetch_optional(pool)
    .await
}

pub async fn get_enabled_project_modules(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT DISTINCT project_module
        FROM source_registry
        WHERE enabled = true
        ORDER BY project_module ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn count_discovery_in_flight_batches(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT discovery_claim_token)
        FROM source_registry
        WHERE discovery_claim_token IS NOT NULL
          AND discovery_claim_expires_at > now()
        "#,
    )
    .fetch_one(pool)
    .await
}

pub async fn count_discovery_in_flight_for_module(
    pool: &PgPool,
    project_module: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT discovery_claim_token)
        FROM source_registry
        WHERE project_module = $1
          AND discovery_claim_token IS NOT NULL
          AND discovery_claim_expires_at > now()
        "#,
    )
    .bind(project_module)
    .fetch_one(pool)
    .await
}

pub async fn insert_project_config_wasm(
    pool: &PgPool,
    project_config_id: Uuid,
    wasm_sha256: &str,
    wasm_bytes: &[u8],
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO project_config_wasm (uuid, project_config_id, wasm_sha256, wasm_bytes)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (project_config_id, wasm_sha256) DO UPDATE
        SET wasm_bytes = EXCLUDED.wasm_bytes, uploaded_at = now()
        RETURNING uuid
        "#,
    )
    .bind(id)
    .bind(project_config_id)
    .bind(wasm_sha256)
    .bind(wasm_bytes)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn get_project_config_wasm(
    pool: &PgPool,
    project_config_id: Uuid,
    wasm_sha256: &str,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT wasm_bytes
        FROM project_config_wasm
        WHERE project_config_id = $1 AND wasm_sha256 = $2
        "#,
    )
    .bind(project_config_id)
    .bind(wasm_sha256)
    .fetch_optional(pool)
    .await
}

pub async fn get_project_config_wasm_meta(
    pool: &PgPool,
    project_config_id: Uuid,
    wasm_sha256: &str,
) -> Result<Option<(Uuid, chrono::DateTime<chrono::Utc>)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT uuid, uploaded_at
        FROM project_config_wasm
        WHERE project_config_id = $1 AND wasm_sha256 = $2
        "#,
    )
    .bind(project_config_id)
    .bind(wasm_sha256)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, Default)]
pub struct JobEnqueueOptions {
    pub execution_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub max_attempts: Option<i32>,
    pub pool: Option<String>,
    pub required_capability: Option<String>,
    pub required_labels: BTreeMap<String, String>,
    pub priority: Option<i32>,
}

pub async fn enqueue_job(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    execution_id: Option<Uuid>,
    idempotency_key: Option<&str>,
) -> Result<JobRow, sqlx::Error> {
    enqueue_job_with_options(
        pool,
        kind,
        payload,
        JobEnqueueOptions {
            execution_id,
            idempotency_key: idempotency_key.map(str::to_string),
            ..Default::default()
        },
    )
    .await
}

pub async fn enqueue_job_with_options(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    opts: JobEnqueueOptions,
) -> Result<JobRow, sqlx::Error> {
    let next_run_at = opts.next_run_at.unwrap_or_else(Utc::now);
    let required_capability = opts
        .required_capability
        .as_deref()
        .or_else(|| job_kind_capability(kind));
    let required_labels = serde_json::to_value(&opts.required_labels)
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    if let Some(max_attempts) = opts.max_attempts {
        sqlx::query_as::<_, JobRow>(
            r#"
            INSERT INTO jobs (
                uuid, kind, payload, execution_id, idempotency_key, next_run_at,
                max_attempts, pool, required_capability, required_labels, priority
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
            DO UPDATE SET updated_at = now()
            RETURNING *
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(kind)
        .bind(payload)
        .bind(opts.execution_id)
        .bind(opts.idempotency_key.as_deref())
        .bind(next_run_at)
        .bind(max_attempts)
        .bind(opts.pool.as_deref().unwrap_or("default"))
        .bind(required_capability)
        .bind(&required_labels)
        .bind(opts.priority.unwrap_or(0))
        .fetch_one(pool)
        .await
    } else {
        sqlx::query_as::<_, JobRow>(
            r#"
            INSERT INTO jobs (
                uuid, kind, payload, execution_id, idempotency_key, next_run_at,
                pool, required_capability, required_labels, priority
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
            DO UPDATE SET updated_at = now()
            RETURNING *
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(kind)
        .bind(payload)
        .bind(opts.execution_id)
        .bind(opts.idempotency_key.as_deref())
        .bind(next_run_at)
        .bind(opts.pool.as_deref().unwrap_or("default"))
        .bind(required_capability)
        .bind(&required_labels)
        .bind(opts.priority.unwrap_or(0))
        .fetch_one(pool)
        .await
    }
}

fn job_kind_capability(kind: &str) -> Option<&'static str> {
    match kind {
        "scheduler_tick" | "discover_batch" => Some("casda-discovery"),
        "execution_scheduler_tick" => Some("manifest-generation"),
        "execute" | "dim_poll" | "dim_poll_tick" => Some("daliuge-deployment"),
        "slurm_poll_tick" => Some("slurm-remote"),
        _ => None,
    }
}

pub async fn enqueue_job_deferred(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    delay_secs: i64,
    execution_id: Option<Uuid>,
    idempotency_key: Option<&str>,
) -> Result<JobRow, sqlx::Error> {
    enqueue_job_deferred_with_options(
        pool,
        kind,
        payload,
        delay_secs,
        JobEnqueueOptions {
            execution_id,
            idempotency_key: idempotency_key.map(str::to_string),
            ..Default::default()
        },
    )
    .await
}

pub async fn enqueue_job_deferred_with_options(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    delay_secs: i64,
    mut options: JobEnqueueOptions,
) -> Result<JobRow, sqlx::Error> {
    let next_run_at = Utc::now() + chrono::Duration::seconds(delay_secs);
    options.next_run_at = Some(next_run_at);
    enqueue_job_with_options(pool, kind, payload, options).await
}

/// Most recent non-terminal job for an execution (queued or running).
pub async fn get_active_job_for_execution(
    pool: &PgPool,
    execution_id: Uuid,
) -> Result<Option<JobRow>, sqlx::Error> {
    sqlx::query_as::<_, JobRow>(
        r#"
        SELECT *
        FROM jobs
        WHERE execution_id = $1
          AND status IN ('queued', 'running')
        ORDER BY
          CASE status WHEN 'running' THEN 0 ELSE 1 END,
          CASE kind
            WHEN 'execute' THEN 0
            WHEN 'dim_poll' THEN 1
            ELSE 2
          END,
          created_at DESC
        LIMIT 1
        "#,
    )
    .bind(execution_id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone)]
pub struct ExecutionTraceSummary {
    pub correlation_id: Option<String>,
    pub active_job_id: Option<Uuid>,
    pub active_job_kind: Option<String>,
    pub events: Vec<crate::models::ProvenanceEventRow>,
}

pub async fn execution_trace_summary(
    pool: &PgPool,
    execution_id: Uuid,
    event_limit: i64,
) -> Result<ExecutionTraceSummary, sqlx::Error> {
    let events = list_provenance_events_for_execution(pool, execution_id, event_limit).await?;
    let correlation_id = events.iter().rev().find_map(|e| e.correlation_id.clone());
    let active_job = get_active_job_for_execution(pool, execution_id).await?;
    Ok(ExecutionTraceSummary {
        correlation_id,
        active_job_id: active_job.as_ref().map(|j| j.uuid),
        active_job_kind: active_job.map(|j| j.kind),
        events,
    })
}

/// Mark sources for rediscovery and enqueue a scheduler tick in one transaction.
pub async fn mark_sources_and_enqueue_discovery_tick(
    pool: &PgPool,
    project_module: &str,
    source_identifiers: Option<&[String]>,
    payload: Value,
    idempotency_key: &str,
) -> Result<(Vec<String>, Option<JobRow>), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        r#"
        UPDATE source_registry
        SET last_checked_at = NULL,
            last_attempted_at = NULL,
            discovery_claim_token = NULL,
            discovery_claim_expires_at = NULL,
            workflow_claim_token = NULL,
            workflow_claimed_at = NULL,
            workflow_claim_expires_at = NULL
        WHERE project_module =
        "#,
    );
    qb.push_bind(project_module);
    qb.push(" AND enabled = true");
    if let Some(ids) = source_identifiers.filter(|ids| !ids.is_empty()) {
        qb.push(" AND source_identifier = ANY(")
            .push_bind(ids)
            .push(")");
    }
    qb.push(" RETURNING source_identifier");
    let marked = qb
        .build_query_scalar::<String>()
        .fetch_all(&mut *tx)
        .await?;
    if marked.is_empty() {
        tx.commit().await?;
        return Ok((marked, None));
    }
    let job = sqlx::query_as::<_, JobRow>(
        r#"
        INSERT INTO jobs (uuid, kind, payload, idempotency_key, next_run_at)
        VALUES ($1, $2, $3, $4, now())
        ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
        DO UPDATE SET updated_at = now()
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind("scheduler_tick")
    .bind(payload)
    .bind(idempotency_key)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((marked, Some(job)))
}

/// Enqueue or re-queue a recurring scheduler job (discovery/execution ticks).
pub async fn enqueue_recurring_job(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    idempotency_key: &str,
) -> Result<JobRow, sqlx::Error> {
    enqueue_recurring_job_with_options(
        pool,
        kind,
        payload,
        idempotency_key,
        JobEnqueueOptions::default(),
    )
    .await
}

pub async fn enqueue_recurring_job_with_options(
    pool: &PgPool,
    kind: &str,
    payload: Value,
    idempotency_key: &str,
    opts: JobEnqueueOptions,
) -> Result<JobRow, sqlx::Error> {
    let required_capability = opts
        .required_capability
        .as_deref()
        .or_else(|| job_kind_capability(kind));
    let required_labels = serde_json::to_value(&opts.required_labels)
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    sqlx::query_as::<_, JobRow>(
        r#"
        INSERT INTO jobs (
            uuid, kind, payload, idempotency_key, next_run_at, pool,
            required_capability, required_labels, priority
        )
        VALUES ($1, $2, $3, $4, now(), $5, $6, $7, $8)
        ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
        DO UPDATE SET
            status = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN 'queued'
                ELSE jobs.status
            END,
            payload = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN EXCLUDED.payload
                ELSE jobs.payload
            END,
            pool = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN EXCLUDED.pool
                ELSE jobs.pool
            END,
            required_capability = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter')
                    THEN EXCLUDED.required_capability
                ELSE jobs.required_capability
            END,
            required_labels = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter')
                    THEN EXCLUDED.required_labels
                ELSE jobs.required_labels
            END,
            priority = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN EXCLUDED.priority
                ELSE jobs.priority
            END,
            next_run_at = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN now()
                ELSE jobs.next_run_at
            END,
            attempts = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN 0
                ELSE jobs.attempts
            END,
            locked_until = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.locked_until
            END,
            lease_owner = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.lease_owner
            END,
            lease_token = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.lease_token
            END,
            lease_expires_at = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.lease_expires_at
            END,
            heartbeat_at = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.heartbeat_at
            END,
            last_error = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.last_error
            END,
            failure_class = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.failure_class
            END,
            dead_lettered_at = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.dead_lettered_at
            END,
            dead_letter_reason = CASE
                WHEN jobs.status IN ('completed', 'failed', 'dead_letter') THEN NULL
                ELSE jobs.dead_letter_reason
            END,
            updated_at = now()
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(kind)
    .bind(payload)
    .bind(idempotency_key)
    .bind(opts.pool.as_deref().unwrap_or("default"))
    .bind(required_capability)
    .bind(required_labels)
    .bind(opts.priority.unwrap_or(0))
    .fetch_one(pool)
    .await
}

pub async fn fail_job_permanently(pool: &PgPool, id: Uuid, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE jobs
        SET status = 'failed',
            locked_until = NULL,
            lease_owner = NULL,
            lease_token = NULL,
            lease_expires_at = NULL,
            heartbeat_at = NULL,
            last_error = $2,
            updated_at = now()
        WHERE uuid = $1
        "#,
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn blacklist_token(
    pool: &PgPool,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO token_blacklist (token_hash, expires_at)
        VALUES ($1, $2)
        ON CONFLICT (token_hash) DO NOTHING
        "#,
    )
    .bind(token_hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn is_token_blacklisted(pool: &PgPool, token_hash: &str) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM token_blacklist
        WHERE token_hash = $1 AND expires_at > now()
        "#,
    )
    .bind(token_hash)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

pub async fn cleanup_expired_blacklisted_tokens(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM token_blacklist WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedExecutions {
    pub items: Vec<ExecutionRow>,
    pub total: i64,
    pub page: i64,
    pub items_per_page: i64,
}

pub async fn list_executions(
    pool: &PgPool,
    project_module: Option<&str>,
    status: Option<&str>,
    page: i64,
    items_per_page: i64,
) -> Result<PaginatedExecutions, sqlx::Error> {
    let page = page.max(1);
    let items_per_page = items_per_page.clamp(1, 500);
    let offset = (page - 1) * items_per_page;

    let mut count_qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT COUNT(*) FROM batch_execution_record WHERE 1=1");
    if let Some(module) = project_module {
        count_qb.push(" AND project_module = ").push_bind(module);
    }
    if let Some(st) = status {
        count_qb.push(" AND status = ").push_bind(st);
    }
    let total: i64 = count_qb.build_query_scalar().fetch_one(pool).await?;

    let mut list_qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT * FROM batch_execution_record WHERE 1=1");
    if let Some(module) = project_module {
        list_qb.push(" AND project_module = ").push_bind(module);
    }
    if let Some(st) = status {
        list_qb.push(" AND status = ").push_bind(st);
    }
    list_qb
        .push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(items_per_page)
        .push(" OFFSET ")
        .push_bind(offset);
    let items = list_qb.build_query_as().fetch_all(pool).await?;

    Ok(PaginatedExecutions {
        items,
        total,
        page,
        items_per_page,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn create_deployment_profile(
    pool: &PgPool,
    name: &str,
    description: Option<&str>,
    project_module: Option<&str>,
    is_default: bool,
    max_concurrent_executions: Option<i32>,
    translation: Value,
    deployment: Value,
) -> Result<DeploymentProfileRow, sqlx::Error> {
    let spec_sha256 = deployment_profile_spec_sha256(
        name,
        description,
        project_module,
        is_default,
        max_concurrent_executions,
        &translation,
        &deployment,
    );
    sqlx::query_as::<_, DeploymentProfileRow>(
        r#"
        INSERT INTO daliuge_deployment_profile
            (uuid, name, description, project_module, is_default,
             max_concurrent_executions, translation, deployment, revision, spec_sha256)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9)
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(description)
    .bind(project_module)
    .bind(is_default)
    .bind(max_concurrent_executions)
    .bind(translation)
    .bind(deployment)
    .bind(spec_sha256)
    .fetch_one(pool)
    .await
}

pub async fn list_deployment_profiles(
    pool: &PgPool,
    project_module: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<DeploymentProfileRow>, sqlx::Error> {
    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new("SELECT * FROM daliuge_deployment_profile WHERE 1=1");
    if let Some(module) = project_module {
        qb.push(" AND project_module = ").push_bind(module);
    }
    qb.push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    qb.build_query_as().fetch_all(pool).await
}

#[allow(clippy::too_many_arguments)]
pub async fn update_deployment_profile(
    pool: &PgPool,
    id: Uuid,
    name: &str,
    description: Option<&str>,
    project_module: Option<&str>,
    is_default: bool,
    max_concurrent_executions: Option<i32>,
    translation: Value,
    deployment: Value,
) -> Result<Option<DeploymentProfileRow>, sqlx::Error> {
    let spec_sha256 = deployment_profile_spec_sha256(
        name,
        description,
        project_module,
        is_default,
        max_concurrent_executions,
        &translation,
        &deployment,
    );
    sqlx::query_as::<_, DeploymentProfileRow>(
        r#"
        UPDATE daliuge_deployment_profile
        SET name = $2,
            description = $3,
            project_module = $4,
            is_default = $5,
            max_concurrent_executions = $6,
            translation = $7,
            deployment = $8,
            revision = revision + 1,
            spec_sha256 = $9,
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(description)
    .bind(project_module)
    .bind(is_default)
    .bind(max_concurrent_executions)
    .bind(translation)
    .bind(deployment)
    .bind(spec_sha256)
    .fetch_optional(pool)
    .await
}

pub fn deployment_profile_spec_sha256(
    name: &str,
    description: Option<&str>,
    project_module: Option<&str>,
    is_default: bool,
    max_concurrent_executions: Option<i32>,
    translation: &Value,
    deployment: &Value,
) -> String {
    let canonical = serde_json::json!({
        "name": name,
        "description": description,
        "project_module": project_module,
        "is_default": is_default,
        "max_concurrent_executions": max_concurrent_executions,
        "translation": translation,
        "deployment": deployment,
    });
    let bytes = serde_json::to_vec(&canonical)
        .expect("serializing a deployment profile JSON value cannot fail");
    format!("{:x}", Sha256::digest(bytes))
}

pub async fn delete_deployment_profile(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM daliuge_deployment_profile WHERE uuid = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn insert_project_config(
    pool: &PgPool,
    project_id: &str,
    spec: Value,
    spec_sha256: &str,
) -> Result<ProjectConfigRow, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let version: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM project_configs WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE project_configs SET active = false WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query_as::<_, ProjectConfigRow>(
        r#"
        INSERT INTO project_configs (uuid, project_id, version, spec, spec_sha256, active)
        VALUES ($1, $2, $3, $4, $5, true)
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(version)
    .bind(spec)
    .bind(spec_sha256)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    crate::provenance::record_provenance_event(
        pool,
        "config.activated",
        project_id,
        None,
        None,
        Some("system:api"),
        None,
        &serde_json::json!({"spec_sha256": spec_sha256, "version": row.version}),
    )
    .await;
    Ok(row)
}

pub async fn get_active_project_config(
    pool: &PgPool,
    project_id: &str,
) -> Result<Option<ProjectConfigRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectConfigRow>(
        "SELECT * FROM project_configs WHERE project_id = $1 AND active = true",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
}

pub async fn get_project_config_by_uuid(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<ProjectConfigRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectConfigRow>("SELECT * FROM project_configs WHERE uuid = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Resolve project config for an execution: pinned version when set, else active config.
pub async fn get_project_config_for_execution(
    pool: &PgPool,
    execution: &ExecutionRow,
) -> Result<Option<ProjectConfigRow>, sqlx::Error> {
    if let Some(id) = execution.project_config_id {
        if let Some(row) = get_project_config_by_uuid(pool, id).await? {
            return Ok(Some(row));
        }
    }
    get_active_project_config(pool, &execution.project_module).await
}

pub async fn list_project_config_versions(
    pool: &PgPool,
    project_id: &str,
) -> Result<Vec<ProjectConfigRow>, sqlx::Error> {
    sqlx::query_as::<_, ProjectConfigRow>(
        "SELECT * FROM project_configs WHERE project_id = $1 ORDER BY version DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
}

pub async fn register_worker_instance(
    pool: &PgPool,
    registration: &WorkerRegistration,
) -> Result<WorkerInstanceRow, sqlx::Error> {
    sqlx::query_as::<_, WorkerInstanceRow>(
        r#"
        INSERT INTO worker_instances (
            uuid, instance_name, host_name, process_id, role, pool, capabilities,
            labels, version, concurrency_limit
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (uuid) DO UPDATE
        SET instance_name = EXCLUDED.instance_name,
            host_name = EXCLUDED.host_name,
            process_id = EXCLUDED.process_id,
            role = EXCLUDED.role,
            pool = EXCLUDED.pool,
            capabilities = EXCLUDED.capabilities,
            labels = EXCLUDED.labels,
            version = EXCLUDED.version,
            concurrency_limit = EXCLUDED.concurrency_limit,
            status = CASE
                WHEN worker_instances.status = 'draining' THEN 'draining'
                ELSE 'active'
            END,
            stopped_at = NULL,
            last_heartbeat_at = now(),
            updated_at = now()
        RETURNING *
        "#,
    )
    .bind(registration.uuid)
    .bind(&registration.instance_name)
    .bind(&registration.host_name)
    .bind(registration.process_id)
    .bind(&registration.role)
    .bind(&registration.pool)
    .bind(&registration.capabilities)
    .bind(&registration.labels)
    .bind(&registration.version)
    .bind(registration.concurrency_limit.max(1))
    .fetch_one(pool)
    .await
}

pub async fn heartbeat_worker(pool: &PgPool, worker_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE worker_instances
        SET last_heartbeat_at = now(), updated_at = now()
        WHERE uuid = $1 AND status IN ('active', 'draining')
        "#,
    )
    .bind(worker_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_worker_draining(
    pool: &PgPool,
    worker_id: Uuid,
    draining: bool,
) -> Result<Option<WorkerInstanceRow>, sqlx::Error> {
    sqlx::query_as::<_, WorkerInstanceRow>(
        r#"
        UPDATE worker_instances
        SET status = CASE WHEN $2 THEN 'draining' ELSE 'active' END,
            draining_at = CASE WHEN $2 THEN COALESCE(draining_at, now()) ELSE NULL END,
            last_heartbeat_at = now(),
            updated_at = now()
        WHERE uuid = $1 AND status <> 'stopped'
        RETURNING *
        "#,
    )
    .bind(worker_id)
    .bind(draining)
    .fetch_optional(pool)
    .await
}

pub async fn mark_worker_stopped(pool: &PgPool, worker_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE worker_instances
        SET status = 'stopped', stopped_at = now(), updated_at = now()
        WHERE uuid = $1
        "#,
    )
    .bind(worker_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn list_worker_instances(
    pool: &PgPool,
    include_stopped: bool,
) -> Result<Vec<WorkerInstanceRow>, sqlx::Error> {
    sqlx::query_as::<_, WorkerInstanceRow>(
        r#"
        SELECT *
        FROM worker_instances
        WHERE $1 OR status <> 'stopped'
        ORDER BY status, instance_name, started_at
        "#,
    )
    .bind(include_stopped)
    .fetch_all(pool)
    .await
}

pub async fn get_worker_instance(
    pool: &PgPool,
    worker_id: Uuid,
) -> Result<Option<WorkerInstanceRow>, sqlx::Error> {
    sqlx::query_as::<_, WorkerInstanceRow>("SELECT * FROM worker_instances WHERE uuid = $1")
        .bind(worker_id)
        .fetch_optional(pool)
        .await
}

pub async fn active_worker_lease_count(pool: &PgPool, worker_id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM jobs
        WHERE status = 'running'
          AND lease_owner = $1
          AND lease_expires_at > now()
        "#,
    )
    .bind(worker_id)
    .fetch_one(pool)
    .await
}

pub async fn list_worker_leases(
    pool: &PgPool,
    worker_id: Option<Uuid>,
    include_expired: bool,
) -> Result<Vec<JobRow>, sqlx::Error> {
    sqlx::query_as::<_, JobRow>(
        r#"
        SELECT *
        FROM jobs
        WHERE status = 'running'
          AND ($1::uuid IS NULL OR lease_owner = $1)
          AND ($2 OR lease_expires_at > now())
        ORDER BY lease_expires_at ASC NULLS FIRST, priority DESC, created_at
        "#,
    )
    .bind(worker_id)
    .bind(include_expired)
    .fetch_all(pool)
    .await
}

pub async fn list_worker_pools(pool: &PgPool) -> Result<Vec<WorkerPoolSummary>, sqlx::Error> {
    sqlx::query_as::<_, WorkerPoolSummary>(
        r#"
        SELECT w.pool,
               COUNT(*) FILTER (WHERE w.status = 'active')::bigint AS active_workers,
               COUNT(*) FILTER (WHERE w.status = 'draining')::bigint AS draining_workers,
               COUNT(*) FILTER (
                   WHERE w.status = 'unhealthy'
                      OR (w.status IN ('active', 'draining')
                          AND w.last_heartbeat_at < now() - interval '2 minutes')
               )::bigint AS unhealthy_workers,
               COALESCE(SUM(w.concurrency_limit) FILTER (WHERE w.status = 'active'), 0)::bigint
                   AS concurrency_limit,
               COALESCE((
                   SELECT COUNT(*)
                   FROM jobs j
                   WHERE j.pool = w.pool
                     AND j.status = 'running'
                     AND j.lease_expires_at > now()
               ), 0)::bigint AS active_leases
        FROM worker_instances w
        WHERE w.status <> 'stopped'
        GROUP BY w.pool
        ORDER BY w.pool
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn operator_overview_counts(
    pool: &PgPool,
    stale_after_seconds: i64,
) -> Result<OperatorOverviewCounts, sqlx::Error> {
    sqlx::query_as::<_, OperatorOverviewCounts>(
        r#"
        SELECT
            (SELECT COUNT(*) FROM source_registry WHERE enabled = true)::bigint
                AS registered_sources,
            (SELECT COUNT(*) FROM source_registry WHERE workflow_run_pending = true)::bigint
                AS pending_admissions,
            (SELECT COUNT(*) FROM batch_execution_record
             WHERE status IN ('pending', 'running', 'awaiting_scheduler', 'retrying'))::bigint
                AS running_executions,
            (SELECT COUNT(*) FROM batch_execution_record WHERE status = 'failed')::bigint
                AS failed_executions,
            (SELECT COUNT(*) FROM jobs WHERE status = 'queued')::bigint AS queue_depth,
            (SELECT COUNT(*) FROM worker_instances WHERE status = 'active')::bigint
                AS active_workers,
            (SELECT COUNT(*) FROM worker_instances
             WHERE status IN ('active', 'draining')
               AND last_heartbeat_at < now() - ($1::text || ' seconds')::interval)::bigint
                AS stale_workers,
            (SELECT COUNT(*) FROM alert_deliveries
             WHERE created_at >= now() - interval '24 hours')::bigint AS recent_alerts
        "#,
    )
    .bind(stale_after_seconds.max(1))
    .fetch_one(pool)
    .await
}

pub async fn list_scheduler_executions(
    pool: &PgPool,
    scheduler_name: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<ExecutionRow>, sqlx::Error> {
    sqlx::query_as::<_, ExecutionRow>(
        r#"
        SELECT *
        FROM batch_execution_record
        WHERE scheduler_name = $1
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(scheduler_name)
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
}

pub async fn list_job_claim_history(
    pool: &PgPool,
    job_id: Uuid,
) -> Result<Vec<JobClaimHistoryRow>, sqlx::Error> {
    sqlx::query_as::<_, JobClaimHistoryRow>(
        r#"
        SELECT * FROM job_claim_history
        WHERE job_id = $1
        ORDER BY occurred_at, uuid
        "#,
    )
    .bind(job_id)
    .fetch_all(pool)
    .await
}

/// Claim one eligible job with a persisted, owner-fenced lease.
///
/// Expired `running` jobs are recoverable; active leases are never selected. The worker
/// must be registered and active, and the task must match its pool and capabilities.
pub async fn claim_next_job_for_worker(
    pool: &PgPool,
    worker_id: Uuid,
    worker_pool: &str,
    _capabilities: &[String],
    lock_seconds: i64,
) -> Result<Option<JobRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let worker: Option<(Vec<String>, Value, i32)> = sqlx::query_as(
        r#"
        SELECT capabilities, labels, concurrency_limit
        FROM worker_instances
        WHERE uuid = $1 AND status = 'active' AND pool = $2
        "#,
    )
    .bind(worker_id)
    .bind(worker_pool)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((capabilities, worker_labels, worker_concurrency_limit)) = worker else {
        tx.rollback().await?;
        return Ok(None);
    };

    // Claims in one pool share a short transaction-scoped lock so capacity checks
    // cannot be raced by other replicas claiming different job rows.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("beampipe:worker-pool:{worker_pool}"))
        .execute(&mut *tx)
        .await?;

    let worker_active_leases: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM jobs
        WHERE status = 'running'
          AND lease_owner = $1
          AND COALESCE(lease_expires_at, locked_until, '-infinity'::TIMESTAMPTZ) > now()
        "#,
    )
    .bind(worker_id)
    .fetch_one(&mut *tx)
    .await?;
    if worker_active_leases >= i64::from(worker_concurrency_limit) {
        tx.commit().await?;
        return Ok(None);
    }

    let pool_concurrency_limit: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(concurrency_limit), 0)::BIGINT
        FROM worker_instances
        WHERE pool = $1 AND status = 'active'
        "#,
    )
    .bind(worker_pool)
    .fetch_one(&mut *tx)
    .await?;
    let pool_active_leases: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM jobs
        WHERE pool = $1
          AND status = 'running'
          AND COALESCE(lease_expires_at, locked_until, '-infinity'::TIMESTAMPTZ) > now()
        "#,
    )
    .bind(worker_pool)
    .fetch_one(&mut *tx)
    .await?;
    if pool_concurrency_limit < 1 || pool_active_leases >= pool_concurrency_limit {
        tx.commit().await?;
        return Ok(None);
    }

    let candidate: Option<(Uuid, String, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        r#"
        SELECT uuid, status, lease_owner, lease_token
        FROM jobs
        WHERE pool = $1
          AND (required_capability IS NULL OR required_capability = ANY($2::TEXT[]))
          AND $3::JSONB @> required_labels
          AND (
              (
                  status = 'queued'
                  AND next_run_at <= now()
                  AND (lease_expires_at IS NULL OR lease_expires_at <= now())
                  AND (locked_until IS NULL OR locked_until <= now())
              )
              OR (
                  status = 'running'
                  AND COALESCE(lease_expires_at, locked_until, '-infinity'::TIMESTAMPTZ) <= now()
              )
          )
        ORDER BY (
            priority + LEAST(
                1000,
                FLOOR(EXTRACT(EPOCH FROM (now() - created_at)) / 300)::INTEGER
            )
        ) DESC, next_run_at ASC, created_at ASC
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(worker_pool)
    .bind(capabilities)
    .bind(worker_labels)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((job_id, previous_status, previous_owner, previous_token)) = candidate else {
        tx.commit().await?;
        return Ok(None);
    };
    let lease_token = Uuid::now_v7();
    let job = sqlx::query_as::<_, JobRow>(
        r#"
        UPDATE jobs
        SET status = 'running',
            attempts = attempts + 1,
            lease_owner = $2,
            lease_token = $3,
            lease_expires_at = now() + ($4::TEXT || ' seconds')::INTERVAL,
            locked_until = now() + ($4::TEXT || ' seconds')::INTERVAL,
            heartbeat_at = now(),
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(lock_seconds.max(1))
    .fetch_one(&mut *tx)
    .await?;
    let event = if previous_status == "running" {
        "recovered"
    } else {
        "claimed"
    };
    sqlx::query(
        r#"
        INSERT INTO job_claim_history
            (uuid, job_id, worker_id, lease_token, event, details)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(event)
    .bind(json!({
        "previous_status": previous_status,
        "previous_owner": previous_owner,
        "previous_lease_token": previous_token,
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(job))
}

pub async fn renew_job_lease(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    lock_seconds: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE jobs
        SET lease_expires_at = now() + ($4::TEXT || ' seconds')::INTERVAL,
            locked_until = now() + ($4::TEXT || ' seconds')::INTERVAL,
            heartbeat_at = now(),
            updated_at = now()
        WHERE uuid = $1
          AND status = 'running'
          AND lease_owner = $2
          AND lease_token = $3
          AND lease_expires_at > now()
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(lock_seconds.max(1))
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

async fn record_claim_terminal_event(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    job_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    event: &str,
    details: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO job_claim_history
            (uuid, job_id, worker_id, lease_token, event, details)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(event)
    .bind(details)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn complete_job_with_lease(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"
        UPDATE jobs
        SET status = 'completed',
            locked_until = NULL,
            lease_owner = NULL,
            lease_token = NULL,
            lease_expires_at = NULL,
            heartbeat_at = NULL,
            updated_at = now()
        WHERE uuid = $1 AND status = 'running'
          AND lease_owner = $2 AND lease_token = $3
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .execute(&mut *tx)
    .await?;
    let updated = result.rows_affected() == 1;
    if updated {
        record_claim_terminal_event(
            &mut tx,
            job_id,
            worker_id,
            lease_token,
            "completed",
            json!({}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(updated)
}

pub async fn reschedule_recurring_job_with_lease(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    delay_secs: i64,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"
        UPDATE jobs
        SET status = 'queued',
            next_run_at = now() + ($4::TEXT || ' seconds')::INTERVAL,
            locked_until = NULL,
            lease_owner = NULL,
            lease_token = NULL,
            lease_expires_at = NULL,
            heartbeat_at = NULL,
            attempts = 0,
            last_error = NULL,
            failure_class = NULL,
            updated_at = now()
        WHERE uuid = $1 AND status = 'running'
          AND lease_owner = $2 AND lease_token = $3
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(delay_secs.max(1))
    .execute(&mut *tx)
    .await?;
    let updated = result.rows_affected() == 1;
    if updated {
        record_claim_terminal_event(
            &mut tx,
            job_id,
            worker_id,
            lease_token,
            "requeued",
            json!({"delay_seconds": delay_secs.max(1), "recurring": true}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(updated)
}

pub async fn fail_job_permanently_with_lease(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    error: &str,
    failure_class: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"
        UPDATE jobs
        SET status = 'failed',
            locked_until = NULL,
            lease_owner = NULL,
            lease_token = NULL,
            lease_expires_at = NULL,
            heartbeat_at = NULL,
            last_error = $4,
            failure_class = $5,
            updated_at = now()
        WHERE uuid = $1 AND status = 'running'
          AND lease_owner = $2 AND lease_token = $3
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(error)
    .bind(failure_class)
    .execute(&mut *tx)
    .await?;
    let updated = result.rows_affected() == 1;
    if updated {
        record_claim_terminal_event(
            &mut tx,
            job_id,
            worker_id,
            lease_token,
            "failed",
            json!({"failure_class": failure_class}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(updated)
}

pub async fn fail_or_retry_job_with_lease(
    pool: &PgPool,
    job_id: Uuid,
    worker_id: Uuid,
    lease_token: Uuid,
    error: &str,
    failure_class: &str,
    retry_delay_seconds: i64,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let status: Option<String> = sqlx::query_scalar(
        r#"
        UPDATE jobs
        SET status = CASE WHEN attempts >= max_attempts THEN 'dead_letter' ELSE 'queued' END,
            next_run_at = now() + ($6::TEXT || ' seconds')::INTERVAL,
            locked_until = NULL,
            lease_owner = NULL,
            lease_token = NULL,
            lease_expires_at = NULL,
            heartbeat_at = NULL,
            last_error = $4,
            failure_class = $5,
            dead_lettered_at = CASE WHEN attempts >= max_attempts THEN now() ELSE NULL END,
            dead_letter_reason = CASE WHEN attempts >= max_attempts THEN $4 ELSE NULL END,
            updated_at = now()
        WHERE uuid = $1 AND status = 'running'
          AND lease_owner = $2 AND lease_token = $3
        RETURNING status
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(lease_token)
    .bind(error)
    .bind(failure_class)
    .bind(retry_delay_seconds.max(1))
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(status) = status.as_deref() {
        let event = if status == "dead_letter" {
            "dead_lettered"
        } else {
            "requeued"
        };
        record_claim_terminal_event(
            &mut tx,
            job_id,
            worker_id,
            lease_token,
            event,
            json!({
                "failure_class": failure_class,
                "retry_delay_seconds": retry_delay_seconds.max(1),
            }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(status.is_some())
}

#[deprecated(note = "use claim_next_job_for_worker for owner-fenced leases")]
pub async fn claim_next_job(
    pool: &PgPool,
    lock_seconds: i64,
) -> Result<Option<JobRow>, sqlx::Error> {
    sqlx::query_as::<_, JobRow>(
        r#"
        WITH candidate AS (
            SELECT uuid
            FROM jobs
            WHERE status = 'queued'
              AND next_run_at <= now()
              AND (locked_until IS NULL OR locked_until <= now())
            ORDER BY next_run_at ASC, created_at ASC
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE jobs
        SET status = 'running',
            attempts = attempts + 1,
            locked_until = now() + ($1::text || ' seconds')::interval,
            updated_at = now()
        WHERE uuid IN (SELECT uuid FROM candidate)
        RETURNING *
        "#,
    )
    .bind(lock_seconds)
    .fetch_optional(pool)
    .await
}

pub async fn complete_job(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE jobs SET status = 'completed', locked_until = NULL, lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL, heartbeat_at = NULL, updated_at = now() WHERE uuid = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Re-queue a recurring job after a successful tick (keeps same row, sets `next_run_at`).
pub async fn reschedule_recurring_job(
    pool: &PgPool,
    id: Uuid,
    delay_secs: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE jobs
        SET status = 'queued',
            next_run_at = now() + ($2::text || ' seconds')::interval,
            locked_until = NULL,
            lease_owner = NULL,
            lease_token = NULL,
            lease_expires_at = NULL,
            heartbeat_at = NULL,
            attempts = 0,
            last_error = NULL,
            updated_at = now()
        WHERE uuid = $1
        "#,
    )
    .bind(id)
    .bind(delay_secs.max(1))
    .execute(pool)
    .await?;
    Ok(())
}

/// Active Slurm executions that need remote scheduler polling.
pub async fn list_slurm_executions_pending_poll(
    pool: &PgPool,
) -> Result<Vec<crate::models::ExecutionRow>, sqlx::Error> {
    sqlx::query_as::<_, crate::models::ExecutionRow>(
        r#"
        SELECT *
        FROM batch_execution_record
        WHERE scheduler_name = 'slurm'
          AND scheduler_job_id IS NOT NULL
          AND status IN ('awaiting_scheduler', 'running')
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn list_slurm_submissions_pending_reconciliation(
    pool: &PgPool,
) -> Result<Vec<crate::models::ExecutionRow>, sqlx::Error> {
    sqlx::query_as::<_, crate::models::ExecutionRow>(
        r#"
        SELECT *
        FROM batch_execution_record
        WHERE scheduler_name = 'slurm'
          AND scheduler_job_id IS NULL
          AND daliuge_session_id IS NOT NULL
          AND submission_state IN ('in_flight', 'uncertain')
          AND terminal_outcome IS NULL
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

/// Active REST/DIM executions that need session status polling.
pub async fn list_rest_executions_pending_poll(
    pool: &PgPool,
) -> Result<Vec<crate::models::ExecutionRow>, sqlx::Error> {
    sqlx::query_as::<_, crate::models::ExecutionRow>(
        r#"
        SELECT *
        FROM batch_execution_record
        WHERE scheduler_name = 'daliuge'
          AND daliuge_session_id IS NOT NULL
          AND (
              status = 'running'
              OR submission_state IN ('in_flight', 'uncertain')
          )
          AND terminal_outcome IS NULL
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn fail_or_retry_job(pool: &PgPool, id: Uuid, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE jobs
        SET status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END,
            next_run_at = now() + ($2::text || ' seconds')::interval,
            locked_until = NULL,
            lease_owner = NULL,
            lease_token = NULL,
            lease_expires_at = NULL,
            heartbeat_at = NULL,
            last_error = $3,
            updated_at = now()
        WHERE uuid = $1
        "#,
    )
    .bind(id)
    .bind(30_i64)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

fn status_str(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Pending => "pending",
        ExecutionStatus::Running => "running",
        ExecutionStatus::AwaitingScheduler => "awaiting_scheduler",
        ExecutionStatus::NotSubmitted => "not_submitted",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Retrying => "retrying",
        ExecutionStatus::Cancelled => "cancelled",
    }
}

fn phase_str(phase: beampipe_domain::ExecutionPhase) -> &'static str {
    match phase {
        beampipe_domain::ExecutionPhase::StageAndManifest => "stage_and_manifest",
        beampipe_domain::ExecutionPhase::Submit => "submit",
    }
}

pub async fn max_pending_age_by_module(pool: &PgPool) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT project_module,
               COALESCE(
                   EXTRACT(EPOCH FROM (now() - MIN(workflow_run_pending_at)))::bigint,
                   0
               )
        FROM source_registry
        WHERE enabled = true AND workflow_run_pending = true
        GROUP BY project_module
        "#,
    )
    .fetch_all(pool)
    .await
}

/// Active work on a source: discovery claim, execution-scheduler claim, or non-terminal execution.
pub async fn list_sources_currently_processing(
    pool: &PgPool,
) -> Result<Vec<(String, String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT project_module, source_identifier, phase
        FROM (
            SELECT project_module, source_identifier, 'discovering'::text AS phase
            FROM source_registry
            WHERE enabled = true
              AND discovery_claim_token IS NOT NULL
              AND discovery_claim_expires_at > now()
              AND project_module NOT LIKE 'fail_requeue_%'
              AND project_module NOT LIKE 'sig_test_%'
              AND project_module NOT LIKE 'test_%'
              AND project_module NOT LIKE 'exec_sig_%'
            UNION ALL
            SELECT project_module, source_identifier, 'admitting'
            FROM source_registry
            WHERE workflow_claim_token IS NOT NULL
              AND workflow_claim_expires_at > now()
              AND project_module NOT LIKE 'fail_requeue_%'
              AND project_module NOT LIKE 'sig_test_%'
              AND project_module NOT LIKE 'test_%'
              AND project_module NOT LIKE 'exec_sig_%'
            UNION ALL
            SELECT e.project_module,
                   elem->>'source_identifier',
                   'executing'
            FROM batch_execution_record e
            CROSS JOIN LATERAL jsonb_array_elements(e.sources) AS elem
            WHERE e.status IN ('pending', 'running', 'awaiting_scheduler', 'retrying')
              AND elem->>'source_identifier' IS NOT NULL
              AND e.project_module NOT LIKE 'fail_requeue_%'
              AND e.project_module NOT LIKE 'sig_test_%'
              AND e.project_module NOT LIKE 'test_%'
              AND e.project_module NOT LIKE 'exec_sig_%'
        ) AS active
        "#,
    )
    .fetch_all(pool)
    .await
}

/// Remove all registry rows for a module (integration-test teardown).
pub async fn delete_all_sources_for_project_module(
    pool: &PgPool,
    project_module: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM source_registry WHERE project_module = $1")
        .bind(project_module)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Distinct test modules still present in the DB (for zeroing stale aggregate gauges).
pub async fn list_internal_test_project_modules(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT DISTINCT project_module
        FROM source_registry
        WHERE project_module LIKE 'fail_requeue_%'
           OR project_module LIKE 'sig_test_%'
           OR project_module LIKE 'test_%'
           OR project_module LIKE 'exec_sig_%'
        "#,
    )
    .fetch_all(pool)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_provenance_event<'e, E>(
    executor: E,
    event_type: &str,
    project_module: &str,
    source_identifier: Option<&str>,
    execution_id: Option<Uuid>,
    actor: Option<&str>,
    correlation_id: Option<&str>,
    payload: &serde_json::Value,
) -> Result<Uuid, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO provenance_events
            (id, event_type, project_module, source_identifier, execution_id, actor, correlation_id, payload)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(id)
    .bind(event_type)
    .bind(project_module)
    .bind(source_identifier)
    .bind(execution_id)
    .bind(actor)
    .bind(correlation_id)
    .bind(payload)
    .execute(executor)
    .await?;
    Ok(id)
}

pub async fn list_provenance_events_for_execution(
    pool: &PgPool,
    execution_id: Uuid,
    limit: i64,
) -> Result<Vec<crate::models::ProvenanceEventRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT * FROM provenance_events
        WHERE execution_id = $1
        ORDER BY occurred_at ASC
        LIMIT $2
        "#,
    )
    .bind(execution_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await
}

pub async fn list_provenance_events_for_source(
    pool: &PgPool,
    project_module: &str,
    source_identifier: &str,
    limit: i64,
) -> Result<Vec<crate::models::ProvenanceEventRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT * FROM provenance_events
        WHERE project_module = $1 AND source_identifier = $2
        ORDER BY occurred_at DESC
        LIMIT $3
        "#,
    )
    .bind(project_module)
    .bind(source_identifier)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await
}

pub async fn list_provenance_events_for_project(
    pool: &PgPool,
    project_module: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<crate::models::ProvenanceEventRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT * FROM provenance_events
        WHERE project_module = $1
        ORDER BY occurred_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(project_module)
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
}

pub async fn list_recent_provenance_events(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<crate::models::ProvenanceEventRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT * FROM provenance_events
        ORDER BY occurred_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
}

pub async fn count_discovery_changed_since(
    pool: &PgPool,
    project_module: &str,
    since: DateTime<Utc>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM provenance_events
        WHERE project_module = $1
          AND event_type = 'discovery.changed'
          AND occurred_at >= $2
        "#,
    )
    .bind(project_module)
    .bind(since)
    .fetch_one(pool)
    .await
}

pub async fn create_notification_channel(
    pool: &PgPool,
    name: &str,
    kind: &str,
    config: &serde_json::Value,
    enabled: bool,
) -> Result<crate::models::NotificationChannelRow, sqlx::Error> {
    sqlx::query_as(
        r#"
        INSERT INTO notification_channels (uuid, name, kind, config, enabled)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(kind)
    .bind(config)
    .bind(enabled)
    .fetch_one(pool)
    .await
}

pub async fn list_notification_channels(
    pool: &PgPool,
) -> Result<Vec<crate::models::NotificationChannelRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM notification_channels ORDER BY name ASC")
        .fetch_all(pool)
        .await
}

pub async fn get_notification_channel(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<crate::models::NotificationChannelRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM notification_channels WHERE uuid = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn update_notification_channel(
    pool: &PgPool,
    id: Uuid,
    name: Option<&str>,
    config: Option<&serde_json::Value>,
    enabled: Option<bool>,
) -> Result<Option<crate::models::NotificationChannelRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        UPDATE notification_channels
        SET name = COALESCE($2, name),
            config = COALESCE($3, config),
            enabled = COALESCE($4, enabled),
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(config)
    .bind(enabled)
    .fetch_optional(pool)
    .await
}

pub async fn delete_notification_channel(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM notification_channels WHERE uuid = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_alert_rule(
    pool: &PgPool,
    name: &str,
    project_module: Option<&str>,
    severity: &str,
    trigger_kind: &str,
    trigger_config: &serde_json::Value,
    channel_ids: &[Uuid],
    cooldown_minutes: i32,
) -> Result<crate::models::AlertRuleRow, sqlx::Error> {
    sqlx::query_as(
        r#"
        INSERT INTO alert_rules
            (uuid, name, project_module, severity, trigger_kind, trigger_config, channel_ids, cooldown_minutes)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(project_module)
    .bind(severity)
    .bind(trigger_kind)
    .bind(trigger_config)
    .bind(channel_ids)
    .bind(cooldown_minutes)
    .fetch_one(pool)
    .await
}

pub async fn list_alert_rules(
    pool: &PgPool,
    project_module: Option<&str>,
) -> Result<Vec<crate::models::AlertRuleRow>, sqlx::Error> {
    match project_module {
        Some(m) => {
            sqlx::query_as(
                "SELECT * FROM alert_rules WHERE project_module IS NULL OR project_module = $1 ORDER BY name",
            )
            .bind(m)
            .fetch_all(pool)
            .await
        }
        None => {
            sqlx::query_as("SELECT * FROM alert_rules ORDER BY name")
                .fetch_all(pool)
                .await
        }
    }
}

pub async fn get_alert_rule(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<crate::models::AlertRuleRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM alert_rules WHERE uuid = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn update_alert_rule(
    pool: &PgPool,
    id: Uuid,
    enabled: Option<bool>,
    trigger_config: Option<&serde_json::Value>,
    channel_ids: Option<&[Uuid]>,
    cooldown_minutes: Option<i32>,
) -> Result<Option<crate::models::AlertRuleRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        UPDATE alert_rules
        SET enabled = COALESCE($2, enabled),
            trigger_config = COALESCE($3, trigger_config),
            channel_ids = COALESCE($4, channel_ids),
            cooldown_minutes = COALESCE($5, cooldown_minutes),
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(enabled)
    .bind(trigger_config)
    .bind(channel_ids)
    .bind(cooldown_minutes)
    .fetch_optional(pool)
    .await
}

pub async fn delete_alert_rule(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM alert_rules WHERE uuid = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn mark_alert_rule_fired(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE alert_rules SET last_fired_at = now() WHERE uuid = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_alert_delivery(
    pool: &PgPool,
    rule_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    status: &str,
    payload: &serde_json::Value,
    error: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO alert_deliveries (uuid, rule_id, channel_id, status, payload, error)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(rule_id)
    .bind(channel_id)
    .bind(status)
    .bind(beampipe_security::redact_value(payload))
    .bind(error.map(beampipe_security::redact_string))
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn list_alert_deliveries(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<crate::models::AlertDeliveryRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM alert_deliveries ORDER BY created_at DESC LIMIT $1")
        .bind(limit.clamp(1, 500))
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::deployment_profile_spec_sha256;
    use serde_json::json;

    #[test]
    fn deployment_profile_hash_is_stable_and_content_addressed() {
        let translation = json!({"algorithm": "metis", "num_partitions": 4});
        let deployment = json!({"kind": "rest_remote", "endpoint": "http://dim:8001"});
        let first = deployment_profile_spec_sha256(
            "setonix",
            Some("production"),
            Some("wallaby_hires"),
            true,
            Some(2),
            &translation,
            &deployment,
        );
        let second = deployment_profile_spec_sha256(
            "setonix",
            Some("production"),
            Some("wallaby_hires"),
            true,
            Some(2),
            &translation,
            &deployment,
        );
        let changed = deployment_profile_spec_sha256(
            "setonix",
            Some("production"),
            Some("wallaby_hires"),
            true,
            Some(2),
            &json!({"algorithm": "metis", "num_partitions": 8}),
            &deployment,
        );

        assert_eq!(first, second);
        assert_ne!(first, changed);
        assert_eq!(first.len(), 64);
    }
}

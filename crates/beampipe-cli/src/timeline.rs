use anyhow::Context;
use beampipe_db::repo;
use beampipe_domain::readiness::{source_execution_status, ArchiveMetadataReadiness};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct TimelineEvent {
    pub at: String,
    pub event_type: String,
    pub correlation_id: Option<String>,
    pub detail: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ExecutionTimeline {
    pub execution_id: Uuid,
    pub status: String,
    pub execution_phase: Option<String>,
    pub project_module: String,
    pub correlation_id: Option<String>,
    pub active_job_id: Option<Uuid>,
    pub events: Vec<TimelineEvent>,
}

pub async fn execution_timeline(pool: &PgPool, id: Uuid) -> anyhow::Result<ExecutionTimeline> {
    let row = repo::get_execution(pool, id)
        .await?
        .context("execution not found")?;
    let trace = repo::execution_trace_summary(pool, id, 100).await?;
    let timeline_events: Vec<TimelineEvent> = trace
        .events
        .into_iter()
        .map(|e| TimelineEvent {
            at: e.occurred_at.to_rfc3339(),
            event_type: e.event_type,
            correlation_id: e.correlation_id,
            detail: e.payload,
        })
        .collect();
    Ok(ExecutionTimeline {
        execution_id: id,
        status: row.status,
        execution_phase: row.execution_phase,
        project_module: row.project_module,
        correlation_id: trace.correlation_id,
        active_job_id: trace.active_job_id,
        events: timeline_events,
    })
}

#[derive(Debug, Serialize)]
pub struct SourceTimeline {
    pub source_id: Uuid,
    pub source_identifier: String,
    pub project_module: String,
    pub status: serde_json::Value,
    pub events: Vec<TimelineEvent>,
}

pub async fn source_timeline(pool: &PgPool, id: Uuid) -> anyhow::Result<SourceTimeline> {
    let source = repo::get_source(pool, id)
        .await?
        .context("source not found")?;
    let metadata_rows = repo::list_source_metadata(pool, &source).await?;
    let metadata: Vec<ArchiveMetadataReadiness> = metadata_rows
        .iter()
        .map(|r| ArchiveMetadataReadiness {
            sbid: r.sbid.clone(),
            metadata_json: r.metadata_json.clone(),
        })
        .collect();
    let status = source_execution_status(
        &source.source_identifier,
        source.enabled,
        source.last_checked_at,
        source.discovery_signature.as_deref(),
        source.last_executed_discovery_signature.as_deref(),
        source.discovery_claim_token.as_deref(),
        source.workflow_run_pending,
        source.workflow_run_pending_at,
        &metadata,
        None,
    );
    let events = repo::list_provenance_events_for_source(
        pool,
        &source.project_module,
        &source.source_identifier,
        100,
    )
    .await?;
    Ok(SourceTimeline {
        source_id: id,
        source_identifier: source.source_identifier,
        project_module: source.project_module,
        status: serde_json::to_value(status)?,
        events: events
            .into_iter()
            .map(|e| TimelineEvent {
                at: e.occurred_at.to_rfc3339(),
                event_type: e.event_type,
                correlation_id: e.correlation_id,
                detail: e.payload,
            })
            .collect(),
    })
}

pub async fn project_timeline(
    pool: &PgPool,
    module: &str,
    limit: i64,
) -> anyhow::Result<Vec<TimelineEvent>> {
    let events = repo::list_provenance_events_for_project(pool, module, limit, 0).await?;
    Ok(events
        .into_iter()
        .map(|e| TimelineEvent {
            at: e.occurred_at.to_rfc3339(),
            event_type: e.event_type,
            correlation_id: e.correlation_id,
            detail: e.payload,
        })
        .collect())
}

pub fn print_table_execution(timeline: &ExecutionTimeline) {
    println!("execution {} ({})", timeline.execution_id, timeline.status);
    if let Some(phase) = &timeline.execution_phase {
        println!("  phase: {phase}");
    }
    if let Some(corr) = &timeline.correlation_id {
        println!("  correlation_id: {corr}");
    }
    if let Some(job) = timeline.active_job_id {
        println!("  active_job_id: {job}");
    }
    println!("  events:");
    for e in &timeline.events {
        println!("    {} {} {:?}", e.at, e.event_type, e.correlation_id);
    }
}

pub fn print_table_source(timeline: &SourceTimeline) {
    println!(
        "source {} ({}) module={}",
        timeline.source_identifier, timeline.source_id, timeline.project_module
    );
    println!("  status: {}", timeline.status);
    println!("  events:");
    for e in &timeline.events {
        println!("    {} {}", e.at, e.event_type);
    }
}

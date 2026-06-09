use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

pub async fn record_provenance_event(
    pool: &PgPool,
    event_type: &str,
    project_module: &str,
    source_identifier: Option<&str>,
    execution_id: Option<Uuid>,
    actor: Option<&str>,
    correlation_id: Option<&str>,
    payload: &serde_json::Value,
) {
    if let Err(e) = super::repo::insert_provenance_event(
        pool,
        event_type,
        project_module,
        source_identifier,
        execution_id,
        actor,
        correlation_id,
        payload,
    )
    .await
    {
        warn!(
            event_type,
            project_module,
            ?execution_id,
            error = %e,
            "event=provenance_insert_failed"
        );
    }
}

use crate::{ApiError, AuthUser};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use beampipe_alerts;
use beampipe_db::{models::*, repo};
use beampipe_security::{
    redact_string, redact_value, secret_paths, unsafe_inline_secret_paths, SecretPolicy,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
pub struct NotificationChannelCreate {
    pub name: String,
    pub kind: String,
    pub config: Value,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct NotificationChannelUpdate {
    pub name: Option<String>,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = observability::NotificationChannelResponse)]
pub struct NotificationChannelResponse {
    pub uuid: Uuid,
    pub name: String,
    pub kind: String,
    pub config: Value,
    pub secret_fields: Vec<String>,
    pub configured_fields: Vec<String>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<NotificationChannelRow> for NotificationChannelResponse {
    fn from(row: NotificationChannelRow) -> Self {
        let configured_fields = row
            .config
            .as_object()
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();
        Self {
            uuid: row.uuid,
            name: row.name,
            kind: row.kind,
            secret_fields: secret_paths(&row.config),
            config: redact_value(&row.config),
            configured_fields,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AlertRuleCreate {
    pub name: String,
    pub project_module: Option<String>,
    #[serde(default = "default_severity")]
    pub severity: String,
    pub trigger_kind: String,
    pub trigger_config: Value,
    pub channel_ids: Vec<Uuid>,
    #[serde(default = "default_cooldown")]
    pub cooldown_minutes: i32,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AlertRuleUpdate {
    pub enabled: Option<bool>,
    pub trigger_config: Option<Value>,
    pub channel_ids: Option<Vec<Uuid>>,
    pub cooldown_minutes: Option<i32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = observability::AlertDeliveryResponse)]
pub struct AlertDeliveryResponse {
    pub uuid: Uuid,
    pub rule_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub status: String,
    pub payload: Value,
    pub error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<AlertDeliveryRow> for AlertDeliveryResponse {
    fn from(row: AlertDeliveryRow) -> Self {
        Self {
            uuid: row.uuid,
            rule_id: row.rule_id,
            channel_id: row.channel_id,
            status: row.status,
            payload: redact_value(&row.payload),
            error: row.error.map(|e| redact_string(&e)),
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(as = observability::ProvenanceEventResponse)]
pub struct ProvenanceEventResponse {
    pub id: Uuid,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    pub event_type: String,
    pub project_module: String,
    pub source_identifier: Option<String>,
    pub execution_id: Option<Uuid>,
    pub actor: Option<String>,
    pub correlation_id: Option<String>,
    pub payload: Value,
}

impl From<ProvenanceEventRow> for ProvenanceEventResponse {
    fn from(row: ProvenanceEventRow) -> Self {
        Self {
            id: row.id,
            occurred_at: row.occurred_at,
            event_type: row.event_type,
            project_module: row.project_module,
            source_identifier: row.source_identifier,
            execution_id: row.execution_id,
            actor: row.actor,
            correlation_id: row.correlation_id,
            payload: redact_value(&row.payload),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ProjectEventsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn default_true() -> bool {
    true
}

fn default_severity() -> String {
    "warning".into()
}

fn default_cooldown() -> i32 {
    60
}

fn validate_notification_config(kind: &str, config: &Value) -> Result<(), ApiError> {
    let policy = SecretPolicy::from_process_env();
    let inline = unsafe_inline_secret_paths(config, policy);
    if !inline.is_empty() {
        beampipe_metrics::record_unsafe_inline_secret_rejected("notification_channel");
        return Err(ApiError::BadRequest(format!(
            "inline secrets are not allowed in production; use env/file secret refs for: {}",
            inline.join(", ")
        )));
    }
    for path in secret_paths(config) {
        let metric_kind = path.rsplit('.').next().unwrap_or(path.as_str());
        beampipe_metrics::record_secret_ref_configured(metric_kind);
    }
    match kind {
        "webhook" => {
            if config.get("url").and_then(Value::as_str).is_none() {
                return Err(ApiError::BadRequest(
                    "webhook config.url is required".into(),
                ));
            }
        }
        "email" => {
            if config.get("smtp_host").and_then(Value::as_str).is_none() {
                return Err(ApiError::BadRequest(
                    "email config.smtp_host is required".into(),
                ));
            }
        }
        _ => return Err(ApiError::BadRequest("kind must be webhook or email".into())),
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v2/notification-channels",
    tag = "alerts",
    responses((status = 200, body = [NotificationChannelResponse]))
)]
pub async fn list_notification_channels(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<NotificationChannelResponse>>, ApiError> {
    Ok(Json(
        repo::list_notification_channels(&state.pool)
            .await?
            .into_iter()
            .map(NotificationChannelResponse::from)
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v2/notification-channels",
    tag = "alerts",
    request_body = NotificationChannelCreate,
    responses((status = 201, body = NotificationChannelResponse))
)]
pub async fn create_notification_channel(
    State(state): State<Arc<crate::AppState>>,
    user: AuthUser,
    Json(req): Json<NotificationChannelCreate>,
) -> Result<(StatusCode, Json<NotificationChannelResponse>), ApiError> {
    user.require_superuser()?;
    if !matches!(req.kind.as_str(), "webhook" | "email") {
        return Err(ApiError::BadRequest("kind must be webhook or email".into()));
    }
    validate_notification_config(&req.kind, &req.config)?;
    Ok((
        StatusCode::CREATED,
        Json(
            repo::create_notification_channel(
                &state.pool,
                &req.name,
                &req.kind,
                &req.config,
                req.enabled,
            )
            .await?
            .into(),
        ),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/v2/notification-channels/{id}",
    tag = "alerts",
    params(("id" = Uuid, Path, description = "Channel UUID")),
    request_body = NotificationChannelUpdate,
    responses((status = 200, body = NotificationChannelResponse), (status = 404))
)]
pub async fn update_notification_channel(
    State(state): State<Arc<crate::AppState>>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<NotificationChannelUpdate>,
) -> Result<Json<NotificationChannelResponse>, ApiError> {
    user.require_superuser()?;
    if let Some(config) = req.config.as_ref() {
        let kind = repo::get_notification_channel(&state.pool, id)
            .await?
            .map(|row| row.kind)
            .ok_or(ApiError::NotFound)?;
        validate_notification_config(&kind, config)?;
    }
    repo::update_notification_channel(
        &state.pool,
        id,
        req.name.as_deref(),
        req.config.as_ref(),
        req.enabled,
    )
    .await?
    .map(NotificationChannelResponse::from)
    .map(Json)
    .ok_or(ApiError::NotFound)
}

#[utoipa::path(
    delete,
    path = "/api/v2/notification-channels/{id}",
    tag = "alerts",
    params(("id" = Uuid, Path, description = "Channel UUID")),
    responses((status = 204), (status = 404))
)]
pub async fn delete_notification_channel(
    State(state): State<Arc<crate::AppState>>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    user.require_superuser()?;
    if repo::delete_notification_channel(&state.pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(
    post,
    path = "/api/v2/notification-channels/{id}/test",
    tag = "alerts",
    params(("id" = Uuid, Path, description = "Channel UUID")),
    responses((status = 200))
)]
pub async fn test_notification_channel(
    State(state): State<Arc<crate::AppState>>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    user.require_superuser()?;
    let delivery_id = beampipe_alerts::send_test_notification(&state.pool, id)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(
        json!({"delivery_id": delivery_id, "status": "sent_or_failed"}),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/alert-rules",
    tag = "alerts",
    responses((status = 200, body = [AlertRuleRow]))
)]
pub async fn list_alert_rules(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Query(query): Query<ProjectEventsQuery>,
) -> Result<Json<Vec<AlertRuleRow>>, ApiError> {
    let _ = query;
    Ok(Json(repo::list_alert_rules(&state.pool, None).await?))
}

#[utoipa::path(
    post,
    path = "/api/v2/alert-rules",
    tag = "alerts",
    request_body = AlertRuleCreate,
    responses((status = 201, body = AlertRuleRow))
)]
pub async fn create_alert_rule(
    State(state): State<Arc<crate::AppState>>,
    user: AuthUser,
    Json(req): Json<AlertRuleCreate>,
) -> Result<(StatusCode, Json<AlertRuleRow>), ApiError> {
    user.require_superuser()?;
    Ok((
        StatusCode::CREATED,
        Json(
            repo::create_alert_rule(
                &state.pool,
                &req.name,
                req.project_module.as_deref(),
                &req.severity,
                &req.trigger_kind,
                &req.trigger_config,
                &req.channel_ids,
                req.cooldown_minutes,
            )
            .await?,
        ),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/v2/alert-rules/{id}",
    tag = "alerts",
    params(("id" = Uuid, Path, description = "Rule UUID")),
    request_body = AlertRuleUpdate,
    responses((status = 200, body = AlertRuleRow), (status = 404))
)]
pub async fn update_alert_rule(
    State(state): State<Arc<crate::AppState>>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<AlertRuleUpdate>,
) -> Result<Json<AlertRuleRow>, ApiError> {
    user.require_superuser()?;
    repo::update_alert_rule(
        &state.pool,
        id,
        req.enabled,
        req.trigger_config.as_ref(),
        req.channel_ids.as_deref(),
        req.cooldown_minutes,
    )
    .await?
    .map(Json)
    .ok_or(ApiError::NotFound)
}

#[utoipa::path(
    delete,
    path = "/api/v2/alert-rules/{id}",
    tag = "alerts",
    params(("id" = Uuid, Path, description = "Rule UUID")),
    responses((status = 204), (status = 404))
)]
pub async fn delete_alert_rule(
    State(state): State<Arc<crate::AppState>>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    user.require_superuser()?;
    if repo::delete_alert_rule(&state.pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(
    get,
    path = "/api/v2/alert-deliveries",
    tag = "alerts",
    responses((status = 200, body = [AlertDeliveryResponse]))
)]
pub async fn list_alert_deliveries(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Query(query): Query<ProjectEventsQuery>,
) -> Result<Json<Vec<AlertDeliveryResponse>>, ApiError> {
    Ok(Json(
        repo::list_alert_deliveries(&state.pool, query.limit.unwrap_or(50))
            .await?
            .into_iter()
            .map(AlertDeliveryResponse::from)
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/executions/{id}/events",
    tag = "provenance",
    params(("id" = Uuid, Path, description = "Execution UUID")),
    responses((status = 200, body = [ProvenanceEventResponse]))
)]
pub async fn list_execution_events(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ProvenanceEventResponse>>, ApiError> {
    Ok(Json(
        repo::list_provenance_events_for_execution(&state.pool, id, 100)
            .await?
            .into_iter()
            .map(ProvenanceEventResponse::from)
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/sources/{id}/events",
    tag = "provenance",
    params(("id" = Uuid, Path, description = "Source UUID")),
    responses((status = 200, body = [ProvenanceEventResponse]))
)]
pub async fn list_source_events(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ProvenanceEventResponse>>, ApiError> {
    let source = repo::get_source(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(
        repo::list_provenance_events_for_source(
            &state.pool,
            &source.project_module,
            &source.source_identifier,
            100,
        )
        .await?
        .into_iter()
        .map(ProvenanceEventResponse::from)
        .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/projects/{module}/events",
    tag = "provenance",
    params(("module" = String, Path, description = "Project module name")),
    responses((status = 200, body = [ProvenanceEventResponse]))
)]
pub async fn list_project_events(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Path(module): Path<String>,
    Query(query): Query<ProjectEventsQuery>,
) -> Result<Json<Vec<ProvenanceEventResponse>>, ApiError> {
    Ok(Json(
        repo::list_provenance_events_for_project(
            &state.pool,
            &module,
            query.limit.unwrap_or(50),
            query.offset.unwrap_or(0),
        )
        .await?
        .into_iter()
        .map(ProvenanceEventResponse::from)
        .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notification_response_redacts_config() {
        let row = NotificationChannelRow {
            uuid: Uuid::now_v7(),
            name: "ops".into(),
            kind: "email".into(),
            config: json!({
                "smtp_host": "smtp.example.test",
                "password": "plain",
                "headers": {"Authorization": {"env": "ALERT_AUTH"}}
            }),
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: None,
        };
        let response = NotificationChannelResponse::from(row);
        assert_eq!(response.config["password"], beampipe_security::REDACTED);
        assert_eq!(
            response.config["headers"]["Authorization"],
            beampipe_security::REDACTED
        );
        assert!(response.secret_fields.contains(&"password".to_string()));
    }

    #[test]
    fn production_rejects_inline_notification_secret() {
        std::env::set_var("BEAMPIPE_ENV", "production");
        std::env::remove_var("BEAMPIPE_ALLOW_INLINE_SECRETS");
        let config = json!({
            "smtp_host": "smtp.example.test",
            "password": "plain"
        });
        assert!(validate_notification_config("email", &config).is_err());
        std::env::remove_var("BEAMPIPE_ENV");
    }
}

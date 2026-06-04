use crate::{ApiError, AuthUser};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use beampipe_alerts;
use beampipe_db::{models::*, repo};
use serde::Deserialize;
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

#[utoipa::path(
    get,
    path = "/api/v2/notification-channels",
    tag = "alerts",
    responses((status = 200, body = [NotificationChannelRow]))
)]
pub async fn list_notification_channels(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<NotificationChannelRow>>, ApiError> {
    Ok(Json(repo::list_notification_channels(&state.pool).await?))
}

#[utoipa::path(
    post,
    path = "/api/v2/notification-channels",
    tag = "alerts",
    request_body = NotificationChannelCreate,
    responses((status = 201, body = NotificationChannelRow))
)]
pub async fn create_notification_channel(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Json(req): Json<NotificationChannelCreate>,
) -> Result<(StatusCode, Json<NotificationChannelRow>), ApiError> {
    if !matches!(req.kind.as_str(), "webhook" | "email") {
        return Err(ApiError::BadRequest("kind must be webhook or email".into()));
    }
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
            .await?,
        ),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/v2/notification-channels/{id}",
    tag = "alerts",
    params(("id" = Uuid, Path, description = "Channel UUID")),
    request_body = NotificationChannelUpdate,
    responses((status = 200, body = NotificationChannelRow), (status = 404))
)]
pub async fn update_notification_channel(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<NotificationChannelUpdate>,
) -> Result<Json<NotificationChannelRow>, ApiError> {
    repo::update_notification_channel(
        &state.pool,
        id,
        req.name.as_deref(),
        req.config.as_ref(),
        req.enabled,
    )
    .await?
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
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
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
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
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
    AuthUser(_user): AuthUser,
    Json(req): Json<AlertRuleCreate>,
) -> Result<(StatusCode, Json<AlertRuleRow>), ApiError> {
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
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<AlertRuleUpdate>,
) -> Result<Json<AlertRuleRow>, ApiError> {
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
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
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
    responses((status = 200, body = [AlertDeliveryRow]))
)]
pub async fn list_alert_deliveries(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Query(query): Query<ProjectEventsQuery>,
) -> Result<Json<Vec<AlertDeliveryRow>>, ApiError> {
    Ok(Json(
        repo::list_alert_deliveries(&state.pool, query.limit.unwrap_or(50)).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/executions/{id}/events",
    tag = "provenance",
    params(("id" = Uuid, Path, description = "Execution UUID")),
    responses((status = 200, body = [ProvenanceEventRow]))
)]
pub async fn list_execution_events(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ProvenanceEventRow>>, ApiError> {
    Ok(Json(
        repo::list_provenance_events_for_execution(&state.pool, id, 100).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/sources/{id}/events",
    tag = "provenance",
    params(("id" = Uuid, Path, description = "Source UUID")),
    responses((status = 200, body = [ProvenanceEventRow]))
)]
pub async fn list_source_events(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ProvenanceEventRow>>, ApiError> {
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
        .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v2/projects/{module}/events",
    tag = "provenance",
    params(("module" = String, Path, description = "Project module name")),
    responses((status = 200, body = [ProvenanceEventRow]))
)]
pub async fn list_project_events(
    State(state): State<Arc<crate::AppState>>,
    AuthUser(_user): AuthUser,
    Path(module): Path<String>,
    Query(query): Query<ProjectEventsQuery>,
) -> Result<Json<Vec<ProvenanceEventRow>>, ApiError> {
    Ok(Json(
        repo::list_provenance_events_for_project(
            &state.pool,
            &module,
            query.limit.unwrap_or(50),
            query.offset.unwrap_or(0),
        )
        .await?,
    ))
}

mod correlation;
mod observability;
mod openapi;
mod rate_limit;
mod route_metrics;

use axum::{
    body::Body,
    extract::{FromRef, FromRequestParts, Path, Query, State},
    http::{request::Parts, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use beampipe_adapters::probe_tap_health;
use beampipe_config::Settings;
use beampipe_db::{models::*, repo};
use beampipe_domain::{
    readiness::{
        parsed_source_readiness_error, source_execution_status, ArchiveMetadataReadiness,
        RegisteredSourceReadiness, SourceExecutionStatus,
    },
    ExecutionStatus, LedgerPatch,
};
use beampipe_jobs::{spawn_workers, WorkerConfig};
use beampipe_metrics as metrics;
use beampipe_orchestration::{cancel::CancelParams, cancel_scheduler_session};
use beampipe_profiles::DeploymentProfile;
use beampipe_project::{ProjectConfig, ValidationDiagnostic, ValidationReport, WasmHost};
use beampipe_security::{redact_string, redact_value, unsafe_inline_secret_paths, SecretPolicy};
use chrono::Utc;
use rate_limit::{check_rate_limit, client_ip, RateLimitError, RateLimiter};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub settings: Settings,
    pub rate_limiter: RateLimiter,
    pub wasm_host: Arc<WasmHost>,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health, ready, metrics, health_tap, login, refresh, logout, current_user, create_source, bulk_create_sources, discover_sources,
        list_sources, get_source, get_source_status, update_source, delete_source, get_source_metadata,
        list_source_executions, prepare_execution, create_execution, list_executions, get_execution,
        execution_status, execution_summary, execution_ledger_snapshot, patch_execution, execute_execution,
        upload_project_config, get_project_config, list_project_config_versions,
        upload_project_config_wasm, get_project_config_wasm,
        list_projects, list_project_contracts, get_project_contract,
        enqueue_job,
        create_deployment_profile, list_deployment_profiles, get_deployment_profile,
        update_deployment_profile, delete_deployment_profile,
        observability::list_notification_channels, observability::create_notification_channel,
        observability::update_notification_channel, observability::delete_notification_channel,
        observability::test_notification_channel, observability::list_alert_rules,
        observability::create_alert_rule, observability::update_alert_rule,
        observability::delete_alert_rule, observability::list_alert_deliveries,
        observability::list_execution_events, observability::list_source_events,
        observability::list_project_events
    ),
    components(schemas(
        HealthResponse, ReadyResponse, LoginRequest, TokenResponse, CurrentUserResponse,
        RefreshRequest, LogoutRequest,
        SourceCreate, SourceBulkCreate, SourceBulkCreateResponse, SourceUpdate,
        DiscoverTriggerRequest, DiscoverTriggerResponse, SourceRegistryRow, ArchiveMetadataResponse,
        ExecutionCreate, ExecutionPatchRequest, ExecuteRequest, ExecutionStatus,
        JobCreate, JobResponse, WasmUploadResponse,
        ProjectConfig, ValidationReport,
        beampipe_project::ProjectMetadata,
        beampipe_project::AdapterConfig,
        beampipe_project::TapConfig,
        beampipe_project::GraphConfig,
        beampipe_project::DiscoveryConfig,
        beampipe_project::DiscoveryQuery,
        beampipe_project::PrepareMetadataConfig,
        beampipe_project::SignatureConfig,
        beampipe_project::ManifestConfig,
        beampipe_project::GraphPatch,
        beampipe_project::AutomationConfig,
        beampipe_project::DiscoveryAutomationConfig,
        beampipe_project::ExecutionAutomationConfig,
        beampipe_project::ExtensionConfig,
        beampipe_project::DefinitionsConfig,
        beampipe_project::SourceIdentityConfig,
        beampipe_project::TemplateVarSpec,
        beampipe_project::TransformSpec,
        DeploymentProfile, DeploymentProfileResponse,
        beampipe_profiles::DaliugeTranslationConfig,
        beampipe_profiles::DaliugeAlgo,
        beampipe_profiles::DeploymentConfig,
        beampipe_profiles::RestRemoteDeploymentConfig,
        beampipe_profiles::SlurmRemoteDeploymentConfig,
        SourceExecutionStatus,
        observability::NotificationChannelCreate, observability::NotificationChannelUpdate,
        observability::NotificationChannelResponse, observability::AlertDeliveryResponse,
        observability::ProvenanceEventResponse, beampipe_security::SecretRef,
        observability::AlertRuleCreate, observability::AlertRuleUpdate,
        ProvenanceEventRow, NotificationChannelRow, AlertRuleRow, AlertDeliveryRow,
        ProvenanceSummary,
    )),
    tags(
        (name = "health", description = "Liveness, readiness, and archive TAP connectivity probes."),
        (name = "auth", description = "OAuth2 password flow and token refresh."),
        (name = "sources", description = "Source registry: register astronomical sources, trigger discovery, and read archive metadata."),
        (name = "executions", description = "Batch execution ledger: create runs, enqueue staging/submit jobs, and inspect status."),
        (name = "project-configs", description = "Registered project modules and versioned survey configuration."),
        (name = "jobs", description = "Postgres-backed async jobs."),
        (name = "deployment-profiles", description = "DALiuGE deployment profiles (translation + REST/Slurm remote deployment configuration)."),
        (name = "alerts", description = "Notification channels and alert rules."),
        (name = "provenance", description = "Audit event stream.")
    )
)]
pub struct ApiDoc;

pub use openapi::{build_openapi, export_openapi_json};

pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let cors = cors_layer(&state.settings);
    let sensitive = Router::new()
        .route("/api/v2/login", post(login))
        .route("/api/v2/executions", post(create_execution))
        .route("/api/v2/project-configs", post(upload_project_config))
        .route(
            "/api/v2/project-configs/:id/wasm",
            post(upload_project_config_wasm),
        )
        .route("/api/v2/jobs", post(enqueue_job))
        .route("/api/v2/executions/:id/execute", post(execute_execution))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ));
    Router::new()
        .merge(sensitive)
        .route("/api/v2/health", get(health))
        .route("/api/v2/health/tap", get(health_tap))
        .route("/api/v2/metrics", get(metrics))
        .route("/api/v2/ready", get(ready))
        .route("/api/v2/refresh", post(refresh))
        .route("/api/v2/logout", post(logout))
        .route("/api/v2/user/me", get(current_user))
        .route("/api/v2/sources", post(create_source).get(list_sources))
        .route("/api/v2/sources/bulk", post(bulk_create_sources))
        .route("/api/v2/sources/discover", post(discover_sources))
        .route(
            "/api/v2/sources/:id",
            get(get_source).patch(update_source).delete(delete_source),
        )
        .route("/api/v2/sources/:id/status", get(get_source_status))
        .route("/api/v2/sources/:id/metadata", get(get_source_metadata))
        .route(
            "/api/v2/sources/:id/executions",
            get(list_source_executions),
        )
        .route("/api/v2/executions/prepare", post(prepare_execution))
        .route("/api/v2/executions", get(list_executions))
        .route(
            "/api/v2/executions/:id",
            get(get_execution).patch(patch_execution),
        )
        .route("/api/v2/executions/:id/status", get(execution_status))
        .route("/api/v2/executions/:id/summary", get(execution_summary))
        .route(
            "/api/v2/executions/:id/ledger-snapshot",
            get(execution_ledger_snapshot),
        )
        .route("/api/v2/project-configs/:id", get(get_project_config))
        .route(
            "/api/v2/project-configs/:id/wasm/:sha256",
            get(get_project_config_wasm),
        )
        .route(
            "/api/v2/project-configs/:id/versions",
            get(list_project_config_versions),
        )
        .route("/api/v2/projects", get(list_projects))
        .route("/api/v2/projects/contracts", get(list_project_contracts))
        .route("/api/v2/projects/contracts/:id", get(get_project_contract))
        .route(
            "/api/v2/deployment-profiles",
            post(create_deployment_profile).get(list_deployment_profiles),
        )
        .route(
            "/api/v2/deployment-profiles/:id",
            get(get_deployment_profile)
                .patch(update_deployment_profile)
                .delete(delete_deployment_profile),
        )
        .route(
            "/api/v2/notification-channels",
            get(observability::list_notification_channels)
                .post(observability::create_notification_channel),
        )
        .route(
            "/api/v2/notification-channels/:id",
            axum::routing::patch(observability::update_notification_channel)
                .delete(observability::delete_notification_channel),
        )
        .route(
            "/api/v2/notification-channels/:id/test",
            post(observability::test_notification_channel),
        )
        .route(
            "/api/v2/alert-rules",
            get(observability::list_alert_rules).post(observability::create_alert_rule),
        )
        .route(
            "/api/v2/alert-rules/:id",
            axum::routing::patch(observability::update_alert_rule)
                .delete(observability::delete_alert_rule),
        )
        .route(
            "/api/v2/alert-deliveries",
            get(observability::list_alert_deliveries),
        )
        .route(
            "/api/v2/executions/:id/events",
            get(observability::list_execution_events),
        )
        .route(
            "/api/v2/sources/:id/events",
            get(observability::list_source_events),
        )
        .route(
            "/api/v2/projects/:module/events",
            get(observability::list_project_events),
        )
        .merge(SwaggerUi::new("/api/v2/docs").url("/api/v2/openapi.json", openapi::build_openapi()))
        .layer(cors)
        .layer(middleware::from_fn(api_metrics_middleware))
        .layer(middleware::from_fn(correlation::correlation_middleware))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn cors_layer(settings: &Settings) -> CorsLayer {
    if let Some(raw) = settings.cors_allow_origins.as_deref() {
        let origins: Vec<HeaderValue> = raw
            .split(',')
            .filter_map(|origin| origin.trim().parse::<HeaderValue>().ok())
            .collect();
        if !origins.is_empty() {
            return CorsLayer::new()
                .allow_origin(origins)
                .allow_methods(Any)
                .allow_headers(Any);
        }
    }
    CorsLayer::permissive()
}

fn reject_inline_secrets_in_production(
    settings: &Settings,
    surface: &str,
    value: &Value,
) -> Result<(), ApiError> {
    let policy = SecretPolicy::from_env_name(&settings.beampipe_env);
    let paths = unsafe_inline_secret_paths(value, policy);
    if !paths.is_empty() {
        metrics::record_unsafe_inline_secret_rejected(surface);
        return Err(ApiError::BadRequest(format!(
            "inline secrets are not allowed in production for {surface}; use env/file secret refs for: {}",
            paths.join(", ")
        )));
    }
    Ok(())
}

async fn api_metrics_middleware(request: Request<Body>, next: Next) -> Response {
    let method = request.method().to_string();
    let route = route_metrics::normalize_api_route(request.uri().path());
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    metrics::record_api_request_duration(&method, &route, status, started.elapsed().as_secs_f64());
    response
}

pub async fn serve(settings: Settings, pool: PgPool, with_worker: bool) -> anyhow::Result<()> {
    if let Err(errors) = beampipe_orchestration::validate_security(&settings) {
        anyhow::bail!("security validation failed:\n  - {}", errors.join("\n  - "));
    }
    metrics::init_recorder();
    beampipe_metrics::set_slurm_ssh_configured(
        beampipe_orchestration::SlurmSshCredentials::try_resolve_ok(),
    );
    let mut worker_pool = None;
    if settings.metrics_server_enabled {
        if let Ok(addr) = settings.metrics_bind_addr.parse() {
            let _ = metrics::server::spawn_metrics_server(addr, Some(pool.clone()));
        }
    }
    if with_worker {
        worker_pool = Some(spawn_workers(
            pool.clone(),
            WorkerConfig::from_settings(&settings),
        ));
    }
    let rate_limiter = RateLimiter::from_settings(&settings).await;
    let bind_addr: SocketAddr = settings.bind_addr.parse()?;
    let app = router(AppState {
        pool,
        settings,
        rate_limiter,
        wasm_host: Arc::new(WasmHost::default()),
    });
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = %bind_addr, "event=api_listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    if let Some(workers) = worker_pool {
        workers.shutdown().await;
    }
    tracing::info!("event=api_shutdown_complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("event=shutdown_signal_received");
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("service unavailable")]
    ServiceUnavailable,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("project config validation failed")]
    Validation(ValidationReport),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("auth error: {0}")]
    Auth(#[from] beampipe_auth::AuthError),
    #[error("project config error: {0}")]
    Project(#[from] beampipe_project::ProjectConfigError),
    #[error("rate limit exceeded")]
    TooManyRequests,
    #[error("wasm error: {0}")]
    Wasm(#[from] beampipe_project::WasmHostError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ApiError::BadRequest(_) | ApiError::Project(_) | ApiError::Validation(_) => {
                StatusCode::BAD_REQUEST
            }
            ApiError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::Auth(_) => StatusCode::UNAUTHORIZED,
            ApiError::Wasm(_) => StatusCode::BAD_REQUEST,
        };
        match self {
            ApiError::Validation(report) => (status, Json(json!(report))).into_response(),
            other => (status, Json(json!({"error": other.to_string()}))).into_response(),
        }
    }
}

#[derive(Clone)]
struct AuthUser(UserRow);

impl AuthUser {
    fn require_superuser(&self) -> Result<(), ApiError> {
        if self.0.is_superuser {
            Ok(())
        } else {
            Err(ApiError::Forbidden("superuser required".into()))
        }
    }
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = Arc::<AppState>::from_ref(state);
        let Some(auth) = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        else {
            return Err(ApiError::Unauthorized(
                "missing Authorization header".into(),
            ));
        };
        let Some(token) = auth.strip_prefix("Bearer ").map(str::trim) else {
            return Err(ApiError::Unauthorized(
                "Authorization must be Bearer token".into(),
            ));
        };
        let claims = beampipe_auth::decode_access_token(token, &state.settings.jwt_secret)?;
        if repo::is_token_blacklisted(&state.pool, &beampipe_auth::token_hash(token)).await? {
            return Err(ApiError::Unauthorized("token revoked".into()));
        }
        let user = repo::get_user_by_username(&state.pool, &claims.sub)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
        Ok(AuthUser(user))
    }
}

async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path().to_string();
    let ip = client_ip(req.headers(), "127.0.0.1");
    if let Err(RateLimitError::Limited) =
        check_rate_limit(&state.rate_limiter, None, &ip, &path).await
    {
        return Err(ApiError::TooManyRequests);
    }
    Ok(next.run(req).await)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}

#[utoipa::path(get, path = "/api/v2/health", tag = "health", responses((status = 200, body = HealthResponse)))]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        service: "beampipe-v2".into(),
    })
}

#[utoipa::path(get, path = "/api/v2/metrics", tag = "health", responses((status = 200), (status = 401)))]
async fn metrics(
    State(state): State<Arc<AppState>>,
    mut parts: Parts,
) -> Result<Response, ApiError> {
    if !state.settings.metrics_public {
        AuthUser::from_request_parts(&mut parts, &state).await?;
    }
    metrics::refresh_gauges_from_pool(&state.pool).await;
    let body = metrics::render_prometheus().unwrap_or_default();
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )
        .body(Body::from(body))
        .unwrap())
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyResponse {
    pub status: String,
    pub service: String,
    pub database: String,
    pub redis: String,
    pub tap_casda: String,
    pub tap_vizier: String,
    pub queue_depth: i64,
    pub jobs_running: i64,
}

#[utoipa::path(get, path = "/api/v2/ready", tag = "health", responses((status = 200, body = ReadyResponse), (status = 503)))]
async fn ready(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<(StatusCode, Json<ReadyResponse>), ApiError> {
    let database = match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => {
            metrics::set_dependency_up("postgres", true);
            "ok".into()
        }
        Err(_) => {
            metrics::set_dependency_up("postgres", false);
            return Err(ApiError::ServiceUnavailable);
        }
    };
    let redis = if state.rate_limiter.enabled() {
        match state.rate_limiter.ping().await {
            Ok(()) => {
                metrics::set_dependency_up("redis", true);
                "ok".into()
            }
            Err(_) => {
                metrics::set_dependency_up("redis", false);
                "error".into()
            }
        }
    } else {
        "not_configured".into()
    };
    let timeout = Duration::from_secs(state.settings.discovery_tap_health_timeout_seconds);
    let casda_url = std::env::var("BEAMPIPE_CASDA_TAP_URL").ok();
    let vizier_url = std::env::var("BEAMPIPE_VIZIER_TAP_URL").ok();
    let tap_report = probe_tap_health(
        casda_url.as_deref().filter(|u| !u.is_empty()),
        vizier_url.as_deref().filter(|u| !u.is_empty()),
        timeout,
    )
    .await;
    let tap_casda = endpoint_status_label(&tap_report.casda);
    let tap_vizier = endpoint_status_label(&tap_report.vizier);
    metrics::set_dependency_up(
        "casda",
        tap_report.casda.reachable || !tap_report.casda.configured,
    );
    metrics::set_dependency_up(
        "vizier",
        tap_report.vizier.reachable || !tap_report.vizier.configured,
    );
    let queue_depth = repo::queue_depth(&state.pool).await?;
    let jobs_running = repo::jobs_running_count(&state.pool).await?;
    metrics::set_jobs_queue_depth(queue_depth);
    metrics::set_jobs_running(jobs_running);
    if let Ok(pending) = repo::workflow_pending_counts_by_module(&state.pool).await {
        for (module, count) in pending {
            metrics::set_workflow_pending_sources(&module, count);
        }
    }
    if let Ok(ages) = repo::max_pending_age_by_module(&state.pool).await {
        for (module, age) in ages {
            metrics::set_pending_age_seconds(&module, age);
        }
    }
    Ok((
        StatusCode::OK,
        Json(ReadyResponse {
            status: "ready".into(),
            service: "beampipe-v2".into(),
            database,
            redis,
            tap_casda,
            tap_vizier,
            queue_depth,
            jobs_running,
        }),
    ))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TapHealthResponse {
    pub casda: String,
    pub vizier: String,
}

#[utoipa::path(get, path = "/api/v2/health/tap", tag = "health")]
async fn health_tap() -> Json<TapHealthResponse> {
    let casda = std::env::var("BEAMPIPE_CASDA_TAP_URL").unwrap_or_default();
    let vizier = std::env::var("BEAMPIPE_VIZIER_TAP_URL").unwrap_or_default();
    let timeout = Duration::from_secs(
        Settings::from_env()
            .map(|s| s.discovery_tap_health_timeout_seconds)
            .unwrap_or(10),
    );
    let report = probe_tap_health(
        (!casda.is_empty()).then_some(casda.as_str()),
        (!vizier.is_empty()).then_some(vizier.as_str()),
        timeout,
    )
    .await;
    Json(TapHealthResponse {
        casda: endpoint_status_label(&report.casda),
        vizier: endpoint_status_label(&report.vizier),
    })
}

fn endpoint_status_label(status: &beampipe_adapters::TapEndpointStatus) -> String {
    if !status.configured {
        "not_configured".into()
    } else if status.reachable {
        "ok".into()
    } else {
        "error".into()
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    pub refresh_token: Option<String>,
}

#[utoipa::path(post, path = "/api/v2/refresh", tag = "auth")]
async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let _ = repo::cleanup_expired_blacklisted_tokens(&state.pool).await;
    let Some(token) = req.refresh_token else {
        return Err(ApiError::BadRequest("refresh_token required".into()));
    };
    if repo::is_token_blacklisted(&state.pool, &beampipe_auth::token_hash(&token)).await? {
        return Err(ApiError::BadRequest("token revoked".into()));
    }
    let claims = beampipe_auth::decode_refresh_token(&token, &state.settings.jwt_secret)?;
    let exp = chrono::DateTime::<Utc>::from_timestamp(claims.exp as i64, 0).unwrap_or_else(|| {
        Utc::now() + chrono::Duration::days(state.settings.refresh_token_expire_days)
    });
    repo::blacklist_token(&state.pool, &beampipe_auth::token_hash(&token), exp).await?;
    let pair = beampipe_auth::issue_token_pair(
        &claims.sub,
        &state.settings.jwt_secret,
        state.settings.access_token_expire_minutes,
        state.settings.refresh_token_expire_days,
    )?;
    Ok(Json(TokenResponse {
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
        token_type: pair.token_type,
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LogoutRequest {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

#[utoipa::path(post, path = "/api/v2/logout", tag = "auth")]
async fn logout(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LogoutRequest>,
) -> Result<StatusCode, ApiError> {
    let _ = repo::cleanup_expired_blacklisted_tokens(&state.pool).await;
    let exp = chrono::Utc::now() + chrono::Duration::days(state.settings.refresh_token_expire_days);
    if let Some(token) = req.access_token {
        repo::blacklist_token(&state.pool, &beampipe_auth::token_hash(&token), exp).await?;
    }
    if let Some(token) = req.refresh_token {
        repo::blacklist_token(&state.pool, &beampipe_auth::token_hash(&token), exp).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v2/executions", tag = "executions")]
async fn list_executions(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Query(query): Query<ListExecutionsQuery>,
) -> Result<Json<repo::PaginatedExecutions>, ApiError> {
    Ok(Json(
        repo::list_executions(
            &state.pool,
            query.project_module.as_deref(),
            query.status.as_deref(),
            query.page.unwrap_or(1),
            query.items_per_page.unwrap_or(50),
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct ListExecutionsQuery {
    pub project_module: Option<String>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub items_per_page: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProvenanceSummary {
    pub config_version: Option<i32>,
    pub discovery_signature: Option<String>,
    pub recent_events: Vec<observability::ProvenanceEventResponse>,
}

#[derive(Debug, Deserialize)]
pub struct LedgerSnapshotQuery {
    #[serde(default = "default_true")]
    pub include_manifest: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LedgerSnapshotResponse {
    pub uuid: Uuid,
    pub status: String,
    pub execution_phase: Option<String>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_manifest: Option<Value>,
    pub last_error: Option<String>,
    pub run_record_phases: Value,
    pub provenance_summary: ProvenanceSummary,
}

#[utoipa::path(
    get,
    path = "/api/v2/executions/{id}/ledger-snapshot",
    tag = "executions"
)]
async fn execution_ledger_snapshot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(query): Query<LedgerSnapshotQuery>,
) -> Result<Json<LedgerSnapshotResponse>, ApiError> {
    let row = repo::get_execution(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let run_record = row
        .workflow_manifest
        .as_ref()
        .and_then(beampipe_domain::run_record::extract_beampipe_run_record);
    let run_record_phases =
        beampipe_domain::run_record::summarize_run_record_phases(run_record.as_ref());
    let config_version = repo::get_active_project_config(&state.pool, &row.project_module)
        .await?
        .map(|c| c.version);
    let source_id = row
        .sources
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str());
    let discovery_signature = if let Some(sid) = source_id {
        repo::get_source_by_identifier(&state.pool, &row.project_module, sid)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.discovery_signature)
    } else {
        None
    };
    let recent_events = repo::list_provenance_events_for_execution(&state.pool, id, 5)
        .await?
        .into_iter()
        .map(observability::ProvenanceEventResponse::from)
        .collect();
    Ok(Json(LedgerSnapshotResponse {
        uuid: row.uuid,
        status: row.status,
        execution_phase: row.execution_phase,
        scheduler_name: row.scheduler_name,
        scheduler_job_id: row.scheduler_job_id,
        workflow_manifest: if query.include_manifest {
            row.workflow_manifest.map(|v| redact_value(&v))
        } else {
            None
        },
        last_error: row.last_error.map(|e| redact_string(&e)),
        run_record_phases: redact_value(&run_record_phases),
        provenance_summary: ProvenanceSummary {
            config_version,
            discovery_signature,
            recent_events,
        },
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectListItem {
    pub project_id: String,
    pub version: i32,
    pub active: bool,
}

#[utoipa::path(get, path = "/api/v2/projects", tag = "project-configs")]
async fn list_projects(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<ProjectListItem>>, ApiError> {
    let rows = repo::list_active_project_configs(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| ProjectListItem {
                project_id: r.project_id,
                version: r.version,
                active: r.active,
            })
            .collect(),
    ))
}

#[utoipa::path(get, path = "/api/v2/projects/contracts", tag = "project-configs")]
async fn list_project_contracts(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<ValidationReport>>, ApiError> {
    let rows = repo::list_active_project_configs(&state.pool).await?;
    let mut reports = Vec::new();
    for row in rows {
        if let Ok(cfg) = serde_json::from_value::<ProjectConfig>(row.spec) {
            reports.push(cfg.validate_report());
        }
    }
    Ok(Json(reports))
}

#[utoipa::path(get, path = "/api/v2/projects/contracts/{id}", tag = "project-configs")]
async fn get_project_contract(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ValidationReport>, ApiError> {
    let row = repo::get_active_project_config(&state.pool, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let cfg: ProjectConfig =
        serde_json::from_value(row.spec).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(cfg.validate_report()))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
}

#[utoipa::path(post, path = "/api/v2/login", tag = "auth", request_body = LoginRequest, responses((status = 200, body = TokenResponse), (status = 401)))]
async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let user = repo::get_user_by_username(&state.pool, &req.username).await?;
    let Some(user) = user else {
        return Err(ApiError::BadRequest("invalid username or password".into()));
    };
    if !beampipe_auth::verify_password(&req.password, &user.hashed_password) {
        return Err(ApiError::BadRequest("invalid username or password".into()));
    }
    let pair = beampipe_auth::issue_token_pair(
        &user.username,
        &state.settings.jwt_secret,
        state.settings.access_token_expire_minutes,
        state.settings.refresh_token_expire_days,
    )?;
    Ok(Json(TokenResponse {
        access_token: pair.access_token,
        refresh_token: pair.refresh_token,
        token_type: pair.token_type,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CurrentUserResponse {
    pub uuid: Uuid,
    pub name: String,
    pub username: String,
    pub email: String,
    pub profile_image_url: String,
    pub is_superuser: bool,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: Option<chrono::DateTime<Utc>>,
}

impl From<UserRow> for CurrentUserResponse {
    fn from(user: UserRow) -> Self {
        Self {
            uuid: user.uuid,
            name: user.name,
            username: user.username,
            email: user.email,
            profile_image_url: user.profile_image_url,
            is_superuser: user.is_superuser,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

#[utoipa::path(get, path = "/api/v2/user/me", tag = "auth", responses((status = 200, body = CurrentUserResponse), (status = 401)))]
async fn current_user(AuthUser(user): AuthUser) -> Json<CurrentUserResponse> {
    Json(user.into())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SourceCreate {
    pub project_module: String,
    pub source_identifier: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SourceBulkCreate {
    pub items: Vec<SourceCreate>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceBulkCreateResponse {
    pub items: Vec<SourceRegistryRow>,
    pub total: usize,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SourceUpdate {
    pub enabled: Option<bool>,
    pub stale_after_hours: Option<i32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SourceMetadataResponse {
    pub source: SourceRegistryRow,
    pub metadata: Vec<ArchiveMetadataResponse>,
    pub metadata_count: usize,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ArchiveMetadataResponse {
    pub uuid: Uuid,
    pub project_module: String,
    pub source_identifier: String,
    pub sbid: String,
    pub metadata_json: Option<Value>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: Option<chrono::DateTime<Utc>>,
}

impl From<ArchiveMetadataRow> for ArchiveMetadataResponse {
    fn from(row: ArchiveMetadataRow) -> Self {
        Self {
            uuid: row.uuid,
            project_module: row.project_module,
            source_identifier: row.source_identifier,
            sbid: row.sbid,
            metadata_json: row.metadata_json.map(|v| redact_value(&v)),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DiscoverTriggerRequest {
    pub project_module: String,
    pub source_identifier: Option<String>,
    pub source_identifiers: Option<Vec<String>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DiscoverTriggerResponse {
    pub project_module: String,
    pub marked_count: usize,
    pub source_identifiers: Vec<String>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct ListSourcesQuery {
    pub project_module: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[utoipa::path(post, path = "/api/v2/sources", tag = "sources", request_body = SourceCreate, responses((status = 200)))]
async fn create_source(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Json(req): Json<SourceCreate>,
) -> Result<Json<SourceRegistryRow>, ApiError> {
    Ok(Json(
        repo::upsert_source(
            &state.pool,
            &req.project_module,
            &req.source_identifier,
            req.enabled,
        )
        .await?,
    ))
}

#[utoipa::path(post, path = "/api/v2/sources/bulk", tag = "sources", request_body = SourceBulkCreate, responses((status = 200)))]
async fn bulk_create_sources(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Json(req): Json<SourceBulkCreate>,
) -> Result<Json<SourceBulkCreateResponse>, ApiError> {
    let mut items = Vec::with_capacity(req.items.len());
    for item in req.items {
        items.push(
            repo::upsert_source(
                &state.pool,
                &item.project_module,
                &item.source_identifier,
                item.enabled,
            )
            .await?,
        );
    }
    Ok(Json(SourceBulkCreateResponse {
        total: items.len(),
        items,
    }))
}

#[utoipa::path(post, path = "/api/v2/sources/discover", tag = "sources", request_body = DiscoverTriggerRequest, responses((status = 200), (status = 400)))]
async fn discover_sources(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Json(req): Json<DiscoverTriggerRequest>,
) -> Result<Json<DiscoverTriggerResponse>, ApiError> {
    if req.source_identifier.is_some() && req.source_identifiers.is_some() {
        return Err(ApiError::BadRequest(
            "Provide only one of source_identifier or source_identifiers".into(),
        ));
    }
    let ids = match (req.source_identifier, req.source_identifiers) {
        (Some(one), None) => Some(vec![one]),
        (None, Some(many)) => Some(many),
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!(),
    };
    let marked =
        repo::mark_sources_for_rediscovery(&state.pool, &req.project_module, ids.as_deref())
            .await?;
    Ok(Json(DiscoverTriggerResponse {
        project_module: req.project_module,
        marked_count: marked.len(),
        source_identifiers: marked,
        message: "Sources marked for rediscovery. Discovery runs asynchronously via the background scheduler.".into(),
    }))
}

#[utoipa::path(get, path = "/api/v2/sources", tag = "sources", responses((status = 200)))]
async fn list_sources(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Query(query): Query<ListSourcesQuery>,
) -> Result<Json<Vec<SourceRegistryRow>>, ApiError> {
    Ok(Json(
        repo::list_sources(
            &state.pool,
            query.project_module.as_deref(),
            query.limit.unwrap_or(100).clamp(1, 500),
            query.offset.unwrap_or(0).max(0),
        )
        .await?,
    ))
}

#[utoipa::path(get, path = "/api/v2/sources/{id}", tag = "sources", responses((status = 200), (status = 404)))]
async fn get_source(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<SourceRegistryRow>, ApiError> {
    repo::get_source(&state.pool, id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[utoipa::path(get, path = "/api/v2/sources/{id}/status", tag = "sources", responses((status = 200, body = SourceExecutionStatus), (status = 404)))]
async fn get_source_status(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<SourceExecutionStatus>, ApiError> {
    let source = repo::get_source(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let metadata_rows = repo::list_source_metadata(&state.pool, &source).await?;
    let metadata: Vec<ArchiveMetadataReadiness> = metadata_rows
        .iter()
        .map(|r| ArchiveMetadataReadiness {
            sbid: r.sbid.clone(),
            metadata_json: r.metadata_json.clone(),
        })
        .collect();
    Ok(Json(source_execution_status(
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
    )))
}

#[utoipa::path(patch, path = "/api/v2/sources/{id}", tag = "sources", request_body = SourceUpdate, responses((status = 200), (status = 404)))]
async fn update_source(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SourceUpdate>,
) -> Result<Json<SourceRegistryRow>, ApiError> {
    repo::update_source(&state.pool, id, req.enabled, req.stale_after_hours)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[utoipa::path(delete, path = "/api/v2/sources/{id}", tag = "sources", responses((status = 204), (status = 404)))]
async fn delete_source(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    if repo::delete_source(&state.pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[utoipa::path(get, path = "/api/v2/sources/{id}/metadata", tag = "sources", responses((status = 200), (status = 404)))]
async fn get_source_metadata(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<SourceMetadataResponse>, ApiError> {
    let source = repo::get_source(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let metadata = repo::list_source_metadata(&state.pool, &source).await?;
    Ok(Json(SourceMetadataResponse {
        metadata_count: metadata.len(),
        source,
        metadata: metadata
            .into_iter()
            .map(ArchiveMetadataResponse::from)
            .collect(),
    }))
}

#[utoipa::path(get, path = "/api/v2/sources/{id}/executions", tag = "sources", responses((status = 200), (status = 404)))]
async fn list_source_executions(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
    Query(query): Query<ListSourcesQuery>,
) -> Result<Json<Vec<ExecutionRow>>, ApiError> {
    let source = repo::get_source(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(
        repo::list_executions_for_source(
            &state.pool,
            &source,
            query.limit.unwrap_or(100).clamp(1, 500),
            query.offset.unwrap_or(0).max(0),
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExecutionCreate {
    pub project_module: String,
    pub sources: Vec<Value>,
    pub archive_name: String,
    pub deployment_profile_id: Option<Uuid>,
    pub deployment_profile_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionRead {
    pub uuid: Uuid,
    pub project_module: String,
    pub archive_name: String,
    pub sources: Value,
    pub status: String,
    pub execution_phase: Option<String>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub workflow_manifest: Option<Value>,
    pub beampipe_run_record: Option<Value>,
    pub last_error: Option<String>,
    pub retry_count: i32,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub completed_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
    pub created_by_id: Option<i32>,
    pub deployment_profile_id: Option<Uuid>,
    pub project_config_id: Option<Uuid>,
    pub project_config_version: Option<i32>,
    #[serde(flatten)]
    pub debug_urls: ExecutionDebugUrls,
}

#[derive(Debug, Serialize, ToSchema, Default)]
pub struct ExecutionDebugUrls {
    pub dim_session_status_url: Option<String>,
    pub dim_graph_status_url: Option<String>,
    pub slurm_session_dir: Option<String>,
    pub slurm_login_node: Option<String>,
    pub slurm_remote_user: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecuteResponse {
    pub status: String,
    pub execution_id: Uuid,
    pub job_id: Uuid,
    pub do_stage: bool,
    pub do_submit: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionPrepareResponse {
    pub project_module: String,
    pub valid: bool,
    pub errors: Vec<String>,
    pub total_datasets: usize,
    pub sources_preview: Vec<Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionStatusResponse {
    pub uuid: Uuid,
    pub status: String,
    pub execution_phase: Option<String>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub last_error: Option<String>,
    pub retry_count: i32,
    pub started_at: Option<chrono::DateTime<Utc>>,
    pub completed_at: Option<chrono::DateTime<Utc>>,
    pub slurm_state: Option<String>,
    pub dim_state: Option<String>,
    pub last_observation_at: Option<chrono::DateTime<Utc>>,
    pub duration_seconds: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExecutionSummaryResponse {
    pub uuid: Uuid,
    pub project_module: String,
    pub archive_name: String,
    pub status: String,
    pub requested_source_count: usize,
    pub requested_source_identifiers: Vec<String>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub last_error: Option<String>,
}

#[utoipa::path(post, path = "/api/v2/executions/prepare", tag = "executions", request_body = ExecutionCreate, responses((status = 200)))]
async fn prepare_execution(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Json(req): Json<ExecutionCreate>,
) -> Result<Json<ExecutionPrepareResponse>, ApiError> {
    let sids = source_identifiers_from_values(&req.sources);
    let rows =
        repo::list_archive_metadata_for_sources(&state.pool, &req.project_module, &sids).await?;
    let mut errors = Vec::new();
    let mut preview = Vec::new();
    let mut total_datasets = 0usize;

    for sid in sids {
        let source = sqlx::query_as::<_, SourceRegistryRow>(
            "SELECT * FROM source_registry WHERE project_module = $1 AND source_identifier = $2",
        )
        .bind(&req.project_module)
        .bind(&sid)
        .fetch_optional(&state.pool)
        .await?;
        let reg = source.as_ref().map(|s| RegisteredSourceReadiness {
            enabled: s.enabled,
            last_checked_at_present: s.last_checked_at.is_some(),
            discovery_signature: s.discovery_signature.clone(),
            discovery_claim_token: s.discovery_claim_token.clone(),
        });
        let metadata: Vec<ArchiveMetadataReadiness> = rows
            .iter()
            .filter(|r| r.source_identifier == sid)
            .map(|r| ArchiveMetadataReadiness {
                sbid: r.sbid.clone(),
                metadata_json: r.metadata_json.clone(),
            })
            .collect();
        if let Some(err) = parsed_source_readiness_error(&sid, None, reg.as_ref(), &metadata) {
            errors.push(err);
            continue;
        }
        let dataset_count = metadata
            .iter()
            .filter_map(|m| m.metadata_json.as_ref())
            .map(dataset_count_from_metadata_json)
            .sum::<usize>();
        total_datasets += dataset_count;
        preview.push(json!({
            "source_identifier": sid,
            "sbid_count": metadata.len(),
            "dataset_count": dataset_count,
        }));
    }

    Ok(Json(ExecutionPrepareResponse {
        project_module: req.project_module,
        valid: errors.is_empty(),
        errors,
        total_datasets,
        sources_preview: preview,
    }))
}

#[utoipa::path(post, path = "/api/v2/executions", tag = "executions", request_body = ExecutionCreate, responses((status = 201)))]
async fn create_execution(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<Arc<correlation::RequestContext>>,
    AuthUser(user): AuthUser,
    Json(req): Json<ExecutionCreate>,
) -> Result<(StatusCode, Json<ExecutionRead>), ApiError> {
    let deployment_profile_id = if let Some(id) = req.deployment_profile_id {
        Some(id)
    } else if let Some(name) = req.deployment_profile_name.as_deref() {
        repo::get_deployment_profile_by_name(&state.pool, name)
            .await?
            .map(|p| p.uuid)
    } else {
        None
    };
    let project_config_id = repo::get_active_project_config(&state.pool, &req.project_module)
        .await?
        .map(|c| c.uuid);
    let row = repo::create_execution_with_correlation(
        &state.pool,
        &req.project_module,
        Value::Array(req.sources),
        &req.archive_name,
        deployment_profile_id,
        project_config_id,
        Some(user.id),
        Some(ctx.correlation_id()),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(enrich_execution(&state.pool, row).await?),
    ))
}

#[utoipa::path(get, path = "/api/v2/executions/{id}", tag = "executions", responses((status = 200), (status = 404)))]
async fn get_execution(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ExecutionRead>, ApiError> {
    let row = repo::get_execution(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(enrich_execution(&state.pool, row).await?))
}

async fn enrich_execution(pool: &PgPool, row: ExecutionRow) -> Result<ExecutionRead, ApiError> {
    let beampipe_run_record = row
        .workflow_manifest
        .as_ref()
        .and_then(beampipe_domain::run_record::extract_beampipe_run_record)
        .map(|v| redact_value(&v));
    let debug_urls = execution_debug_urls(pool, &row).await?;
    let project_config_version = if let Some(id) = row.project_config_id {
        repo::get_project_config_by_uuid(pool, id)
            .await?
            .map(|c| c.version)
    } else {
        None
    };
    Ok(ExecutionRead {
        uuid: row.uuid,
        project_module: row.project_module,
        archive_name: row.archive_name,
        sources: row.sources,
        status: row.status,
        execution_phase: row.execution_phase,
        scheduler_name: row.scheduler_name.clone(),
        scheduler_job_id: row.scheduler_job_id.clone(),
        workflow_manifest: row.workflow_manifest.map(|v| redact_value(&v)),
        beampipe_run_record,
        last_error: row.last_error.map(|e| redact_string(&e)),
        retry_count: row.retry_count,
        started_at: row.started_at,
        completed_at: row.completed_at,
        created_at: row.created_at,
        created_by_id: row.created_by_id,
        deployment_profile_id: row.deployment_profile_id,
        project_config_id: row.project_config_id,
        project_config_version,
        debug_urls,
    })
}

async fn execution_debug_urls(
    pool: &PgPool,
    row: &ExecutionRow,
) -> Result<ExecutionDebugUrls, ApiError> {
    let Some(scheduler_name) = row.scheduler_name.as_deref() else {
        return Ok(ExecutionDebugUrls::default());
    };
    if scheduler_name != "daliuge" && scheduler_name != "slurm" {
        return Ok(ExecutionDebugUrls::default());
    }
    let profile = match row.deployment_profile_id {
        Some(id) => repo::get_deployment_profile(pool, id).await?,
        None => repo::get_default_deployment_profile(pool, &row.project_module).await?,
    };
    if scheduler_name == "daliuge" {
        if let Some(ref profile) = profile {
            if let Ok(beampipe_profiles::DeploymentConfig::RestRemote(rest)) =
                serde_json::from_value(profile.deployment.clone())
            {
                if let (Some(host), Some(port)) = (rest.deploy_host, rest.deploy_port) {
                    let base = beampipe_orchestration::dim::dim_rest_http_base(&host, port);
                    let sid = row.scheduler_job_id.clone().unwrap_or_default();
                    let urls =
                        beampipe_orchestration::dim::dim_operator_urls_from_base(&base, &sid);
                    return Ok(ExecutionDebugUrls {
                        dim_session_status_url: urls
                            .get("dim_session_status_url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        dim_graph_status_url: urls
                            .get("dim_graph_status_url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        ..Default::default()
                    });
                }
            }
        }
    }
    if scheduler_name == "slurm" {
        let sid = row.scheduler_job_id.clone().unwrap_or_default();
        let parsed = beampipe_domain::slurm::parse_scheduler_job_id(&sid);
        let mut urls = ExecutionDebugUrls {
            slurm_session_dir: parsed.session_dir,
            ..Default::default()
        };
        if let Some(ref profile) = profile {
            if let Ok(beampipe_profiles::DeploymentConfig::SlurmRemote(slurm)) =
                serde_json::from_value(profile.deployment.clone())
            {
                urls.slurm_login_node = Some(slurm.login_node);
                urls.slurm_remote_user = slurm.remote_user;
            }
        }
        return Ok(urls);
    }
    Ok(ExecutionDebugUrls::default())
}

#[utoipa::path(get, path = "/api/v2/executions/{id}/status", tag = "executions", responses((status = 200), (status = 404)))]
async fn execution_status(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ExecutionStatusResponse>, ApiError> {
    let row = repo::get_execution(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ExecutionStatusResponse {
        uuid: row.uuid,
        status: row.status.clone(),
        execution_phase: row.execution_phase.clone(),
        scheduler_name: row.scheduler_name.clone(),
        scheduler_job_id: row.scheduler_job_id.clone(),
        last_error: row.last_error.clone().map(|e| redact_string(&e)),
        retry_count: row.retry_count,
        started_at: row.started_at,
        completed_at: row.completed_at,
        slurm_state: observed_slurm_state(&row),
        dim_state: observed_dim_state(&row),
        last_observation_at: last_observation_at(&row),
        duration_seconds: duration_seconds(&row),
    }))
}

#[utoipa::path(get, path = "/api/v2/executions/{id}/summary", tag = "executions", responses((status = 200), (status = 404)))]
async fn execution_summary(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ExecutionSummaryResponse>, ApiError> {
    let row = repo::get_execution(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let source_ids = source_identifiers_from_json(&row.sources);
    Ok(Json(ExecutionSummaryResponse {
        uuid: row.uuid,
        project_module: row.project_module,
        archive_name: row.archive_name,
        status: row.status,
        requested_source_count: source_ids.len(),
        requested_source_identifiers: source_ids,
        scheduler_name: row.scheduler_name,
        scheduler_job_id: row.scheduler_job_id,
        last_error: row.last_error.map(|e| redact_string(&e)),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExecutionPatchRequest {
    pub status: Option<ExecutionStatus>,
    pub scheduler_name: Option<String>,
    pub scheduler_job_id: Option<String>,
    pub workflow_manifest: Option<Value>,
    pub last_error: Option<String>,
}

#[utoipa::path(patch, path = "/api/v2/executions/{id}", tag = "executions", request_body = ExecutionPatchRequest, responses((status = 200), (status = 404)))]
async fn patch_execution(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<ExecutionPatchRequest>,
) -> Result<Json<ExecutionRead>, ApiError> {
    if req.status == Some(ExecutionStatus::Cancelled) {
        let execution = repo::get_execution(&state.pool, id)
            .await?
            .ok_or(ApiError::NotFound)?;
        if matches!(
            execution.status_enum(),
            Some(ExecutionStatus::AwaitingScheduler) | Some(ExecutionStatus::Running)
        ) {
            cancel_execution_scheduler(&state.pool, id).await?;
        }
    }
    let patch = LedgerPatch {
        status: req.status,
        scheduler_name: req.scheduler_name,
        scheduler_job_id: req.scheduler_job_id,
        workflow_manifest: req.workflow_manifest,
        error: req.last_error,
        execution_phase: None,
        clear_error: false,
    };
    let row = repo::apply_execution_patch_with_correlation(&state.pool, id, patch, None)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(enrich_execution(&state.pool, row).await?))
}

async fn cancel_execution_scheduler(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    let Some(execution) = repo::get_execution(pool, id).await? else {
        return Ok(());
    };
    let profile = match execution.deployment_profile_id {
        Some(pid) => repo::get_deployment_profile(pool, pid).await?,
        None => repo::get_default_deployment_profile(pool, &execution.project_module).await?,
    };
    let deployment = profile
        .as_ref()
        .map(|p| p.deployment.clone())
        .unwrap_or(serde_json::json!({}));
    let _ = cancel_scheduler_session(CancelParams {
        scheduler_job_id: execution.scheduler_job_id,
        deployment,
    })
    .await
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ExecuteRequest {
    #[serde(default = "default_true")]
    pub do_stage: bool,
    #[serde(default = "default_true")]
    pub do_submit: bool,
}

fn default_true() -> bool {
    true
}

#[utoipa::path(post, path = "/api/v2/executions/{id}/execute", tag = "executions", responses((status = 202)))]
async fn execute_execution(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<Arc<correlation::RequestContext>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<ExecuteRequest>,
) -> Result<(StatusCode, Json<ExecuteResponse>), ApiError> {
    let payload = json!({
        "execution_id": id,
        "do_stage": req.do_stage,
        "do_submit": req.do_submit,
        "correlation_id": ctx.correlation_id(),
    });
    let job = repo::enqueue_job(
        &state.pool,
        "execute",
        payload,
        Some(id),
        Some(&format!("execute:{id}")),
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ExecuteResponse {
            status: "accepted".into(),
            execution_id: id,
            job_id: job.uuid,
            do_stage: req.do_stage,
            do_submit: req.do_submit,
        }),
    ))
}

#[utoipa::path(post, path = "/api/v2/project-configs", tag = "project-configs", request_body = String, responses((status = 201, body = ValidationReport), (status = 400)))]
async fn upload_project_config(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    body: String,
) -> Result<(StatusCode, Json<ValidationReport>), ApiError> {
    user.require_superuser()?;
    let config = ProjectConfig::from_slice(body.as_bytes())?;
    let previous = repo::get_active_project_config(&state.pool, &config.metadata.id)
        .await?
        .and_then(|row| serde_json::from_value::<ProjectConfig>(row.spec).ok());
    let mut report = config.validate_report_against(previous.as_ref());
    if report.valid {
        let pinned = repo::count_active_executions_with_different_spec(
            &state.pool,
            &config.metadata.id,
            &report.spec_sha256,
        )
        .await?;
        if pinned > 0 {
            report.warnings.push(ValidationDiagnostic::warning(
                "spec_sha256",
                "in_flight_config_pins",
                format!("{pinned} in-flight execution(s) pin a different project config spec_sha256; new config applies to future runs only"),
            ));
        }
    }
    if !report.valid {
        return Err(ApiError::Validation(report));
    }
    let spec = serde_json::to_value(&config).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    reject_inline_secrets_in_production(&state.settings, "project_config", &spec)?;
    repo::insert_project_config(&state.pool, &config.metadata.id, spec, &report.spec_sha256)
        .await?;
    Ok((StatusCode::CREATED, Json(report)))
}

#[utoipa::path(get, path = "/api/v2/project-configs/{id}", tag = "project-configs", responses((status = 200), (status = 404)))]
async fn get_project_config(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<ProjectConfigRow>, ApiError> {
    repo::get_active_project_config(&state.pool, &id)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[utoipa::path(get, path = "/api/v2/project-configs/{id}/versions", tag = "project-configs", responses((status = 200)))]
async fn list_project_config_versions(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<ProjectConfigRow>>, ApiError> {
    Ok(Json(
        repo::list_project_config_versions(&state.pool, &id).await?,
    ))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WasmUploadResponse {
    pub wasm_sha256: String,
    pub project_config_id: Uuid,
}

#[utoipa::path(post, path = "/api/v2/project-configs/{id}/wasm", tag = "project-configs", responses((status = 201, body = WasmUploadResponse)))]
async fn upload_project_config_wasm(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(project_id): Path<String>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<WasmUploadResponse>), ApiError> {
    user.require_superuser()?;
    let config_row = repo::get_active_project_config(&state.pool, &project_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    state.wasm_host.validate_module(&body)?;
    let wasm_sha256 = format!("{:x}", Sha256::digest(&body));
    repo::insert_project_config_wasm(&state.pool, config_row.uuid, &wasm_sha256, &body).await?;
    Ok((
        StatusCode::CREATED,
        Json(WasmUploadResponse {
            wasm_sha256,
            project_config_id: config_row.uuid,
        }),
    ))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WasmMetaResponse {
    pub wasm_sha256: String,
    pub project_config_id: Uuid,
    pub uploaded_at: chrono::DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_len: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/api/v2/project-configs/{id}/wasm/{sha256}",
    tag = "project-configs"
)]
async fn get_project_config_wasm(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path((project_id, sha256)): Path<(String, String)>,
    Query(query): Query<WasmGetQuery>,
) -> Result<Response, ApiError> {
    let config_row = repo::get_active_project_config(&state.pool, &project_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let Some((_, uploaded_at)) =
        repo::get_project_config_wasm_meta(&state.pool, config_row.uuid, &sha256).await?
    else {
        return Err(ApiError::NotFound);
    };
    if query.download.unwrap_or(false) {
        let Some(bytes) =
            repo::get_project_config_wasm(&state.pool, config_row.uuid, &sha256).await?
        else {
            return Err(ApiError::NotFound);
        };
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/wasm")],
            bytes,
        )
            .into_response());
    }
    Ok(Json(WasmMetaResponse {
        wasm_sha256: sha256,
        project_config_id: config_row.uuid,
        uploaded_at,
        bytes_len: None,
    })
    .into_response())
}

#[derive(Debug, Deserialize)]
struct WasmGetQuery {
    download: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct JobCreate {
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
    pub execution_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobResponse {
    pub uuid: Uuid,
    pub kind: String,
    pub payload: Value,
    pub status: String,
    pub execution_id: Option<Uuid>,
    pub phase: Option<String>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub next_run_at: chrono::DateTime<Utc>,
    pub locked_until: Option<chrono::DateTime<Utc>>,
    pub idempotency_key: Option<String>,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: Option<chrono::DateTime<Utc>>,
}

impl From<JobRow> for JobResponse {
    fn from(row: JobRow) -> Self {
        Self {
            uuid: row.uuid,
            kind: row.kind,
            payload: redact_value(&row.payload),
            status: row.status,
            execution_id: row.execution_id,
            phase: row.phase,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            next_run_at: row.next_run_at,
            locked_until: row.locked_until,
            idempotency_key: row.idempotency_key,
            last_error: row.last_error.map(|e| redact_string(&e)),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeploymentProfileResponse {
    pub uuid: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub project_module: Option<String>,
    pub is_default: bool,
    pub translation: Value,
    pub deployment: Value,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: Option<chrono::DateTime<Utc>>,
}

impl From<DeploymentProfileRow> for DeploymentProfileResponse {
    fn from(row: DeploymentProfileRow) -> Self {
        Self {
            uuid: row.uuid,
            name: row.name,
            description: row.description,
            project_module: row.project_module,
            is_default: row.is_default,
            translation: redact_value(&row.translation),
            deployment: redact_value(&row.deployment),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[utoipa::path(post, path = "/api/v2/jobs", tag = "jobs", request_body = JobCreate, responses((status = 202)))]
async fn enqueue_job(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<JobCreate>,
) -> Result<(StatusCode, Json<JobResponse>), ApiError> {
    user.require_superuser()?;
    reject_inline_secrets_in_production(&state.settings, "job_payload", &req.payload)?;
    let job = repo::enqueue_job(
        &state.pool,
        &req.kind,
        req.payload,
        req.execution_id,
        req.idempotency_key.as_deref(),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

#[utoipa::path(post, path = "/api/v2/deployment-profiles", tag = "deployment-profiles", request_body = DeploymentProfile, responses((status = 201)))]
async fn create_deployment_profile(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(profile): Json<DeploymentProfile>,
) -> Result<(StatusCode, Json<DeploymentProfileResponse>), ApiError> {
    user.require_superuser()?;
    profile
        .validate()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    reject_inline_secrets_in_production(
        &state.settings,
        "deployment_profile.translation",
        &serde_json::to_value(&profile.translation).unwrap_or(Value::Null),
    )?;
    reject_inline_secrets_in_production(
        &state.settings,
        "deployment_profile.deployment",
        &serde_json::to_value(&profile.deployment).unwrap_or(Value::Null),
    )?;
    let row = sqlx::query_as::<_, DeploymentProfileRow>(
        r#"
        INSERT INTO daliuge_deployment_profile
            (uuid, name, description, project_module, is_default, translation, deployment)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING *
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(&profile.name)
    .bind(&profile.description)
    .bind(&profile.project_module)
    .bind(profile.is_default)
    .bind(serde_json::to_value(&profile.translation).unwrap_or(Value::Null))
    .bind(serde_json::to_value(&profile.deployment).unwrap_or(Value::Null))
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

#[utoipa::path(get, path = "/api/v2/deployment-profiles", tag = "deployment-profiles", responses((status = 200)))]
async fn list_deployment_profiles(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Json<Vec<DeploymentProfileResponse>>, ApiError> {
    let rows = sqlx::query_as::<_, DeploymentProfileRow>(
        "SELECT * FROM daliuge_deployment_profile ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(DeploymentProfileResponse::from)
            .collect(),
    ))
}

#[utoipa::path(get, path = "/api/v2/deployment-profiles/{id}", tag = "deployment-profiles", responses((status = 200), (status = 404)))]
async fn get_deployment_profile(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<DeploymentProfileResponse>, ApiError> {
    repo::get_deployment_profile(&state.pool, id)
        .await?
        .map(DeploymentProfileResponse::from)
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[utoipa::path(patch, path = "/api/v2/deployment-profiles/{id}", tag = "deployment-profiles", request_body = DeploymentProfile, responses((status = 200), (status = 404)))]
async fn update_deployment_profile(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(profile): Json<DeploymentProfile>,
) -> Result<Json<DeploymentProfileResponse>, ApiError> {
    user.require_superuser()?;
    profile
        .validate()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    reject_inline_secrets_in_production(
        &state.settings,
        "deployment_profile.translation",
        &serde_json::to_value(&profile.translation).unwrap_or(Value::Null),
    )?;
    reject_inline_secrets_in_production(
        &state.settings,
        "deployment_profile.deployment",
        &serde_json::to_value(&profile.deployment).unwrap_or(Value::Null),
    )?;
    let row = sqlx::query_as::<_, DeploymentProfileRow>(
        r#"
        UPDATE daliuge_deployment_profile
        SET name = $2,
            description = $3,
            project_module = $4,
            is_default = $5,
            translation = $6,
            deployment = $7,
            updated_at = now()
        WHERE uuid = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(&profile.name)
    .bind(&profile.description)
    .bind(&profile.project_module)
    .bind(profile.is_default)
    .bind(serde_json::to_value(&profile.translation).unwrap_or(Value::Null))
    .bind(serde_json::to_value(&profile.deployment).unwrap_or(Value::Null))
    .fetch_optional(&state.pool)
    .await?;
    row.map(DeploymentProfileResponse::from)
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[utoipa::path(delete, path = "/api/v2/deployment-profiles/{id}", tag = "deployment-profiles", responses((status = 204), (status = 404)))]
async fn delete_deployment_profile(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    user.require_superuser()?;
    let result = sqlx::query("DELETE FROM daliuge_deployment_profile WHERE uuid = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn source_identifiers_from_values(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(|v| v.get("source_identifier").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn source_identifiers_from_json(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| source_identifiers_from_values(items))
        .unwrap_or_default()
}

fn dataset_count_from_metadata_json(value: &Value) -> usize {
    value
        .get("datasets")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn current_user_response_excludes_hashed_password() {
        let user = UserRow {
            id: 42,
            uuid: Uuid::now_v7(),
            name: "Admin".into(),
            username: "admin".into(),
            email: "admin@example.test".into(),
            hashed_password: "$2b$redacted".into(),
            profile_image_url: "".into(),
            is_deleted: false,
            is_superuser: true,
            created_at: Utc::now(),
            updated_at: None,
            deleted_at: None,
        };
        let value = serde_json::to_value(CurrentUserResponse::from(user)).unwrap();
        assert!(value.get("hashed_password").is_none());
        assert!(value.get("id").is_none());
        assert!(value.get("deleted_at").is_none());
    }
}

fn observed_slurm_state(row: &ExecutionRow) -> Option<String> {
    row.workflow_manifest
        .as_ref()
        .and_then(|m| m.get("beampipe_run_record"))
        .and_then(|rr| rr.get("slurm"))
        .and_then(|s| s.get("last_observation"))
        .and_then(|o| o.get("state"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn observed_dim_state(row: &ExecutionRow) -> Option<String> {
    row.workflow_manifest
        .as_ref()
        .and_then(|m| m.get("beampipe_run_record"))
        .and_then(|rr| rr.get("dim"))
        .and_then(|d| d.get("last_observation"))
        .and_then(|o| o.get("session_state"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn last_observation_at(row: &ExecutionRow) -> Option<chrono::DateTime<Utc>> {
    let slurm = row.workflow_manifest.as_ref().and_then(|m| {
        m.get("beampipe_run_record")
            .and_then(|rr| rr.get("slurm"))
            .and_then(|s| s.get("last_observation"))
    });
    let dim = row.workflow_manifest.as_ref().and_then(|m| {
        m.get("beampipe_run_record")
            .and_then(|rr| rr.get("dim"))
            .and_then(|d| d.get("last_observation"))
    });
    [slurm, dim]
        .into_iter()
        .flatten()
        .filter_map(beampipe_domain::run_record::parse_observed_at)
        .max()
}

fn duration_seconds(row: &ExecutionRow) -> Option<i64> {
    let start = row.started_at?;
    let end = row.completed_at.unwrap_or_else(chrono::Utc::now);
    Some((end - start).num_seconds())
}

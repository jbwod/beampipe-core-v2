use axum::{routing::get, Router};
use sqlx::PgPool;
use std::net::SocketAddr;
use tokio::task::JoinHandle;

async fn metrics_handler() -> ([(axum::http::HeaderName, &'static str); 1], String) {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        crate::render_prometheus().unwrap_or_default(),
    )
}

async fn health_handler() -> &'static str {
    "ok"
}

pub async fn refresh_gauges_from_pool(pool: &PgPool) {
    crate::refresh::refresh_gauges_from_pool(pool).await;
}

/// Spawn a minimal HTTP server exposing GET /metrics and GET /health.
pub fn spawn_metrics_server(bind_addr: SocketAddr, pool: Option<PgPool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/health", get(health_handler));
        let listener = match tokio::net::TcpListener::bind(bind_addr).await {
            Ok(l) => l,
            Err(err) => {
                tracing::error!(%bind_addr, error = %err, "event=metrics_server_bind_failed");
                return;
            }
        };
        tracing::info!(%bind_addr, "event=metrics_server_listening");
        if let Some(pool) = pool {
            let refresh_pool = pool.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
                loop {
                    interval.tick().await;
                    refresh_gauges_from_pool(&refresh_pool).await;
                }
            });
        }
        if axum::serve(listener, app).await.is_err() {
            tracing::error!(%bind_addr, "event=metrics_server_stopped");
        }
    })
}

pub mod models;
pub mod provenance;
pub mod repo;
pub mod test_modules;

use sqlx::{postgres::PgPoolOptions, PgPool};

const DEFAULT_MAX_CONNECTIONS: u32 = 10;

pub fn max_connections_from_env() -> u32 {
    std::env::var("BEAMPIPE_DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_CONNECTIONS)
        .max(1)
}

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    connect_with_max_connections(database_url, max_connections_from_env()).await
}

pub async fn connect_with_max_connections(
    database_url: &str,
    max_connections: u32,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .connect(database_url)
        .await
}

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

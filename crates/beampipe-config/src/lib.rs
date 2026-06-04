use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub database_url: String,
    pub bind_addr: String,
    pub jwt_secret: String,
    pub access_token_expire_minutes: i64,
    pub refresh_token_expire_days: i64,
    pub worker_poll_interval_ms: u64,
    pub worker_lock_seconds: i64,
    pub worker_concurrency: u32,
    pub worker_scheduler_enabled: bool,
    pub db_max_connections: u32,
    pub discovery_source_concurrency: u32,
    pub discovery_tap_health_check_enabled: bool,
    pub discovery_tap_health_timeout_seconds: u64,
    pub shaping_discovery_max_in_flight_batches: i64,
    pub shaping_discovery_max_batches_per_tick: i64,
    pub shaping_execution_max_in_flight_runs: i64,
    pub shaping_queue_max_depth: i64,
    pub shaping_enqueue_pacing_ms: u64,
    pub scheduler_interval_seconds: u64,
    pub redis_url: Option<String>,
    pub rate_limit_requests: u64,
    pub rate_limit_period_seconds: u64,
    pub metrics_bind_addr: String,
    pub metrics_server_enabled: bool,
    pub log_json: bool,
    pub otel_enabled: bool,
    pub otel_endpoint: String,
    pub otel_service_name: String,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid environment variable {name}: {value}")]
    Invalid { name: &'static str, value: String },
}

impl Settings {
    pub fn from_env() -> Result<Self, SettingsError> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: std::env::var("BEAMPIPE_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8080".into()),
            jwt_secret: std::env::var("BEAMPIPE_JWT_SECRET")
                .unwrap_or_else(|_| "secret-key".into()),
            access_token_expire_minutes: parse_or("BEAMPIPE_ACCESS_TOKEN_EXPIRE_MINUTES", 30)?,
            refresh_token_expire_days: parse_or("BEAMPIPE_REFRESH_TOKEN_EXPIRE_DAYS", 7)?,
            worker_poll_interval_ms: parse_or("BEAMPIPE_WORKER_POLL_INTERVAL_MS", 1000)?,
            worker_lock_seconds: parse_or("BEAMPIPE_WORKER_LOCK_SECONDS", 120)?,
            worker_concurrency: parse_or("BEAMPIPE_WORKER_CONCURRENCY", 1_u32)?,
            worker_scheduler_enabled: parse_bool_or("BEAMPIPE_WORKER_SCHEDULER_ENABLED", true)?,
            db_max_connections: parse_or("BEAMPIPE_DB_MAX_CONNECTIONS", 10_u32)?,
            discovery_source_concurrency: parse_or("BEAMPIPE_DISCOVERY_SOURCE_CONCURRENCY", 5_u32)?,
            discovery_tap_health_check_enabled: parse_bool_or(
                "BEAMPIPE_DISCOVERY_TAP_HEALTH_CHECK_ENABLED",
                true,
            )?,
            discovery_tap_health_timeout_seconds: parse_or(
                "BEAMPIPE_DISCOVERY_TAP_HEALTH_TIMEOUT_SECONDS",
                10,
            )?,
            shaping_discovery_max_in_flight_batches: parse_or(
                "BEAMPIPE_SHAPING_DISCOVERY_MAX_IN_FLIGHT_BATCHES",
                4,
            )?,
            shaping_discovery_max_batches_per_tick: parse_or(
                "BEAMPIPE_SHAPING_DISCOVERY_MAX_BATCHES_PER_TICK",
                4,
            )?,
            shaping_execution_max_in_flight_runs: parse_or(
                "BEAMPIPE_SHAPING_EXECUTION_MAX_IN_FLIGHT_RUNS",
                2,
            )?,
            shaping_queue_max_depth: parse_or("BEAMPIPE_SHAPING_QUEUE_MAX_DEPTH", 200)?,
            shaping_enqueue_pacing_ms: parse_or("BEAMPIPE_SHAPING_ENQUEUE_PACING_MS", 0)?,
            scheduler_interval_seconds: parse_or("BEAMPIPE_SCHEDULER_INTERVAL_SECONDS", 60)?,
            redis_url: std::env::var("BEAMPIPE_REDIS_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            rate_limit_requests: parse_or("BEAMPIPE_RATE_LIMIT_REQUESTS", 10)?,
            rate_limit_period_seconds: parse_or("BEAMPIPE_RATE_LIMIT_PERIOD_SECONDS", 3600)?,
            metrics_bind_addr: std::env::var("BEAMPIPE_METRICS_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:9090".into()),
            metrics_server_enabled: parse_bool_or("BEAMPIPE_METRICS_SERVER_ENABLED", true)?,
            log_json: parse_bool_or("BEAMPIPE_LOG_JSON", false)?,
            otel_enabled: parse_bool_or("BEAMPIPE_OTEL_ENABLED", false)?,
            otel_endpoint: std::env::var("BEAMPIPE_OTEL_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:4317".into()),
            otel_service_name: std::env::var("BEAMPIPE_OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "beampipe-v2".into()),
        })
    }
}

fn required(name: &'static str) -> Result<String, SettingsError> {
    std::env::var(name).map_err(|_| SettingsError::Missing(name))
}

fn parse_or<T>(name: &'static str, default: T) -> Result<T, SettingsError>
where
    T: std::str::FromStr,
{
    match std::env::var(name) {
        Ok(v) => v
            .parse::<T>()
            .map_err(|_| SettingsError::Invalid { name, value: v }),
        Err(_) => Ok(default),
    }
}

fn parse_bool_or(name: &'static str, default: bool) -> Result<bool, SettingsError> {
    match std::env::var(name) {
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => Ok(true),
        Ok(v) if v == "0" || v.eq_ignore_ascii_case("false") => Ok(false),
        Ok(v) => Err(SettingsError::Invalid { name, value: v }),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_defaults_true() {
        assert!(parse_bool_or("BEAMPIPE_TEST_BOOL_MISSING_XYZ", true).unwrap());
    }
}

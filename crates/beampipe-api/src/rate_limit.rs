//! Optional Redis sliding-window HTTP rate limiting.

use beampipe_config::Settings;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum RateLimitError {
    #[error("rate limit exceeded")]
    Limited,
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
}

#[derive(Clone)]
pub struct RateLimiter {
    client: Option<ConnectionManager>,
    limit: u64,
    period_seconds: u64,
}

impl RateLimiter {
    pub async fn from_settings(settings: &Settings) -> Self {
        let client = if let Some(url) = settings.redis_url.as_deref() {
            match redis::Client::open(url) {
                Ok(client) => match ConnectionManager::new(client).await {
                    Ok(cm) => Some(cm),
                    Err(err) => {
                        warn!(error = %err, "event=rate_limit_redis_connect_failed");
                        None
                    }
                },
                Err(err) => {
                    warn!(error = %err, "event=rate_limit_redis_invalid_url");
                    None
                }
            }
        } else {
            None
        };
        Self {
            client,
            limit: settings.rate_limit_requests,
            period_seconds: settings.rate_limit_period_seconds,
        }
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    pub async fn ping(&self) -> Result<(), redis::RedisError> {
        if let Some(client) = &self.client {
            let mut conn = client.clone();
            redis::cmd("PING").query_async::<()>(&mut conn).await?;
        }
        Ok(())
    }

    pub async fn check(&self, subject: &str, path: &str) -> Result<(), RateLimitError> {
        let Some(client) = &self.client else {
            return Ok(());
        };
        let window_start = chrono::Utc::now().timestamp() / self.period_seconds as i64;
        let key = format!("ratelimit:{subject}:{path}:{window_start}");
        let mut conn = client.clone();
        let count: u64 = conn.incr(&key, 1).await?;
        if count == 1 {
            let _: () = conn.expire(&key, self.period_seconds as i64).await?;
        }
        if count > self.limit {
            return Err(RateLimitError::Limited);
        }
        Ok(())
    }
}

pub fn sanitize_path(path: &str) -> String {
    path.trim_end_matches('/').to_string()
}

pub fn client_ip(headers: &axum::http::HeaderMap, fallback: &str) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

pub async fn check_rate_limit(
    limiter: &RateLimiter,
    user_id: Option<i32>,
    ip: &str,
    path: &str,
) -> Result<(), RateLimitError> {
    let subject = user_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| ip.to_string());
    limiter.check(&subject, &sanitize_path(path)).await
}

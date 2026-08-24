//! Optional Redis fixed-window HTTP rate limiting.

use beampipe_config::Settings;
use ipnet::IpNet;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::net::IpAddr;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum RateLimitError {
    #[error("rate limit exceeded")]
    Limited,
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("rate limiter configuration error: {0}")]
    Configuration(String),
}

#[derive(Clone)]
pub struct RateLimiter {
    client: Option<ConnectionManager>,
    limit: u64,
    period_seconds: u64,
    fail_closed: bool,
    trusted_proxy_cidrs: Vec<IpNet>,
}

impl RateLimiter {
    pub async fn from_settings(settings: &Settings) -> Result<Self, RateLimitError> {
        if settings.rate_limit_requests == 0 || settings.rate_limit_period_seconds == 0 {
            return Err(RateLimitError::Configuration(
                "request limit and period must both be greater than zero".into(),
            ));
        }
        let fail_closed =
            rate_limiter_required(&settings.beampipe_env, settings.require_rate_limiter);
        let trusted_proxy_cidrs = settings
            .trusted_proxy_cidrs
            .iter()
            .map(|cidr| {
                cidr.parse::<IpNet>().map_err(|_| {
                    RateLimitError::Configuration(format!("invalid trusted proxy CIDR '{cidr}'"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let client = connect_backend(settings.redis_url.as_deref(), fail_closed).await?;
        Ok(Self {
            client,
            limit: settings.rate_limit_requests,
            period_seconds: settings.rate_limit_period_seconds,
            fail_closed,
            trusted_proxy_cidrs,
        })
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    pub fn fail_closed(&self) -> bool {
        self.fail_closed
    }

    pub fn client_ip(&self, headers: &axum::http::HeaderMap, peer: IpAddr) -> IpAddr {
        client_ip(headers, peer, &self.trusted_proxy_cidrs)
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
        let period_seconds = i64::try_from(self.period_seconds).map_err(|_| {
            RateLimitError::Configuration("rate-limit period is larger than i64::MAX".into())
        })?;
        let window_start = chrono::Utc::now().timestamp() / period_seconds;
        let key = format!("ratelimit:{subject}:{path}:{window_start}");
        let mut conn = client.clone();
        let count: u64 = conn.incr(&key, 1).await?;
        if count == 1 {
            let _: () = conn.expire(&key, period_seconds).await?;
        }
        if count > self.limit {
            return Err(RateLimitError::Limited);
        }
        Ok(())
    }
}

fn rate_limiter_required(environment: &str, explicitly_required: bool) -> bool {
    explicitly_required || beampipe_security::is_production_env_name(environment)
}

async fn connect_backend(
    redis_url: Option<&str>,
    fail_closed: bool,
) -> Result<Option<ConnectionManager>, RateLimitError> {
    let Some(url) = redis_url else {
        return if fail_closed {
            Err(RateLimitError::Configuration(
                "a required rate limiter needs BEAMPIPE_REDIS_URL; production always requires a limiter"
                    .into(),
            ))
        } else {
            Ok(None)
        };
    };
    let client = match redis::Client::open(url) {
        Ok(client) => client,
        Err(error) if fail_closed => return Err(RateLimitError::Redis(error)),
        Err(error) => {
            warn!(error = %error, "event=rate_limit_redis_invalid_url");
            return Ok(None);
        }
    };
    match ConnectionManager::new(client).await {
        Ok(connection) => Ok(Some(connection)),
        Err(error) if fail_closed => Err(RateLimitError::Redis(error)),
        Err(error) => {
            warn!(error = %error, "event=rate_limit_redis_connect_failed");
            Ok(None)
        }
    }
}

pub fn sanitize_path(path: &str) -> String {
    path.trim_end_matches('/').to_string()
}

pub fn client_ip(
    headers: &axum::http::HeaderMap,
    peer: IpAddr,
    trusted_proxy_cidrs: &[IpNet],
) -> IpAddr {
    if !trusted_proxy_cidrs
        .iter()
        .any(|network| network.contains(&peer))
    {
        return peer;
    }
    let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    else {
        return peer;
    };
    let mut forwarded = match forwarded
        .split(',')
        .map(str::trim)
        .map(str::parse::<IpAddr>)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(forwarded) if !forwarded.is_empty() => forwarded,
        _ => return peer,
    };
    let mut current = peer;
    while let Some(hop) = forwarded.pop() {
        if !trusted_proxy_cidrs
            .iter()
            .any(|network| network.contains(&current))
        {
            break;
        }
        current = hop;
    }
    current
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn headers(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn untrusted_peer_cannot_spoof_forwarded_address() {
        let trusted = vec!["10.0.0.0/8".parse().unwrap()];
        let peer = "192.0.2.10".parse().unwrap();
        assert_eq!(client_ip(&headers("198.51.100.25"), peer, &trusted), peer);
    }

    #[test]
    fn trusted_proxy_chain_stops_at_first_untrusted_client() {
        let trusted = vec!["10.0.0.0/8".parse().unwrap()];
        let peer = "10.0.0.2".parse().unwrap();
        assert_eq!(
            client_ip(
                &headers("203.0.113.99, 198.51.100.25, 10.0.0.1"),
                peer,
                &trusted,
            ),
            "198.51.100.25".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn malformed_forwarding_chain_falls_back_to_peer() {
        let trusted = vec!["10.0.0.0/8".parse().unwrap()];
        let peer = "10.0.0.2".parse().unwrap();
        assert_eq!(client_ip(&headers("unknown"), peer, &trusted), peer);
    }

    #[tokio::test]
    async fn production_does_not_disable_a_broken_redis_backend() {
        assert!(rate_limiter_required("production", false));
        assert!(rate_limiter_required("prod", false));
        assert!(rate_limiter_required("development", true));
        assert!(!rate_limiter_required("development", false));
        assert!(matches!(
            connect_backend(Some("not-a-redis-url"), true).await,
            Err(RateLimitError::Redis(_))
        ));
        assert!(connect_backend(Some("not-a-redis-url"), false)
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            connect_backend(None, true).await,
            Err(RateLimitError::Configuration(_))
        ));
        assert!(connect_backend(None, false).await.unwrap().is_none());
    }
}

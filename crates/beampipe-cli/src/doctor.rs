use beampipe_adapters::probe_tap_health;
use beampipe_config::Settings;
use beampipe_db::repo;
use beampipe_orchestration::{collect_security_issues, SlurmSshCredentials};
use serde::Serialize;
use sqlx::PgPool;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        name: name.into(),
        ok,
        detail: detail.into(),
    }
}

pub async fn run_doctor(pool: &PgPool, settings: &Settings) -> DoctorReport {
    let mut checks = Vec::new();

    let db_ok = sqlx::query("SELECT 1").execute(pool).await.is_ok();
    checks.push(check(
        "database",
        db_ok,
        if db_ok { "connected" } else { "unreachable" },
    ));

    let migrations_ok = sqlx::query("SELECT 1 FROM _sqlx_migrations LIMIT 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some();
    checks.push(check(
        "migrations",
        migrations_ok,
        if migrations_ok {
            "applied"
        } else {
            "run beampipe migrate"
        },
    ));

    let jwt_ok =
        settings.jwt_secret.len() >= 32 && !settings.jwt_secret.eq_ignore_ascii_case("change-me");
    checks.push(check(
        "jwt_secret",
        jwt_ok,
        if jwt_ok {
            "configured"
        } else {
            "set BEAMPIPE_JWT_SECRET to at least 32 random characters"
        },
    ));

    let security_issues = collect_security_issues(settings);
    checks.push(check(
        "security",
        security_issues.is_empty(),
        if security_issues.is_empty() {
            "ok".into()
        } else {
            security_issues.join("; ")
        },
    ));

    let slurm_ok = SlurmSshCredentials::try_resolve_ok();
    checks.push(check(
        "slurm_ssh",
        slurm_ok,
        if slurm_ok {
            "credentials resolve"
        } else {
            "not configured (optional unless BEAMPIPE_USE_REAL_BACKENDS)"
        },
    ));

    let redis_configured = settings.redis_url.as_deref().is_some_and(|u| !u.is_empty());
    let redis_ok = if redis_configured {
        let url = settings.redis_url.as_deref().unwrap();
        match redis::Client::open(url) {
            Ok(client) => match client.get_multiplexed_async_connection().await {
                Ok(mut conn) => redis::cmd("PING")
                    .query_async::<String>(&mut conn)
                    .await
                    .map(|r| r == "PONG")
                    .unwrap_or(false),
                Err(_) => false,
            },
            Err(_) => false,
        }
    } else {
        true
    };
    checks.push(check(
        "redis",
        redis_ok,
        if !redis_configured {
            "not configured (optional)"
        } else if redis_ok {
            "reachable"
        } else {
            "unreachable"
        },
    ));

    let timeout = Duration::from_secs(settings.discovery_tap_health_timeout_seconds);
    let tap = probe_tap_health(
        std::env::var("BEAMPIPE_CASDA_TAP_URL")
            .ok()
            .as_deref()
            .filter(|u| !u.is_empty()),
        std::env::var("BEAMPIPE_VIZIER_TAP_URL")
            .ok()
            .as_deref()
            .filter(|u| !u.is_empty()),
        timeout,
    )
    .await;
    let tap_ok = (!tap.casda.configured || tap.casda.reachable)
        && (!tap.vizier.configured || tap.vizier.reachable);
    checks.push(check(
        "tap",
        tap_ok,
        format!(
            "casda={} vizier={}",
            if tap.casda.reachable || !tap.casda.configured {
                "ok"
            } else {
                "down"
            },
            if tap.vizier.reachable || !tap.vizier.configured {
                "ok"
            } else {
                "down"
            }
        ),
    ));

    let queue_depth = repo::queue_depth(pool).await.unwrap_or(-1);
    let jobs_running = repo::jobs_running_count(pool).await.unwrap_or(-1);
    checks.push(check(
        "queue",
        queue_depth >= 0,
        format!("depth={queue_depth} running={jobs_running}"),
    ));

    let use_real = std::env::var("BEAMPIPE_USE_REAL_BACKENDS")
        .ok()
        .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes"));
    let ok = checks.iter().all(|c| {
        if c.name == "slurm_ssh" && !use_real {
            true
        } else if c.name == "tap" && !use_real {
            true
        } else {
            c.ok
        }
    });

    DoctorReport { ok, checks }
}

pub async fn run_status(pool: &PgPool) -> serde_json::Value {
    let queue_depth = repo::queue_depth(pool).await.unwrap_or(0);
    let jobs_running = repo::jobs_running_count(pool).await.unwrap_or(0);
    let pending = repo::workflow_pending_counts_by_module(pool)
        .await
        .unwrap_or_default();
    serde_json::json!({
        "queue_depth": queue_depth,
        "jobs_running": jobs_running,
        "workflow_pending_by_module": pending,
    })
}

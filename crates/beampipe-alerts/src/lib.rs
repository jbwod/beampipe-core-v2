use beampipe_db::{models::AlertRuleRow, repo};
use beampipe_security::{
    redact_string, redact_value, resolve_secret_value_strict, SecretPolicy, SecretValue,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AlertError {
    #[error("delivery failed: {0}")]
    Delivery(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertPayload {
    pub alert: String,
    pub severity: String,
    pub project_module: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_identifiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_signature: Option<String>,
    pub links: Value,
    pub fired_at: String,
}

pub async fn fire_alert(
    pool: &PgPool,
    rule: &AlertRuleRow,
    payload: &AlertPayload,
) -> Result<Vec<Uuid>, AlertError> {
    if !rule.enabled {
        return Ok(Vec::new());
    }
    if let Some(last) = rule.last_fired_at {
        let cooldown = Duration::minutes(rule.cooldown_minutes as i64);
        if Utc::now() - last < cooldown {
            return Ok(Vec::new());
        }
    }
    let body = serde_json::to_value(payload).unwrap_or(json!({}));
    let mut delivery_ids = Vec::new();
    let mut any_failed = false;
    let mut failure_details = Vec::new();
    for channel_id in &rule.channel_ids {
        let Some(channel) = repo::get_notification_channel(pool, *channel_id).await? else {
            continue;
        };
        if !channel.enabled {
            continue;
        }
        let result = match channel.kind.as_str() {
            "webhook" => deliver_webhook(&channel.config, &body).await,
            "email" => deliver_email(&channel.config, payload).await,
            other => Err(AlertError::Delivery(format!(
                "unknown channel kind {other}"
            ))),
        };
        let (status, err) = match result {
            Ok(()) => ("sent", None),
            Err(e) => {
                any_failed = true;
                let error = redact_string(&e.to_string());
                failure_details.push(json!({
                    "channel_id": channel.uuid,
                    "error": error,
                }));
                ("failed", Some(error))
            }
        };
        let id = repo::insert_alert_delivery(
            pool,
            Some(rule.uuid),
            Some(channel.uuid),
            status,
            &redact_value(&body),
            err.as_deref(),
        )
        .await?;
        delivery_ids.push(id);
    }
    repo::mark_alert_rule_fired(pool, rule.uuid).await?;
    if delivery_ids.is_empty() && !rule.channel_ids.is_empty() {
        any_failed = true;
        failure_details.push(json!({"error": "no enabled channels delivered"}));
    }
    if any_failed {
        let fail_payload = json!({
            "rule_id": rule.uuid,
            "alert": payload.alert,
            "failures": redact_value(&Value::Array(failure_details)),
            "original": redact_value(&body),
        });
        beampipe_db::provenance::record_provenance_event(
            pool,
            beampipe_domain::provenance::ProvenanceEventType::AlertDeliveryFailed.as_str(),
            &payload.project_module,
            payload.source_identifiers.first().map(String::as_str),
            payload.execution_id,
            Some("system:alerts"),
            None,
            &fail_payload,
        )
        .await;
    }
    if !delivery_ids.is_empty() {
        beampipe_db::provenance::record_provenance_event(
            pool,
            beampipe_domain::provenance::ProvenanceEventType::AlertFired.as_str(),
            &payload.project_module,
            payload.source_identifiers.first().map(String::as_str),
            payload.execution_id,
            Some("system:alerts"),
            None,
            &redact_value(&body),
        )
        .await;
    }
    Ok(delivery_ids)
}

pub async fn fire_immediate_for_trigger(
    pool: &PgPool,
    trigger_kind: &str,
    project_module: &str,
    payload: AlertPayload,
) -> Result<(), AlertError> {
    let rules = repo::list_alert_rules(pool, Some(project_module)).await?;
    for rule in rules {
        if rule.trigger_kind == trigger_kind && rule.enabled {
            if rule
                .project_module
                .as_deref()
                .is_some_and(|m| m != project_module)
            {
                continue;
            }
            fire_alert(pool, &rule, &payload).await?;
        }
    }
    Ok(())
}

async fn deliver_webhook(config: &Value, body: &Value) -> Result<(), AlertError> {
    let url = config
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| AlertError::Delivery("webhook missing url".into()))?;
    let template = config
        .get("template")
        .and_then(Value::as_str)
        .unwrap_or("generic");
    let routing_key =
        resolve_secret_field(config, "routing_key", "routing_key_ref")?.unwrap_or_default();
    let payload = match template {
        "slack" => json!({ "text": format_slack_text(body) }),
        "pagerduty" => json!({
            "routing_key": routing_key,
            "event_action": "trigger",
            "payload": {
                "summary": body.get("summary").and_then(Value::as_str).unwrap_or("beampipe alert"),
                "severity": body.get("severity").and_then(Value::as_str).unwrap_or("warning"),
                "source": "beampipe-v2",
                "custom_details": body
            }
        }),
        _ => body.clone(),
    };
    let client = reqwest::Client::new();
    let mut req = client.post(url).json(&payload);
    if let Some(headers) = config.get("headers").and_then(Value::as_object) {
        for (k, v) in headers {
            if let Some(secret) = resolve_header_value(v)? {
                req = req.header(k, secret.expose());
            }
        }
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AlertError::Delivery(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(AlertError::Delivery(format!("HTTP {}", resp.status())));
    }
    Ok(())
}

fn secret_policy() -> SecretPolicy {
    SecretPolicy::from_process_env()
}

fn resolve_secret_field(
    config: &Value,
    key: &str,
    ref_key: &str,
) -> Result<Option<String>, AlertError> {
    if let Some(value) = config.get(ref_key) {
        return resolve_secret_value_strict(value, secret_policy())
            .map_err(|e| AlertError::Delivery(e.to_string()))
            .map(|v| v.map(|secret| secret.expose().to_string()));
    }
    if let Some(value) = config.get(key) {
        return resolve_secret_value_strict(value, secret_policy())
            .map_err(|e| AlertError::Delivery(e.to_string()))
            .map(|v| v.map(|secret| secret.expose().to_string()));
    }
    Ok(None)
}

fn resolve_header_value(value: &Value) -> Result<Option<SecretValue>, AlertError> {
    resolve_secret_value_strict(value, secret_policy())
        .map_err(|e| AlertError::Delivery(e.to_string()))
}

async fn deliver_email(config: &Value, payload: &AlertPayload) -> Result<(), AlertError> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    let smtp_host = config
        .get("smtp_host")
        .and_then(Value::as_str)
        .ok_or_else(|| AlertError::Delivery("email missing smtp_host".into()))?;
    let from = config
        .get("from")
        .and_then(Value::as_str)
        .unwrap_or("beampipe@localhost");
    let to: Vec<String> = config
        .get("to")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if to.is_empty() {
        return Err(AlertError::Delivery("email missing to[]".into()));
    }
    let port = config.get("port").and_then(Value::as_u64).unwrap_or(587) as u16;
    let subject = format!("[beampipe] {} — {}", payload.severity, payload.summary);
    let body = serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.summary.clone());

    let mut builder = Message::builder()
        .from(from.parse().map_err(|e: lettre::address::AddressError| {
            AlertError::Delivery(format!("invalid from address: {e}"))
        })?)
        .subject(subject);
    for recipient in &to {
        builder = builder.to(recipient
            .parse()
            .map_err(|e: lettre::address::AddressError| {
                AlertError::Delivery(format!("invalid to address: {e}"))
            })?);
    }
    let message = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| AlertError::Delivery(e.to_string()))?;

    let mailer = {
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::relay(smtp_host)
            .map_err(|e| AlertError::Delivery(e.to_string()))?
            .port(port);
        if let (Some(user), Some(pass)) = (
            config.get("user").and_then(Value::as_str),
            resolve_secret_field(config, "password", "password_ref")?,
        ) {
            builder = builder.credentials(Credentials::new(user.to_string(), pass.to_string()));
        }
        builder.build()
    };

    mailer
        .send(message)
        .await
        .map_err(|e| AlertError::Delivery(e.to_string()))?;
    Ok(())
}

fn format_slack_text(body: &Value) -> String {
    format!(
        "*{}* [{}]\n{}",
        body.get("severity")
            .and_then(Value::as_str)
            .unwrap_or("alert"),
        body.get("project_module")
            .and_then(Value::as_str)
            .unwrap_or(""),
        body.get("summary").and_then(Value::as_str).unwrap_or("")
    )
}

pub async fn evaluate_scheduled_rules(pool: &PgPool) -> Result<(), AlertError> {
    let rules = repo::list_alert_rules(pool, None).await?;
    for rule in rules {
        if !rule.enabled {
            continue;
        }
        let module = match rule.project_module.as_deref() {
            Some(m) => m.to_string(),
            None => continue,
        };
        match rule.trigger_kind.as_str() {
            "pending_backlog" => {
                let threshold = rule
                    .trigger_config
                    .get("threshold")
                    .and_then(Value::as_i64)
                    .unwrap_or(50);
                let (count, _) = repo::get_workflow_pending_stats(pool, &module).await?;
                if count >= threshold {
                    let payload = AlertPayload {
                        alert: "pending_backlog".into(),
                        severity: rule.severity.clone(),
                        project_module: module.clone(),
                        summary: format!("{count} sources pending execution for {module}"),
                        execution_id: None,
                        source_identifiers: vec![],
                        discovery_signature: None,
                        links: json!({}),
                        fired_at: Utc::now().to_rfc3339(),
                    };
                    fire_alert(pool, &rule, &payload).await?;
                }
            }
            "pending_stale" => {
                let threshold_secs = rule
                    .trigger_config
                    .get("max_age_seconds")
                    .and_then(Value::as_i64)
                    .unwrap_or(21_600);
                let ages = repo::max_pending_age_by_module(pool).await?;
                if ages
                    .iter()
                    .any(|(m, age)| m == &module && *age >= threshold_secs)
                {
                    let payload = AlertPayload {
                        alert: "pending_stale".into(),
                        severity: rule.severity.clone(),
                        project_module: module.clone(),
                        summary: format!("Pending sources stale > {threshold_secs}s for {module}"),
                        execution_id: None,
                        source_identifiers: vec![],
                        discovery_signature: None,
                        links: json!({}),
                        fired_at: Utc::now().to_rfc3339(),
                    };
                    fire_alert(pool, &rule, &payload).await?;
                }
            }
            "discovery_stall" => {
                let window_mins = rule
                    .trigger_config
                    .get("window_minutes")
                    .and_then(Value::as_i64)
                    .unwrap_or(120);
                let since = Utc::now() - Duration::minutes(window_mins);
                let changed = repo::count_discovery_changed_since(pool, &module, since).await?;
                if changed == 0 {
                    let payload = AlertPayload {
                        alert: "discovery_stall".into(),
                        severity: rule.severity.clone(),
                        project_module: module.clone(),
                        summary: format!(
                            "No discovery.changed events in {window_mins}m for {module}"
                        ),
                        execution_id: None,
                        source_identifiers: vec![],
                        discovery_signature: None,
                        links: json!({}),
                        fired_at: Utc::now().to_rfc3339(),
                    };
                    fire_alert(pool, &rule, &payload).await?;
                }
            }
            "dependency_down" => {
                let dependency = rule
                    .trigger_config
                    .get("dependency")
                    .and_then(Value::as_str)
                    .unwrap_or("postgres");
                let up = match dependency {
                    "postgres" => sqlx::query("SELECT 1").execute(pool).await.is_ok(),
                    _ => true,
                };
                if !up {
                    let payload = AlertPayload {
                        alert: "dependency_down".into(),
                        severity: rule.severity.clone(),
                        project_module: module.clone(),
                        summary: format!("Dependency {dependency} is down"),
                        execution_id: None,
                        source_identifiers: vec![],
                        discovery_signature: None,
                        links: json!({}),
                        fired_at: Utc::now().to_rfc3339(),
                    };
                    fire_alert(pool, &rule, &payload).await?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub async fn send_test_notification(pool: &PgPool, channel_id: Uuid) -> Result<Uuid, AlertError> {
    let channel = repo::get_notification_channel(pool, channel_id)
        .await?
        .ok_or_else(|| AlertError::Delivery("channel not found".into()))?;
    let payload = AlertPayload {
        alert: "test".into(),
        severity: "info".into(),
        project_module: "test".into(),
        summary: "Beampipe test notification".into(),
        execution_id: None,
        source_identifiers: vec![],
        discovery_signature: None,
        links: json!({}),
        fired_at: Utc::now().to_rfc3339(),
    };
    let body = serde_json::to_value(&payload).unwrap_or(json!({}));
    let result = match channel.kind.as_str() {
        "webhook" => deliver_webhook(&channel.config, &body).await,
        "email" => deliver_email(&channel.config, &payload).await,
        other => Err(AlertError::Delivery(format!("unknown kind {other}"))),
    };
    let (status, err) = match result {
        Ok(()) => ("sent", None),
        Err(e) => ("failed", Some(e.to_string())),
    };
    repo::insert_alert_delivery(pool, None, Some(channel_id), status, &body, err.as_deref())
        .await
        .map_err(AlertError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_text_formats() {
        let body = json!({"severity": "critical", "project_module": "wallaby", "summary": "fail"});
        assert!(format_slack_text(&body).contains("critical"));
    }
}

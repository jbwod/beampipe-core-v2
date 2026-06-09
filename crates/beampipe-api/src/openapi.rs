//! OpenAPI metadata and post-processing to match the legacy Beampipe Core spec.

use serde_json::{json, Value};
use utoipa::openapi::security::{Flow, OAuth2, Password, Scopes, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "OAuth2PasswordBearer",
                SecurityScheme::OAuth2(OAuth2::new([Flow::Password(Password::new(
                    "/api/v2/login",
                    Scopes::new(),
                ))])),
            );
        }
    }
}

const DESCRIPTION: &str = r#"Beampipe Core (Rust v2)

## Getting started

1. **Authenticate**: `POST /api/v2/login` with the admin **username or email** and password from setup. Copy `access_token` and click **Authorize** (Bearer token).
2. **List projects**: `GET /api/v2/projects` shows registered project modules.
3. **Register sources**: `POST /api/v2/sources` (returns 201 or 200 if already registered).
4. **Run discovery**: `POST /api/v2/sources/discover` marks sources for async archive polling.
5. **Create execution**: `POST /api/v2/executions/prepare` (preview) then `POST /api/v2/executions`.
6. **Execute**: `POST /api/v2/executions/{id}/execute` enqueues staging/submit on the worker queue.
7. **Poll**: `GET /api/v2/executions/{id}/status` or `.../ledger-snapshot` for operator view.

### Common errors

- **401**: missing or expired token. Log in again.
- **400**: unknown `project_module` (check `GET /api/v2/projects`).
- **404**: source, execution, or deployment profile not found.
- **503**: Postgres or dependency unavailable.

Health: `GET /api/v2/health` (liveness) · `GET /api/v2/ready` (deps).

Interactive docs: `GET /api/v2/docs` (Swagger UI)."#;

/// Build OpenAPI for Swagger UI (struct-level metadata; no JSON-only extensions).
pub fn build_openapi() -> utoipa::openapi::OpenApi {
    let mut doc = ApiDoc::openapi();
    SecurityAddon.modify(&mut doc);
    apply_info(&mut doc);
    doc
}

/// Export as JSON value (for `openapi export` and committed `openapi.json`).
pub fn export_openapi_json() -> Value {
    let mut value = serde_json::to_value(build_openapi()).expect("openapi serializes");
    polish_json(&mut value);
    value
}

fn apply_info(doc: &mut utoipa::openapi::OpenApi) {
    doc.info.title = "Beampipe".into();
    doc.info.description = Some(DESCRIPTION.into());
    doc.info.version = env!("CARGO_PKG_VERSION").into();
    if doc.info.contact.is_none() {
        doc.info.contact = Some(
            utoipa::openapi::ContactBuilder::new()
                .name(Some("Jack Blackwood"))
                .email(Some("23326698@student.uwa.edu.au"))
                .build(),
        );
    }
    if doc.info.license.is_none() {
        doc.info.license = Some(utoipa::openapi::LicenseBuilder::new().name("MIT").build());
    }
    doc.servers = Some(vec![utoipa::openapi::ServerBuilder::new()
        .url("/")
        .description(Some("Current host (API routes are under /api/v2)"))
        .build()]);
}

fn polish_json(spec: &mut Value) {
    spec["openapi"] = json!("3.1.0");
    spec.as_object_mut()
        .expect("openapi root object")
        .insert("x-tagGroups".into(), tag_groups());

    inject_error_detail_schema(spec);
    alias_observability_schemas(spec);
    apply_operation_docs(spec);
    apply_security(spec);
    enrich_error_responses(spec);
    default_success_descriptions(spec);
}

fn tag_groups() -> Value {
    json!([
        {"name": "Authentication", "tags": ["auth"]},
        {
            "name": "Core workflow",
            "tags": ["sources", "executions", "project-configs", "deployment-profiles", "jobs"]
        },
        {"name": "Operations", "tags": ["health", "provenance", "alerts"]}
    ])
}

fn inject_error_detail_schema(spec: &mut Value) {
    let components = spec
        .as_object_mut()
        .and_then(|o| o.entry("components").or_insert(json!({})).as_object_mut());
    let Some(components) = components else {
        return;
    };
    let schemas = components
        .entry("schemas")
        .or_insert(json!({}))
        .as_object_mut()
        .expect("components.schemas object");
    schemas.insert(
        "ErrorDetail".into(),
        json!({
            "type": "object",
            "required": ["detail"],
            "properties": {
                "detail": {
                    "type": "string",
                    "description": "Human-readable error message",
                    "examples": ["Resource not found"]
                }
            }
        }),
    );
}

fn error_ref() -> Value {
    json!({
        "description": "Error response",
        "content": {
            "application/json": {
                "schema": { "$ref": "#/components/schemas/ErrorDetail" }
            }
        }
    })
}

fn apply_security(spec: &mut Value) {
    const PUBLIC: &[&str] = &["/api/v2/health", "/api/v2/health/tap", "/api/v2/login"];
    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for (path, item) in paths.iter_mut() {
        if PUBLIC.contains(&path.as_str()) {
            continue;
        }
        let Some(ops) = item.as_object_mut() else {
            continue;
        };
        for op in ops.values_mut().filter(|v| v.is_object()) {
            op.as_object_mut()
                .expect("operation object")
                .insert("security".into(), json!([{"OAuth2PasswordBearer": []}]));
        }
    }
}

fn alias_observability_schemas(spec: &mut Value) {
    let Some(schemas) = spec
        .pointer_mut("/components/schemas")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for name in [
        "NotificationChannelResponse",
        "AlertDeliveryResponse",
        "ProvenanceEventResponse",
    ] {
        let qualified = format!("observability.{name}");
        if !schemas.contains_key(name) {
            if let Some(schema) = schemas.get(&qualified).cloned() {
                schemas.insert(name.into(), schema);
            }
        }
        if !schemas.contains_key(&qualified) {
            if let Some(schema) = schemas.get(name).cloned() {
                schemas.insert(qualified, schema);
            }
        }
    }
}

fn enrich_error_responses(spec: &mut Value) {
    const PUBLIC: &[&str] = &["/api/v2/health", "/api/v2/health/tap", "/api/v2/login"];
    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for (path, item) in paths.iter_mut() {
        let is_public = PUBLIC.contains(&path.as_str());
        let Some(ops) = item.as_object_mut() else {
            continue;
        };
        for op in ops.values_mut().filter(|v| v.is_object()) {
            let has_body = op.get("requestBody").is_some();
            let Some(responses) = op
                .as_object_mut()
                .and_then(|o| o.entry("responses").or_insert(json!({})).as_object_mut())
            else {
                continue;
            };
            if !is_public {
                responses
                    .entry("401")
                    .or_insert_with(|| error_ref_with_desc("Missing or invalid bearer token"));
                responses
                    .entry("403")
                    .or_insert_with(|| error_ref_with_desc("Authenticated but not permitted"));
            }
            if path.contains("/ready") {
                responses
                    .entry("503")
                    .or_insert_with(|| error_ref_with_desc("One or more dependencies unhealthy"));
            }
            if has_body {
                responses.entry("400").or_insert_with(|| {
                    error_ref_with_desc("Invalid request (e.g. unknown project module)")
                });
            }
        }
    }
}

fn error_ref_with_desc(description: &str) -> Value {
    let mut v = error_ref();
    v["description"] = json!(description);
    v
}

fn default_success_descriptions(spec: &mut Value) {
    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for item in paths.values_mut() {
        let Some(ops) = item.as_object_mut() else {
            continue;
        };
        for op in ops.values_mut().filter(|v| v.is_object()) {
            let Some(responses) = op.get_mut("responses").and_then(Value::as_object_mut) else {
                continue;
            };
            for (code, resp) in responses.iter_mut() {
                if resp.get("description").and_then(|d| d.as_str()) == Some("") {
                    resp["description"] = json!(default_description_for_code(code));
                }
            }
        }
    }
}

fn default_description_for_code(code: &str) -> &'static str {
    match code {
        "200" => "Successful response",
        "201" => "Created",
        "202" => "Accepted",
        "204" => "No content",
        _ => "Response",
    }
}

fn apply_operation_docs(spec: &mut Value) {
    static DOCS: &[(&str, &str, &str, &str)] = &[
        ("get", "/api/v2/health", "Liveness probe", "Return process liveness. Does not check external dependencies."),
        ("get", "/api/v2/ready", "Readiness probe", "Check PostgreSQL and archive TAP connectivity. Returns HTTP 503 when Postgres is down."),
        ("get", "/api/v2/health/tap", "Archive TAP probe", "Probe configured archive TAP endpoints used by discovery."),
        ("get", "/api/v2/metrics", "Prometheus metrics", "DB-refreshed gauges and process counters (may require auth unless BEAMPIPE_METRICS_PUBLIC=true)."),
        ("post", "/api/v2/login", "Login for access token", "OAuth2 password flow; returns JWT bearer access token."),
        ("post", "/api/v2/refresh", "Refresh access token", "Exchange a valid refresh token for a new access token."),
        ("post", "/api/v2/logout", "Logout", "Revoke the current access token."),
        ("get", "/api/v2/user/me", "Read current user", "Return the authenticated user profile."),
        ("get", "/api/v2/projects", "List projects", "Registered project modules with active configuration."),
        ("get", "/api/v2/projects/contracts", "List project contracts", "Discovery contract validation status per module."),
        ("get", "/api/v2/projects/contracts/{id}", "Get project contract", "Contract validation detail for one project module."),
        ("get", "/api/v2/sources", "List sources", "Paginated source registry for a project module."),
        ("post", "/api/v2/sources", "Register source", "Register or upsert a single astronomical source."),
        ("post", "/api/v2/sources/bulk", "Bulk register sources", "Register many sources in one request."),
        ("post", "/api/v2/sources/discover", "Trigger discovery", "Mark sources for async archive discovery jobs."),
        ("get", "/api/v2/sources/{id}", "Get source", "Fetch one source registry row."),
        ("patch", "/api/v2/sources/{id}", "Update source", "Patch enabled flag or registry fields."),
        ("delete", "/api/v2/sources/{id}", "Delete source", "Remove a source from the registry."),
        ("get", "/api/v2/sources/{id}/status", "Source execution status", "Readiness and blockers for execution scheduling."),
        ("get", "/api/v2/sources/{id}/metadata", "Source archive metadata", "Persisted discovery metadata for a source."),
        ("get", "/api/v2/sources/{id}/executions", "List executions for source", "Executions that include this source."),
        ("post", "/api/v2/executions/prepare", "Prepare execution", "Validate sources and preview datasets without creating a ledger row."),
        ("post", "/api/v2/executions", "Create execution", "Create a batch execution ledger record."),
        ("get", "/api/v2/executions", "List executions", "Filter executions by project module and status."),
        ("get", "/api/v2/executions/{id}", "Get execution", "Full execution record including manifest and scheduler fields."),
        ("patch", "/api/v2/executions/{id}", "Update execution", "Patch status or scheduler metadata; cancel when permitted."),
        ("post", "/api/v2/executions/{id}/execute", "Execute execution", "Enqueue staging/submit work on the Postgres job queue."),
        ("get", "/api/v2/executions/{id}/status", "Get execution status", "Status-focused execution view."),
        ("get", "/api/v2/executions/{id}/summary", "Get execution summary", "Progress summary for operators."),
        ("get", "/api/v2/executions/{id}/ledger-snapshot", "Get ledger snapshot", "Compact operator snapshot with provenance summary."),
        ("get", "/api/v2/deployment-profiles", "List deployment profiles", "DALiuGE deployment profiles for a project module."),
        ("post", "/api/v2/deployment-profiles", "Create deployment profile", "Create a translation + deployment profile."),
        ("get", "/api/v2/deployment-profiles/{id}", "Get deployment profile", "Fetch one deployment profile."),
        ("patch", "/api/v2/deployment-profiles/{id}", "Update deployment profile", "Patch profile translation or deployment settings."),
        ("delete", "/api/v2/deployment-profiles/{id}", "Delete deployment profile", "Remove a deployment profile."),
        ("post", "/api/v2/project-configs", "Upload project config", "Upload and validate a versioned survey YAML/JSON config."),
        ("get", "/api/v2/project-configs/{id}", "Get project config", "Fetch active or historical project configuration."),
        ("get", "/api/v2/project-configs/{id}/versions", "List config versions", "Version history for a project module."),
        ("post", "/api/v2/project-configs/{id}/wasm", "Upload WASM module", "Attach optional WASM hooks to a config version."),
        ("get", "/api/v2/project-configs/{id}/wasm", "Get WASM module", "Download WASM bytes for a config version."),
        ("post", "/api/v2/jobs", "Enqueue job", "Enqueue a Postgres-backed background job (operator/debug)."),
        ("get", "/api/v2/notification-channels", "List notification channels", "Alert delivery channels (webhook, email)."),
        ("post", "/api/v2/notification-channels", "Create notification channel", "Register a webhook or SMTP channel."),
        ("patch", "/api/v2/notification-channels/{id}", "Update notification channel", "Patch channel configuration."),
        ("delete", "/api/v2/notification-channels/{id}", "Delete notification channel", "Remove a notification channel."),
        ("post", "/api/v2/notification-channels/{id}/test", "Test notification channel", "Send a test alert delivery."),
        ("get", "/api/v2/alert-rules", "List alert rules", "Configured alert rules."),
        ("post", "/api/v2/alert-rules", "Create alert rule", "Create a new alert rule."),
        ("patch", "/api/v2/alert-rules/{id}", "Update alert rule", "Patch alert rule trigger or channels."),
        ("delete", "/api/v2/alert-rules/{id}", "Delete alert rule", "Remove an alert rule."),
        ("get", "/api/v2/alert-deliveries", "List alert deliveries", "Audit log of alert deliveries."),
        ("get", "/api/v2/executions/{id}/events", "List execution events", "Provenance timeline for one execution."),
        ("get", "/api/v2/sources/{id}/events", "List source events", "Discovery and execution history for a source."),
        ("get", "/api/v2/projects/{module}/events", "List project events", "Paginated provenance feed for a project module."),
    ];

    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for (method, path, summary, description) in DOCS {
        let Some(op) = paths
            .get_mut(*path)
            .and_then(|p| p.get_mut(*method))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        op.insert("summary".into(), json!(summary));
        op.insert("description".into(), json!(description));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn collect_unresolved_schema_refs(spec: &Value) -> Vec<String> {
        let Some(schemas) = spec
            .pointer("/components/schemas")
            .and_then(Value::as_object)
        else {
            return vec!["components.schemas missing".into()];
        };
        let names: HashSet<&str> = schemas.keys().map(String::as_str).collect();
        let mut missing = Vec::new();
        walk_refs(spec, &names, &mut missing);
        missing.sort();
        missing.dedup();
        missing
    }

    fn walk_refs(value: &Value, names: &HashSet<&str>, missing: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(ref_path)) = map.get("$ref") {
                    if let Some(name) = ref_path.strip_prefix("#/components/schemas/") {
                        if !names.contains(name) {
                            missing.push(name.to_string());
                        }
                    }
                }
                for v in map.values() {
                    walk_refs(v, names, missing);
                }
            }
            Value::Array(items) => {
                for v in items {
                    walk_refs(v, names, missing);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn polished_openapi_has_security_and_info() {
        let spec = export_openapi_json();
        assert_eq!(spec["info"]["title"], "Beampipe");
        assert_eq!(spec["openapi"], "3.1.0");
        assert!(spec["components"]["securitySchemes"]["OAuth2PasswordBearer"].is_object());
        assert!(spec["paths"]["/api/v2/sources"]["get"]["security"].is_array());
        assert!(
            spec["paths"]["/api/v2/health"]["get"]["security"].is_null()
                || spec["paths"]["/api/v2/health"]["get"]
                    .get("security")
                    .is_none()
        );
    }

    #[test]
    fn polished_openapi_schema_refs_resolve() {
        let spec = export_openapi_json();
        let missing = collect_unresolved_schema_refs(&spec);
        assert!(missing.is_empty(), "unresolved $ref targets: {missing:?}");
        assert!(spec["components"]["schemas"]["ReadyResponse"].is_object());
    }
}

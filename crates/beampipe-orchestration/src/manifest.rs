use crate::OrchestrationError;
use beampipe_project::expressions::evaluate_expression;
use beampipe_project::{ManifestConfig, ManifestTemplate, ProjectConfig};
use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::collections::BTreeMap;

pub fn build_manifest_from_config(
    config: &ProjectConfig,
    metadata: &[Value],
    exclude_sbids: &[String],
) -> Result<Value, OrchestrationError> {
    build_manifest_from_config_with_staging(config, metadata, exclude_sbids, &json!({}))
}

pub fn build_manifest_from_config_with_staging(
    config: &ProjectConfig,
    metadata: &[Value],
    exclude_sbids: &[String],
    staging: &Value,
) -> Result<Value, OrchestrationError> {
    if let Some(manifest_cfg) = config.manifest.as_ref() {
        build_from_manifest_config(manifest_cfg, metadata, exclude_sbids, staging)
    } else {
        crate::build_wallaby_manifest(metadata)
    }
}

fn build_from_manifest_config(
    cfg: &ManifestConfig,
    metadata: &[Value],
    exclude_sbids: &[String],
    staging: &Value,
) -> Result<Value, OrchestrationError> {
    let mut grouped: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();
    for record in metadata {
        let keys: Vec<String> = cfg
            .group_by
            .iter()
            .map(|key| {
                record
                    .get(key)
                    .map(value_key)
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "unknown".into())
            })
            .collect();
        let group_key = keys.join("\0");
        let sbid_key = record
            .get("sbid")
            .map(value_key)
            .filter(|v| v != "0" && !v.is_empty())
            .unwrap_or_else(|| "0".into());
        if sbid_key == "0" || exclude_sbids.iter().any(|s| s == &sbid_key) {
            continue;
        }
        grouped
            .entry(group_key)
            .or_default()
            .entry(sbid_key)
            .or_default()
            .push(record.clone());
    }

    let mut sources = Vec::new();
    let mut total_datasets = 0usize;
    for (_group, by_sbid) in grouped {
        let first = by_sbid
            .values()
            .flatten()
            .next()
            .cloned()
            .unwrap_or(Value::Null);
        let source_id = first
            .get("source_identifier")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let mut sbids = Vec::new();
        for (sbid, datasets) in by_sbid {
            total_datasets += datasets.len();
            let rendered: Vec<Value> = if cfg.expand_from.as_deref() == Some("metadata") {
                datasets
                    .iter()
                    .map(|record| {
                        if let Some(template) = cfg.dataset_template.as_ref() {
                            render_manifest_template(template, record, staging)
                        } else {
                            record.clone()
                        }
                    })
                    .collect()
            } else {
                datasets.clone()
            };
            sbids.push(json!({"sbid": sbid, "datasets": rendered}));
        }
        let mut source_obj = render_manifest_template(&cfg.source_template, &first, staging);
        if let Some(obj) = source_obj.as_object_mut() {
            obj.insert("source_identifier".into(), json!(source_id));
            obj.insert("sbids".into(), json!(sbids));
        }
        sources.push(source_obj);
    }
    if total_datasets == 0 {
        return Err(OrchestrationError::NoUsableDatasets);
    }
    let mut manifest = json!({"inputs": {}, "sources": sources});
    apply_graph_patches_to_manifest(&mut manifest, cfg, total_datasets);
    Ok(manifest)
}

fn apply_graph_patches_to_manifest(manifest: &mut Value, _cfg: &ManifestConfig, total: usize) {
    manifest["graph_overrides"] = json!({
        "patches": [{
            "match": {"equals": "Scatter/GenericScatterApp/Beam"},
            "fields": [{"name": "num_of_copies", "value": total}]
        }]
    });
}

pub fn apply_project_graph_patches(manifest: &mut Value, config: &ProjectConfig) {
    if config.graph_patches.is_empty() {
        return;
    }
    let expr_ctx = graph_patch_expression_context(manifest);
    let patches: Vec<Value> = config
        .graph_patches
        .iter()
        .filter_map(|patch| {
            let equals = patch.r#match.equals.as_str();
            if equals.trim().is_empty() {
                return None;
            }
            let mut fields = Vec::new();
            for (name, raw_value) in &patch.set {
                let value = if let Some(s) = raw_value.as_str() {
                    evaluate_expression(s, &expr_ctx)
                        .unwrap_or_else(|| raw_value.as_value().clone())
                } else {
                    raw_value.as_value().clone()
                };
                fields.push(json!({"name": name, "value": value}));
            }
            Some(json!({"match": {"equals": equals}, "fields": fields}))
        })
        .collect();
    if !patches.is_empty() {
        manifest["graph_overrides"] = json!({"patches": patches});
    }
}

/// Expression context for YAML graph_patches (Wallaby uses per-source sbids).
fn graph_patch_expression_context(manifest: &Value) -> Cow<'_, Value> {
    manifest
        .get("sources")
        .and_then(Value::as_array)
        .and_then(|sources| sources.first())
        .map(Cow::Borrowed)
        .unwrap_or(Cow::Borrowed(manifest))
}

fn render_template_object(template: &Value, record: &Value, context: &Value) -> Value {
    match template {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), render_template_value(v, record, context));
            }
            Value::Object(out)
        }
        other => render_template_value(other, record, context),
    }
}

fn render_manifest_template(template: &ManifestTemplate, record: &Value, context: &Value) -> Value {
    let mut out = Map::new();
    for (key, value) in template.fields() {
        out.insert(key.clone(), render_template_value(value, record, context));
    }
    Value::Object(out)
}

fn render_template_value(template: &Value, record: &Value, context: &Value) -> Value {
    match template {
        Value::String(s) => {
            if s.starts_with('{') && s.ends_with('}') {
                let key = &s[1..s.len() - 1];
                if let Some(flag_key) = key.strip_prefix("flags.") {
                    record
                        .get("discovery_flags")
                        .and_then(|f| f.get(flag_key))
                        .or_else(|| context.get(flag_key))
                        .cloned()
                        .unwrap_or(Value::String(s.clone()))
                } else if let Some(staging_key) = key.strip_prefix("staging.") {
                    value_at_path(context, staging_key)
                        .cloned()
                        .unwrap_or(Value::String(s.clone()))
                } else {
                    record.get(key).cloned().unwrap_or(Value::String(s.clone()))
                }
            } else {
                Value::String(s.clone())
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| render_template_value(v, record, context))
                .collect(),
        ),
        Value::Object(map) => render_template_object(&Value::Object(map.clone()), record, context),
        other => other.clone(),
    }
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn value_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

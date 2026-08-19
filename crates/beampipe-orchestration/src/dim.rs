use serde_json::{json, Value};

pub fn dim_rest_http_base(deploy_host: &str, deploy_port: i32) -> String {
    dim_rest_base(deploy_host, deploy_port, false)
}

pub fn dim_rest_base(deploy_host: &str, deploy_port: i32, use_https: bool) -> String {
    let scheme = if use_https { "https" } else { "http" };
    if deploy_port != 80 {
        format!("{scheme}://{deploy_host}:{deploy_port}")
    } else {
        format!("{scheme}://{deploy_host}")
    }
}

pub fn dim_operator_urls_from_base(dim_base: &str, session_id: &str) -> serde_json::Value {
    let sid = urlencoding_path(session_id);
    let base = dim_base.trim_end_matches('/');
    serde_json::json!({
        "dim_session_status_url": format!("{base}/api/sessions/{sid}/status"),
        "dim_graph_status_url": format!("{base}/api/sessions/{sid}/graph/status"),
    })
}

fn links(links: &Value) -> Vec<String> {
    match links {
        Value::Array(items) => items
            .iter()
            .flat_map(|x| {
                if let Some(obj) = x.as_object() {
                    obj.keys().cloned().collect::<Vec<_>>()
                } else if let Value::Array(inner) = x {
                    inner
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                } else {
                    vec![x.to_string()]
                }
            })
            .collect(),
        Value::Object(obj) => obj.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// DIM's session graph UI keys off `type` (`app`/`plain`/`socket`/`container`)
/// and `humanReadableKey`. Translator `/map` often emits only EAGLE
/// `categoryType` / `dropclass`, which leaves every node as CSS class
/// `undefined` and throws on `humanReadableKey.toString()`.
pub fn prepare_physical_graph(pg_spec: Vec<Value>) -> Vec<Value> {
    let mut drops = Vec::new();
    let mut repro = None;
    for item in pg_spec {
        let Some(obj) = item.as_object() else {
            continue;
        };
        if obj.get("oid").and_then(Value::as_str).is_none() {
            if obj.get("rmode").is_some() {
                repro = Some(item);
            }
            continue;
        }
        drops.push(item);
    }
    for (index, drop) in drops.iter_mut().enumerate() {
        normalize_drop_for_dim(drop, index + 1);
    }
    if let Some(repro) = repro {
        drops.push(repro);
    }
    drops
}

fn normalize_drop_for_dim(drop: &mut Value, human_index: usize) {
    let Some(obj) = drop.as_object_mut() else {
        return;
    };
    let category = obj
        .get("categoryType")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let existing_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
    if existing_type.is_empty() {
        let mapped = match category.as_str() {
            "Application" | "app" => Some("app"),
            "Data" | "data" | "plain" => Some("plain"),
            "Socket" | "socket" => Some("socket"),
            "Container" | "container" => Some("container"),
            _ => None,
        };
        if let Some(mapped) = mapped {
            obj.insert("type".into(), json!(mapped));
        }
    }
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if ty == "app" {
        let has_app = obj
            .get("app")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
        if !has_app {
            if let Some(dropclass) = obj
                .get("dropclass")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
            {
                obj.insert("app".into(), json!(dropclass));
            }
        }
    }
    if obj.get("humanReadableKey").is_none_or(Value::is_null) {
        let iid = obj.get("iid").map_or_else(
            || "0".to_string(),
            |value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())
            },
        );
        obj.insert(
            "humanReadableKey".into(),
            json!(format!("{human_index}_{iid}")),
        );
    }
}

/// OIDs DIM should trigger at deploy time (`completed=`).
///
/// Input-less applications are roots: DIM calls `async_execute()` on them.
/// Producer-less data drops are also roots: DIM marks them COMPLETED so
/// downstream apps fire. Passing only data roots would leave start apps idle.
pub fn get_roots(pg_spec: &[Value]) -> Vec<String> {
    let mut all_oids = std::collections::HashSet::new();
    let mut nonroots = std::collections::HashSet::new();
    for d in pg_spec {
        let Some(obj) = d.as_object() else {
            continue;
        };
        let Some(oid) = obj.get("oid").and_then(Value::as_str) else {
            continue;
        };
        all_oids.insert(oid.to_string());
        let ct = obj
            .get("categoryType")
            .or_else(|| obj.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        match ct {
            "Application" | "app" | "Socket" | "socket" => {
                if obj.get("inputs").is_some() || obj.get("streamingInputs").is_some() {
                    nonroots.insert(oid.to_string());
                }
                if let Some(outputs) = obj.get("outputs") {
                    for link in links(outputs) {
                        nonroots.insert(link);
                    }
                }
            }
            "Data" | "data" | "plain" => {
                if obj.get("producers").is_some() {
                    nonroots.insert(oid.to_string());
                }
                for key in ["consumers", "streamingConsumers"] {
                    if let Some(consumers) = obj.get(key) {
                        for link in links(consumers) {
                            nonroots.insert(link);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut roots = all_oids.difference(&nonroots).cloned().collect::<Vec<_>>();
    roots.sort();
    roots
}

fn urlencoding_path(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn get_roots_includes_inputless_applications_and_producerless_data() {
        let spec = vec![
            json!({
                "oid": "ingest",
                "categoryType": "Application",
                "outputs": [{"child": {}}]
            }),
            json!({
                "oid": "child",
                "categoryType": "Data",
                "producers": ["ingest"],
                "consumers": ["downstream"]
            }),
            json!({
                "oid": "parset",
                "categoryType": "Data",
                "consumers": ["downstream"]
            }),
        ];
        let roots = get_roots(&spec);
        assert_eq!(roots, vec!["ingest".to_string(), "parset".to_string()]);
    }

    #[test]
    fn prepare_physical_graph_fills_dim_ui_fields_and_drops_empty_objects() {
        let prepared = prepare_physical_graph(vec![
            json!({}),
            json!({
                "oid": "ingest",
                "iid": "0",
                "categoryType": "Application",
                "dropclass": "dlg.apps.pyfunc.PyFuncApp",
            }),
            json!({
                "oid": "file",
                "iid": 1,
                "categoryType": "Data",
                "dropclass": "dlg.data.drops.file.FileDROP",
            }),
            json!({"rmode": "1"}),
        ]);
        assert_eq!(prepared.len(), 3);
        assert_eq!(prepared[0]["type"], "app");
        assert_eq!(prepared[0]["app"], "dlg.apps.pyfunc.PyFuncApp");
        assert_eq!(prepared[0]["humanReadableKey"], "1_0");
        assert_eq!(prepared[1]["type"], "plain");
        assert_eq!(prepared[1]["humanReadableKey"], "2_1");
        assert_eq!(prepared[2]["rmode"], "1");
        assert!(prepared[1].get("app").is_none());
    }
}

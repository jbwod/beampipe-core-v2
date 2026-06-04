use serde_json::Value;

pub fn dim_rest_http_base(deploy_host: &str, deploy_port: i32) -> String {
    if deploy_port != 80 {
        format!("http://{deploy_host}:{deploy_port}")
    } else {
        format!("http://{deploy_host}")
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
            "Data" | "data" => {
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
    all_oids.difference(&nonroots).cloned().collect::<Vec<_>>()
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
    fn get_roots_finds_leaf_nodes() {
        let spec = vec![
            json!({
                "oid": "root",
                "categoryType": "Application",
                "outputs": [{"child": {}}]
            }),
            json!({
                "oid": "child",
                "categoryType": "Data",
                "producers": ["root"]
            }),
        ];
        let roots = get_roots(&spec);
        assert!(roots.contains(&"root".to_string()));
    }
}

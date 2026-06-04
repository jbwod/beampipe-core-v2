use serde_json::Value;

/// Evaluate simple config expressions like `$count(sbids[].datasets[])` or `$sum(sbids[].beam_count)`.
pub fn evaluate_expression(expr: &str, manifest: &Value) -> Option<Value> {
    let trimmed = expr.trim();
    if let Some(inner) = trimmed
        .strip_prefix("$count(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Some(Value::Number(count_path(manifest, inner.trim()).into()));
    }
    if let Some(inner) = trimmed
        .strip_prefix("$sum(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Some(Value::Number(sum_path(manifest, inner.trim()).into()));
    }
    None
}

fn count_path(manifest: &Value, path: &str) -> u64 {
    let parts: Vec<&str> = path.split('.').collect();
    count_at(manifest, &parts, 0)
}

fn sum_path(manifest: &Value, path: &str) -> u64 {
    let parts: Vec<&str> = path.split('.').collect();
    sum_at(manifest, &parts, 0)
}

fn count_at(value: &Value, parts: &[&str], idx: usize) -> u64 {
    if idx >= parts.len() {
        return 1;
    }
    let part = parts[idx];
    if let Some(key) = part.strip_suffix("[]") {
        let arr = if key.is_empty() {
            value.as_array()
        } else {
            value.get(key).and_then(Value::as_array)
        };
        return arr
            .map(|items| {
                items
                    .iter()
                    .map(|item| count_at(item, parts, idx + 1))
                    .sum()
            })
            .unwrap_or(0);
    }
    value
        .get(part)
        .map(|next| count_at(next, parts, idx + 1))
        .unwrap_or(0)
}

fn sum_at(value: &Value, parts: &[&str], idx: usize) -> u64 {
    if idx >= parts.len() {
        return numeric_value(value);
    }
    let part = parts[idx];
    if let Some(key) = part.strip_suffix("[]") {
        let arr = if key.is_empty() {
            value.as_array()
        } else {
            value.get(key).and_then(Value::as_array)
        };
        return arr
            .map(|items| items.iter().map(|item| sum_at(item, parts, idx + 1)).sum())
            .unwrap_or(0);
    }
    value
        .get(part)
        .map(|next| sum_at(next, parts, idx + 1))
        .unwrap_or(0)
}

fn numeric_value(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .or_else(|| value.as_f64().map(|v| v as u64))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn count_sbids_datasets() {
        let manifest = json!({
            "sources": [{
                "sbids": [
                    {"datasets": [{"id": 1}, {"id": 2}]},
                    {"datasets": [{"id": 3}]}
                ]
            }]
        });
        let out = evaluate_expression("$count(sbids[].datasets[])", &manifest["sources"][0]);
        assert_eq!(out, Some(json!(3)));
    }

    #[test]
    fn sum_beam_counts() {
        let manifest = json!({
            "sbids": [
                {"beam_count": 2},
                {"beam_count": 3}
            ]
        });
        let out = evaluate_expression("$sum(sbids[].beam_count)", &manifest);
        assert_eq!(out, Some(json!(5)));
    }
}

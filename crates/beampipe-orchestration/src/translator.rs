//! DALiuGE Translator Manager helpers (parity with Python `translator_client.py`).

use serde_json::Value;

const DEFAULT_LG_NAME: &str = "beampipe.graph";

/// Wrap TM `/unroll_and_partition` JSON as `[pgt_filename, drop_list]` for `create_dlg_job -P`.
pub fn partitioned_pgt_for_dlg_deploy(pgt_json: Value, lg_name: &str) -> Value {
    if let Value::Array(arr) = &pgt_json {
        if arr.len() == 2 && arr[0].is_string() && arr[1].is_array() {
            return pgt_json;
        }
    }
    let filename = pgt_filename_from_lg_name(lg_name);
    Value::Array(vec![Value::String(filename), pgt_json])
}

pub fn pgt_filename_from_lg_name(lg_name: &str) -> String {
    let base = lg_name.rsplit('/').next().unwrap_or(lg_name);
    if let Some(stem) = base.strip_suffix(".graph") {
        format!("{stem}_pgt.graph")
    } else {
        format!("{base}.pgt.graph")
    }
}

pub fn pgt_handle_from_partitioned_payload(pgt_json: &Value, fallback_lg_name: &str) -> String {
    if let Value::Array(arr) = pgt_json {
        if let Some(Value::String(name)) = arr.first() {
            return name.clone();
        }
    }
    fallback_lg_name
        .rsplit('/')
        .next()
        .unwrap_or(fallback_lg_name)
        .to_string()
}

pub fn default_lg_name() -> &'static str {
    DEFAULT_LG_NAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partitioned_pgt_wraps_bare_array() {
        let raw = json!([{"oid": "a"}]);
        let wrapped = partitioned_pgt_for_dlg_deploy(raw.clone(), "beampipe.graph");
        assert_eq!(wrapped, json!(["beampipe_pgt.graph", [{"oid": "a"}]]));
    }

    #[test]
    fn partitioned_pgt_passes_through_two_element_list() {
        let raw = json!(["existing.pgt.graph", [{"oid": "a"}]]);
        let out = partitioned_pgt_for_dlg_deploy(raw.clone(), "beampipe.graph");
        assert_eq!(out, raw);
    }

    #[test]
    fn pgt_filename_strips_graph_suffix() {
        assert_eq!(
            pgt_filename_from_lg_name("logical_graphs/chiles_simple.graph"),
            "chiles_simple_pgt.graph"
        );
    }
}

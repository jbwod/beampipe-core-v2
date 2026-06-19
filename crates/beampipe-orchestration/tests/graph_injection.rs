use beampipe_orchestration::{
    apply_manifest_graph_overrides, apply_project_graph_patches, inject_manifest_config_into_graph,
    prepare_graph_for_manifest,
};
use beampipe_project::ProjectConfig;
use serde_json::{json, Value};

fn minimal_lg_with_scatter_and_ingest(scatter_name: &str) -> Value {
    json!({
        "nodeDataArray": [
            {
                "id": "n_ingest",
                "name": "beampipe-ingest",
                "fields": [
                    {"id": "ingf1", "name": "manifest_path", "type": "String", "value": "{}"},
                ],
            },
            {
                "id": "n_scatter",
                "name": scatter_name,
                "fields": [
                    {"id": "sf1", "name": "num_of_copies", "type": "Integer", "value": 1, "defaultValue": "1"},
                ],
            },
        ],
        "linkDataArray": [],
    })
}

#[test]
fn scatter_override_sets_integer_field_for_all_matching_nodes() {
    let mut graph = minimal_lg_with_scatter_and_ingest("Scatter/Test");
    graph["nodeDataArray"].as_array_mut().unwrap().push(json!({
        "id": "n_scatter2",
        "name": "Scatter/Test",
        "fields": [{"id": "sf2", "name": "num_of_copies", "type": "Integer", "value": 1}],
    }));
    let manifest = json!({
        "sources": [],
        "graph_overrides": {
            "version": 1,
            "patches": [{
                "match": {"kind": "node_name", "equals": "Scatter/Test"},
                "fields": [{"name": "num_of_copies", "value": 6}],
            }],
        },
    });
    apply_manifest_graph_overrides(&mut graph, &manifest).unwrap();
    let nodes = graph["nodeDataArray"].as_array().unwrap();
    assert_eq!(nodes[1]["fields"][0]["value"], 6);
    assert_eq!(nodes[1]["fields"][0]["defaultValue"], "6");
    assert_eq!(nodes[2]["fields"][0]["value"], 6);
}

#[test]
fn inject_manifest_embed_excludes_graph_overrides_key() {
    let mut graph = minimal_lg_with_scatter_and_ingest("Scatter/Test");
    let manifest = json!({
        "sources": [{"source_identifier": "x"}],
        "graph_overrides": {"version": 1, "patches": []},
        "secret_marker": "should_be_embedded",
    });
    inject_manifest_config_into_graph(&mut graph, &manifest, Some("cfg-golden")).unwrap();
    let cid = graph["activeGraphConfigId"].as_str().unwrap();
    assert_eq!(cid, "cfg-golden");
    let embedded = graph["graphConfigurations"][cid]["nodes"]["n_ingest"]["fields"]["ingf1"]
        ["value"]
        .as_str()
        .unwrap();
    let parsed: Value = serde_json::from_str(embedded).unwrap();
    assert!(parsed.get("graph_overrides").is_none());
    assert_eq!(parsed["secret_marker"], "should_be_embedded");
}

#[test]
fn wallaby_manifest_and_prepare_graph_applies_scatter_patch() {
    let config =
        ProjectConfig::from_slice(include_bytes!("../../../config/wallaby_hires.v2.yaml")).unwrap();
    let mut manifest = json!({
        "sources": [{
            "source_identifier": "HIPASSJ0000-00",
            "sbids": [
                {"sbid": "1", "datasets": [{"id": "a"}, {"id": "b"}]},
                {"sbid": "2", "datasets": [{"id": "c"}]},
            ],
        }],
    });
    apply_project_graph_patches(&mut manifest, &config);

    let patch = &manifest["graph_overrides"]["patches"][0];
    assert_eq!(patch["match"]["equals"], "Scatter/GenericScatterApp/Beam");
    assert_eq!(patch["fields"][0]["name"], "num_of_copies");
    assert_eq!(patch["fields"][0]["value"], 3);

    let graph = minimal_lg_with_scatter_and_ingest("Scatter/GenericScatterApp/Beam");
    let prepared = prepare_graph_for_manifest(graph, &manifest, "manifest.json").unwrap();

    assert!(prepared.get("graphConfigurations").is_some());
    assert!(prepared.get("activeGraphConfigId").is_some());
    assert_eq!(prepared["nodeDataArray"][1]["fields"][0]["value"], 3);
}

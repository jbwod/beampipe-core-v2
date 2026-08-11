use beampipe_orchestration::build_manifest_from_config;
use beampipe_project::ProjectConfig;
use serde_json::json;

#[test]
fn manifest_excludes_failed_sbids() {
    let config =
        ProjectConfig::from_slice(include_bytes!("../../../config/wallaby_hires.v2.yaml")).unwrap();
    let metadata = vec![
        json!({"source_identifier": "s1", "sbid": "1", "dataset_id": "d1", "discovery_flags": {"ra_string": "1:2:3"}}),
        json!({"source_identifier": "s1", "sbid": "2", "dataset_id": "d2"}),
    ];
    let manifest = build_manifest_from_config(&config, &metadata, &["2".into()]).unwrap();
    let sbids: Vec<_> = manifest["sources"][0]["sbids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["sbid"].as_str().unwrap())
        .collect();
    assert_eq!(sbids, vec!["1"]);
}

#[test]
fn manifest_resolves_flags_from_flat_persisted_dataset_fields() {
    let config =
        ProjectConfig::from_slice(include_bytes!("../../../config/wallaby_hires.v2.yaml")).unwrap();
    let metadata = vec![json!({
        "source_identifier": "HIPASSJ1313-15",
        "sbid": "72962",
        "dataset_id": "dataset-1",
        "ra_string": "13h13m34.1s",
        "dec_string": "-15.27.32",
        "vsys": "2505.3"
    })];

    let manifest = build_manifest_from_config(&config, &metadata, &[]).unwrap();
    let source = &manifest["sources"][0];

    assert_eq!(source["ra_string"], "13h13m34.1s");
    assert_eq!(source["dec_string"], "-15.27.32");
    assert_eq!(source["vsys"], "2505.3");
}

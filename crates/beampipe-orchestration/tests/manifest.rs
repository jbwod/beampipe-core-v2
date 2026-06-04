use beampipe_orchestration::build_manifest_from_config;
use beampipe_project::ProjectConfig;
use serde_json::json;

#[test]
fn manifest_excludes_failed_sbids() {
    let config =
        ProjectConfig::from_slice(include_bytes!("../../../config/wallaby_hires.v1.yaml")).unwrap();
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

use std::process::Command;

#[test]
fn strict_project_validation_prints_structured_diagnostics_and_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.yaml");
    std::fs::write(
        &path,
        r#"
apiVersion: beampipe.dev/v2
kind: ProjectConfig
metadata:
  id: strict-cli-test
  unknown_field: rejected
adapters:
  required: [casda]
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_beampipe"))
        .args(["project", "validate", "--file"])
        .arg(&path)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["valid"], false);
    assert_eq!(report["project_id"], "strict-cli-test");
    assert_eq!(report["errors"][0]["path"], "metadata.unknown_field");
    assert_eq!(report["errors"][0]["severity"], "error");
    assert_eq!(report["errors"][0]["code"], "invalid_config_structure");
    assert!(report["errors"][0]["hint"].is_string());
}

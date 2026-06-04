use beampipe_domain::run_record::{merge_dim_deploy_into_manifest, merge_slurm_poll_into_manifest};

#[test]
fn dim_deploy_records_operator_urls() {
    let mut urls = std::collections::HashMap::new();
    urls.insert("dim_session_status_url".into(), "http://dim/status".into());
    let out = merge_dim_deploy_into_manifest(None, "sid", "http://dim", false, Some(urls));
    assert_eq!(
        out["beampipe_run_record"]["dim"]["deploy"]["operator_urls"]["dim_session_status_url"],
        "http://dim/status"
    );
}

#[test]
fn slurm_poll_records_terminal() {
    let out = merge_slurm_poll_into_manifest(
        None,
        "sid:123|/tmp",
        "123",
        "COMPLETED",
        "sacct",
        Some("COMPLETED"),
        true,
        Some("completed"),
        beampipe_domain::run_record::SlurmPollManifestOpts {
            exit_code: Some(0),
            remote_session_dir: Some("/tmp"),
            reason: None,
            diagnostics: None,
        },
    );
    assert_eq!(
        out["beampipe_run_record"]["slurm"]["terminal"]["ledger_status"],
        "completed"
    );
}

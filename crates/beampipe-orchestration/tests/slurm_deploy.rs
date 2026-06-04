use beampipe_orchestration::slurm_deploy::{parse_jobsub_path, render_generated_ini};
use beampipe_profiles::SlurmRemoteDeploymentConfig;

fn sample_slurm_config() -> SlurmRemoteDeploymentConfig {
    SlurmRemoteDeploymentConfig {
        login_node: "login".into(),
        ssh_port: 22,
        remote_user: None,
        account: "acct".into(),
        home_dir: "/home".into(),
        log_dir: "/log".into(),
        exec_prefix: "srun -l".into(),
        dlg_root: "/dlg".into(),
        venv: None,
        modules: None,
        facility: "setonix".into(),
        job_duration_minutes: 30,
        num_nodes: 1,
        num_islands: 1,
        verbose_level: 1,
        max_threads: 0,
        all_nics: false,
        zerorun: false,
        sleepncopy: false,
        check_with_session: false,
        verify_ssl: None,
        slurm_template: None,
    }
}

#[test]
fn ini_contains_dlg_root_and_account() {
    let ini = render_generated_ini(&sample_slurm_config(), "user", "/path.pgt", "/dlg");
    assert!(ini.contains("DLG_ROOT = /dlg"));
    assert!(ini.contains("ACCOUNT = acct"));
}

#[test]
fn jobsub_path_parsed_from_stdout() {
    let stdout = "Created job submission script /home/user/sessions/x/jobsub.sh\n";
    assert!(parse_jobsub_path(stdout).unwrap().ends_with("jobsub.sh"));
}

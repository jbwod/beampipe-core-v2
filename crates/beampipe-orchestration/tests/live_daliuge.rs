//! Opt-in integration test for a real DALiuGE Translator Manager and DIM.
//!
//! Example graph:
//! https://github.com/ICRAR/EAGLE-graph-repo/blob/master/examples/HelloWorld-Universe.graph
//!
//! ```text
//! BEAMPIPE_TEST_DALIUGE_GRAPH=/tmp/HelloWorld-Universe.graph \
//! BEAMPIPE_TEST_TM_URL=http://dlg-tm.desk \
//! BEAMPIPE_TEST_DIM_URL=http://dlg-dim.desk \
//! BEAMPIPE_TEST_DIM_HOST_FOR_TM=dlg-dim.desk \
//! BEAMPIPE_TEST_DIM_PORT_FOR_TM=80 \
//! cargo test -p beampipe-orchestration --test live_daliuge -- --ignored --nocapture
//! ```

use beampipe_domain::ExecutionStatus;
use beampipe_orchestration::{
    clients::TranslateConfig, DaliugeManager, DaliugeTranslator, DimClient, ExecutionBackend,
    HttpDimClient, HttpTranslatorClient, RestExecutionBackend,
};
use chrono::Utc;
use serde_json::{json, Value};
use std::{env, path::PathBuf, time::Duration};
use uuid::Uuid;

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the live DALiuGE test"))
}

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
}

#[tokio::test]
#[ignore = "requires explicitly configured live DALiuGE TM and DIM services"]
async fn hello_universe_translates_deploys_and_finishes() {
    let graph_path = PathBuf::from(required_env("BEAMPIPE_TEST_DALIUGE_GRAPH"));
    let tm_url = env_or("BEAMPIPE_TEST_TM_URL", "http://dlg-tm.desk");
    let dim_url = env_or("BEAMPIPE_TEST_DIM_URL", "http://dlg-dim.desk");
    let dim_host = env_or("BEAMPIPE_TEST_DIM_HOST_FOR_TM", "dlg-dim.desk");
    let dim_port = env_or("BEAMPIPE_TEST_DIM_PORT_FOR_TM", "80")
        .parse::<i32>()
        .expect("BEAMPIPE_TEST_DIM_PORT_FOR_TM must be an integer");
    let keep_session = env_flag("BEAMPIPE_TEST_KEEP_DALIUGE_SESSION");

    let graph_bytes = tokio::fs::read(&graph_path)
        .await
        .unwrap_or_else(|error| panic!("read {}: {error}", graph_path.display()));
    let graph: Value = serde_json::from_slice(&graph_bytes)
        .unwrap_or_else(|error| panic!("parse {}: {error}", graph_path.display()));
    assert!(
        graph
            .get("nodeDataArray")
            .and_then(Value::as_array)
            .is_some_and(|nodes| !nodes.is_empty()),
        "live test graph must contain nodeDataArray"
    );

    let translator = HttpTranslatorClient::new(&tm_url);
    let dim = HttpDimClient::new(&dim_url);
    let translator_info = translator
        .inspect(Some(&dim_host), Some(dim_port))
        .await
        .expect("inspect Translator Manager");
    assert!(translator_info.capabilities.updated_translation_api);
    let manager_info = dim.inspect().await.expect("inspect Data Island Manager");
    assert!(
        !manager_info.nodes.is_empty(),
        "Data Island Manager must report at least one node"
    );

    let backend = RestExecutionBackend {
        translator,
        dim: dim.clone(),
        profile_name: Some("live-hello-universe".into()),
        tm_url: Some(tm_url.clone()),
        dim_endpoint: Some(dim_url.clone()),
        translate_config: TranslateConfig {
            algo: "metis".into(),
            num_par: 1,
            num_islands: 1,
            dim_host,
            dim_port,
            slurm_path: false,
        },
        session_created_at: Utc::now(),
    };
    let execution_id = Uuid::now_v7().to_string();
    let submitted = backend
        .submit(
            &execution_id,
            json!({
                "smoke_test": "hello_universe",
                "graph_path": graph_path.file_name().and_then(|name| name.to_str()),
            }),
            graph,
        )
        .await
        .expect("translate and deploy Hello Universe graph");
    let session_id = submitted
        .session_id
        .expect("REST deployment must return a DALiuGE session ID");
    assert!(
        submitted
            .physical_graph
            .as_ref()
            .and_then(Value::as_array)
            .is_some_and(|drops| !drops.is_empty()),
        "Translator Manager must return a non-empty physical graph"
    );
    println!("deployed DALiuGE session {session_id}");

    let mut terminal = None;
    for round in 1..=120 {
        let observation = dim
            .poll(&session_id)
            .await
            .unwrap_or_else(|error| panic!("poll DALiuGE session {session_id}: {error}"));
        let normalized = observation
            .poll_summary
            .get("normalized_session_state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let per_node = observation
            .poll_summary
            .get("per_node")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let error_count = observation
            .poll_summary
            .get("error_drop_uids")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        println!(
            "poll round={round} status={} normalized={normalized} errors={error_count} nodes={per_node}",
            observation.status.as_str()
        );
        if matches!(
            observation.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            terminal = Some(observation);
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let terminal = terminal.expect("DALiuGE session did not finish within 120 seconds");
    let error_drop_uids = terminal
        .poll_summary
        .get("error_drop_uids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if !keep_session {
        dim.destroy_session(&session_id)
            .await
            .unwrap_or_else(|error| panic!("destroy DALiuGE session {session_id}: {error}"));
    } else {
        println!("retaining DALiuGE session {session_id} for operator inspection");
    }

    assert_eq!(
        terminal.status,
        ExecutionStatus::Completed,
        "terminal DALiuGE observation: {}",
        terminal.poll_summary
    );
    assert!(
        error_drop_uids.is_empty(),
        "DALiuGE graph status contains error drops: {error_drop_uids:?}"
    );
}

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
    clients::TranslateConfig,
    dim::prepare_physical_graph,
    DaliugeManager, DaliugeTranslator, DimClient, ExecutionBackend, HttpDimClient,
    HttpTranslatorClient, RestExecutionBackend,
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
    let mapped = submitted
        .physical_graph
        .as_ref()
        .and_then(Value::as_array)
        .cloned()
        .unwrap();
    assert_dim_ui_fields(&mapped, "translator physical graph");
    println!("deployed DALiuGE session {session_id}");

    let stored: Value = reqwest::Client::new()
        .get(format!(
            "{}/api/sessions/{}/graph",
            dim_url.trim_end_matches('/'),
            session_id
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET DIM graph for {session_id}: {error}"))
        .json()
        .await
        .expect("parse DIM graph");
    assert_dim_ui_fields(
        &drop_list_from_dim_graph(stored),
        &format!("DIM stored {session_id}"),
    );

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

fn drop_list_from_dim_graph(graph: Value) -> Vec<Value> {
    match graph {
        Value::Array(items) => items,
        Value::Object(map) => map.into_values().collect(),
        other => panic!("DIM graph must be an array or object, got {other}"),
    }
}

fn assert_dim_ui_fields(drops: &[Value], context: &str) {
    assert!(!drops.is_empty(), "{context}: expected at least one drop");
    let mut missing_type = Vec::new();
    let mut missing_key = Vec::new();
    let mut missing_app = Vec::new();
    for drop in drops {
        let oid = drop
            .get("oid")
            .and_then(Value::as_str)
            .unwrap_or("<no oid>");
        let ty = drop.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(ty, "app" | "plain" | "socket" | "container") {
            missing_type.push(format!("{oid} type={ty:?}"));
        }
        if drop.get("humanReadableKey").is_none_or(Value::is_null) {
            missing_key.push(oid.to_string());
        }
        if ty == "app"
            && drop
                .get("app")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            missing_app.push(oid.to_string());
        }
    }
    assert!(
        missing_type.is_empty(),
        "{context}: drops missing DIM type: {missing_type:?}"
    );
    assert!(
        missing_key.is_empty(),
        "{context}: drops missing humanReadableKey: {missing_key:?}"
    );
    assert!(
        missing_app.is_empty(),
        "{context}: app drops missing app/dropclass: {missing_app:?}"
    );
}

#[tokio::test]
#[ignore = "requires a live DIM with a previously mapped Beampipe session"]
async fn prepare_physical_graph_fills_fields_on_live_mapped_session() {
    let dim_url = env_or("BEAMPIPE_TEST_DIM_URL", "http://dlg-dim.desk");
    let client = reqwest::Client::new();
    let sessions: Value = client
        .get(format!("{}/api", dim_url.trim_end_matches('/')))
        .send()
        .await
        .expect("GET DIM /api")
        .json()
        .await
        .expect("parse DIM /api");
    let session_id = env::var("BEAMPIPE_TEST_SOURCE_SESSION").ok().or_else(|| {
        sessions
            .get("sessionIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .rev()
            .filter_map(Value::as_str)
            .find(|id| id.starts_with("BeampipeExecution-"))
            .map(str::to_string)
    });
    let session_id = session_id.expect("no BeampipeExecution session on DIM");
    let graph: Value = client
        .get(format!(
            "{}/api/sessions/{}/graph",
            dim_url.trim_end_matches('/'),
            session_id
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET DIM graph for {session_id}: {error}"))
        .json()
        .await
        .expect("parse DIM graph");
    let raw = drop_list_from_dim_graph(graph);
    let untyped = raw
        .iter()
        .filter(|drop| drop.get("type").and_then(Value::as_str).is_none())
        .count();
    assert!(
        untyped > 0,
        "session {session_id} already has type on every drop; pick an unmapped session"
    );
    let prepared = prepare_physical_graph(raw);
    assert_dim_ui_fields(&prepared, &format!("prepared {session_id}"));
}

#[tokio::test]
#[ignore = "requires a live Data Island Manager"]
async fn prepared_graph_roundtrips_dim_ui_fields() {
    let dim_url = env_or("BEAMPIPE_TEST_DIM_URL", "http://dlg-dim.desk");
    let session_id = format!("BeampipeDimUiFix-{}", Uuid::now_v7());
    let raw = vec![
        json!({
            "oid": "ui-fix-app",
            "iid": "0",
            "name": "sleep",
            "categoryType": "Application",
            "dropclass": "dlg.apps.simple.SleepApp",
            "sleepTime": 0,
            "outputs": [{"ui-fix-data": "out"}]
        }),
        json!({
            "oid": "ui-fix-data",
            "iid": "0",
            "name": "out",
            "categoryType": "Data",
            "dropclass": "dlg.data.drops.memory.InMemoryDROP",
            "producers": ["ui-fix-app"]
        }),
    ];
    let dim = HttpDimClient::new(&dim_url);
    let manager = dim.inspect().await.expect("inspect DIM");
    let node = manager
        .nodes
        .first()
        .cloned()
        .expect("DIM must report at least one node");
    let mut prepared = prepare_physical_graph(raw);
    for drop in &mut prepared {
        if let Some(obj) = drop.as_object_mut() {
            obj.entry("node".to_string())
                .or_insert_with(|| json!(node));
            obj.entry("island".to_string())
                .or_insert_with(|| json!(node));
        }
    }
    assert_dim_ui_fields(&prepared, "local prepare");
    let deploy = dim
        .deploy(&session_id, &prepared, &["ui-fix-app".to_string()])
        .await;
    let client = reqwest::Client::new();
    let fetched = client
        .get(format!(
            "{}/api/sessions/{}/graph",
            dim_url.trim_end_matches('/'),
            session_id
        ))
        .send()
        .await;
    let _ = dim.destroy_session(&session_id).await;
    deploy.unwrap_or_else(|error| panic!("deploy {session_id}: {error:?}"));
    let graph: Value = fetched
        .unwrap_or_else(|error| panic!("GET DIM graph for {session_id}: {error}"))
        .json()
        .await
        .expect("parse DIM graph");
    assert_dim_ui_fields(
        &drop_list_from_dim_graph(graph),
        &format!("DIM stored {session_id}"),
    );
}

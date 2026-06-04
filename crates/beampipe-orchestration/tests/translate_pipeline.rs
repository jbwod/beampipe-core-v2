use async_trait::async_trait;
use beampipe_orchestration::{
    clients::{TranslateConfig, TranslatedGraph},
    prepare_graph_for_manifest, ExecutionBackend, MockSlurmClient, OrchestrationError,
    SlurmExecutionBackend, TranslatorClient,
};
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct CapturingTranslator {
    last_graph: Arc<Mutex<Option<Value>>>,
}

impl CapturingTranslator {
    fn new() -> Self {
        Self {
            last_graph: Arc::new(Mutex::new(None)),
        }
    }

    fn take_graph(&self) -> Option<Value> {
        self.last_graph.lock().unwrap().take()
    }
}

#[async_trait]
impl TranslatorClient for CapturingTranslator {
    async fn translate(
        &self,
        graph: Value,
        _config: &TranslateConfig,
    ) -> Result<TranslatedGraph, OrchestrationError> {
        *self.last_graph.lock().unwrap() = Some(graph);
        Ok(TranslatedGraph {
            pg_spec: vec![],
            roots: vec![],
            pgt_json: Some(json!({"mock_pgt": true})),
        })
    }
}

fn minimal_wallaby_lg() -> Value {
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
                "name": "Scatter/GenericScatterApp/Beam",
                "fields": [
                    {"id": "sf1", "name": "num_of_copies", "type": "Integer", "value": 1},
                ],
            },
        ],
        "linkDataArray": [],
    })
}

#[tokio::test]
async fn slurm_submit_passes_prepared_graph_to_translator() {
    let manifest = json!({
        "sources": [{"source_identifier": "HIPASSJ1313-15", "sbids": [{"sbid": "1", "datasets": [{"id": "d1"}]}]}],
        "graph_overrides": {
            "patches": [{
                "match": {"equals": "Scatter/GenericScatterApp/Beam"},
                "fields": [{"name": "num_of_copies", "value": 1}],
            }],
        },
    });
    let prepared =
        prepare_graph_for_manifest(minimal_wallaby_lg(), &manifest, "manifest.json").unwrap();

    let translator = CapturingTranslator::new();
    let backend = SlurmExecutionBackend {
        translator: translator.clone(),
        slurm: MockSlurmClient,
        profile_name: Some("slurm-remote".into()),
        session_dir: "/tmp/beampipe".into(),
        login_node: Some("login".into()),
        remote_user: Some("user".into()),
        account: Some("acct".into()),
        translate_config: TranslateConfig {
            slurm_path: true,
            ..Default::default()
        },
        session_created_at: Utc::now(),
    };

    backend
        .submit("019e0000-0000-7000-8000-000000000001", manifest, prepared)
        .await
        .unwrap();

    let captured = translator.take_graph().expect("translate was called");
    assert!(captured.get("graphConfigurations").is_some());
    assert!(captured.get("activeGraphConfigId").is_some());
    let cid = captured["activeGraphConfigId"].as_str().unwrap();
    let embedded = captured["graphConfigurations"][cid]["nodes"]["n_ingest"]["fields"]["ingf1"]
        ["value"]
        .as_str()
        .unwrap();
    let parsed: Value = serde_json::from_str(embedded).unwrap();
    assert!(parsed.get("graph_overrides").is_none());
    assert_eq!(parsed["sources"][0]["source_identifier"], "HIPASSJ1313-15");
    assert_eq!(captured["nodeDataArray"][1]["fields"][0]["value"], 1);
}

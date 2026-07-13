//! HTTP fixture test: updated TM translate/map and DIM deploy/poll.

use beampipe_orchestration::{
    clients::{HttpDimClient, HttpTranslatorClient, TranslateConfig},
    dim::get_roots,
    DaliugeErrorKind, DaliugeManager, DaliugeTranslator, DimClient, TranslatorClient,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn rest_translate_and_deploy_against_mock_tm_dim() {
    let tm = MockServer::start().await;
    let dim = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/unroll_and_partition"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "oid": "partitioned_app",
            "categoryType": "Application",
            "outputs": [],
        }])))
        .mount(&tm)
        .await;

    Mock::given(method("POST"))
        .and(path("/map"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "oid": "root_app",
            "categoryType": "Application",
            "outputs": [],
        }])))
        .mount(&tm)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/sessions"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&dim)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/sessions/session-1/graph/append"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&dim)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/sessions/session-1/deploy"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&dim)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions/session-1/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(4)))
        .mount(&dim)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/sessions/session-1/graph/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&dim)
        .await;

    let translator = HttpTranslatorClient::new(tm.uri());
    let config = TranslateConfig {
        algo: "metis".into(),
        num_par: 1,
        num_islands: 1,
        dim_host: "localhost".into(),
        dim_port: 8001,
        slurm_path: false,
    };
    let graph = json!({"nodeDataArray": [], "linkDataArray": []});
    let translated = translator.translate(graph, &config).await.unwrap();
    assert!(!translated.pg_spec.is_empty());
    assert_eq!(get_roots(&translated.pg_spec), vec!["root_app".to_string()]);

    let dim_client = HttpDimClient::new(dim.uri());
    dim_client
        .deploy("session-1", &translated.pg_spec, &translated.roots)
        .await
        .unwrap();
    let poll = dim_client.poll("session-1").await.unwrap();
    assert!(poll.status.is_terminal());
}

#[tokio::test]
async fn capability_inspection_uses_verified_manager_routes() {
    let tm = MockServer::start().await;
    let dim = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/submission_method"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "methods": ["dim", "slurm"]
        })))
        .mount(&tm)
        .await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "hosts": ["dim.example:8000"],
            "sessionIds": ["session-1"]
        })))
        .mount(&dim)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/nodes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!(["node-a:8000", "node-b:8000"])),
        )
        .mount(&dim)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/sessions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "sessionId": "session-1",
            "status": 3,
            "size": 2
        }])))
        .mount(&dim)
        .await;

    let translator = HttpTranslatorClient::new(tm.uri());
    let translator_info = translator
        .inspect(Some("dim.example"), Some(8000))
        .await
        .unwrap();
    assert!(translator_info.capabilities.updated_translation_api);
    assert_eq!(
        translator_info.capabilities.submission_methods,
        ["dim", "slurm"]
    );

    let manager = HttpDimClient::new(dim.uri());
    let manager_info = manager.inspect().await.unwrap();
    assert_eq!(manager_info.hosts, ["dim.example:8000"]);
    assert_eq!(manager_info.nodes.len(), 2);
    assert_eq!(manager_info.sessions[0].session_id, "session-1");
    assert!(manager_info.capabilities.session_api);
}

#[tokio::test]
async fn manager_errors_preserve_component_status_and_bounded_body() {
    let dim = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/sessions/session-1/status"))
        .respond_with(ResponseTemplate::new(503).set_body_string("manager warming up"))
        .mount(&dim)
        .await;

    let error = HttpDimClient::new(dim.uri())
        .session_observation("session-1")
        .await
        .unwrap_err();
    assert_eq!(error.kind, DaliugeErrorKind::HttpStatus);
    assert_eq!(error.http_status, Some(503));
    assert!(error.retryable);
    assert_eq!(
        error.response_excerpt.as_deref(),
        Some("manager warming up")
    );
}

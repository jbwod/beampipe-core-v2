//! HTTP fixture test: TM translate (gen_pgt + gen_pg) and DIM deploy/poll.

use beampipe_orchestration::{
    clients::{HttpDimClient, HttpTranslatorClient, TranslateConfig},
    dim::get_roots,
    DimClient, TranslatorClient,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn rest_translate_and_deploy_against_mock_tm_dim() {
    let tm = MockServer::start().await;
    let dim = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/gen_pgt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"<html><script>var pgtName = "test1_pgt.graph";</script></html>"#,
            ),
        )
        .mount(&tm)
        .await;

    Mock::given(method("GET"))
        .and(path("/gen_pg"))
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

use beampipe_orchestration::{
    clients::{HttpTranslatorClient, TranslateConfig},
    partitioned_pgt_for_dlg_deploy, TranslatorClient,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn slurm_translate_posts_unroll_and_partition_form() {
    let tm = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/unroll_and_partition"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"oid": "drop1"}])))
        .mount(&tm)
        .await;

    let client = HttpTranslatorClient::new(tm.uri());
    let config = TranslateConfig {
        algo: "metis".into(),
        num_par: 2,
        num_islands: 0,
        dim_host: String::new(),
        dim_port: 0,
        slurm_path: true,
    };
    let out = client
        .translate(json!({"nodeDataArray": []}), &config)
        .await
        .unwrap();
    let pgt = out.pgt_json.expect("pgt_json");
    let wrapped = partitioned_pgt_for_dlg_deploy(pgt, "beampipe.graph");
    assert!(wrapped.as_array().unwrap().len() == 2);
    assert!(wrapped[0].is_string());
    assert!(wrapped[1].is_array());
}

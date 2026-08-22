#[test]
fn openapi_spec_generates() {
    use utoipa::OpenApi;
    let spec = beampipe_api::ApiDoc::openapi();
    assert!(spec.paths.paths.len() > 10);
}

#[test]
fn openapi_uses_http_bearer_auth_for_json_login() {
    let spec = beampipe_api::export_openapi_json();
    assert_eq!(
        spec.pointer("/components/securitySchemes/BearerAuth/type")
            .and_then(serde_json::Value::as_str),
        Some("http")
    );
    assert_eq!(
        spec.pointer("/components/securitySchemes/BearerAuth/scheme")
            .and_then(serde_json::Value::as_str),
        Some("bearer")
    );
    assert!(spec
        .pointer("/components/securitySchemes/OAuth2PasswordBearer")
        .is_none());
}

#[test]
fn submission_abandonment_is_a_bearer_authenticated_post() {
    let spec = beampipe_api::export_openapi_json();
    let path = spec
        .pointer("/paths/~1api~1v2~1executions~1{id}~1submission~1abandon")
        .and_then(serde_json::Value::as_object)
        .expect("submission abandonment path");
    assert!(path.get("post").is_some());
    assert!(path.get("get").is_none());

    let operation = path.get("post").expect("POST operation");
    assert_eq!(
        operation.pointer("/security/0/BearerAuth"),
        Some(&serde_json::json!([]))
    );
    assert_eq!(
        operation.pointer("/requestBody/content/application~1json/schema/$ref"),
        Some(&serde_json::json!(
            "#/components/schemas/ExecutionSubmissionAbandonRequest"
        ))
    );
    assert!(operation.pointer("/responses/200").is_some());
    assert!(operation.pointer("/responses/403").is_some());
    assert!(operation.pointer("/responses/409").is_some());
    assert!(operation.pointer("/responses/429").is_some());
}

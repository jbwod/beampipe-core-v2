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

#[test]
fn publisher_capability_endpoints_are_bearer_authenticated_and_typed() {
    let spec = beampipe_api::export_openapi_json();
    let issuance = spec
        .pointer("/paths/~1api~1v2~1executions~1{id}~1outputs~1publisher-token/post")
        .expect("publisher-token POST operation");
    assert_eq!(
        issuance.pointer("/security/0/BearerAuth"),
        Some(&serde_json::json!([]))
    );
    assert_eq!(
        issuance.pointer("/requestBody/content/application~1json/schema/$ref"),
        Some(&serde_json::json!(
            "#/components/schemas/ExecutionPublisherTokenRequest"
        ))
    );
    assert_eq!(
        issuance.pointer("/responses/200/content/application~1json/schema/$ref"),
        Some(&serde_json::json!(
            "#/components/schemas/ExecutionPublisherTokenResponse"
        ))
    );

    let verify = spec
        .pointer("/paths/~1api~1v2~1executions~1{id}~1outputs~1verify/post")
        .expect("output verify POST operation");
    assert_eq!(
        verify.pointer("/security/0/BearerAuth"),
        Some(&serde_json::json!([]))
    );
    assert!(verify
        .get("description")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|description| description.contains("execution-scoped publisher")));
    let inventory = spec
        .pointer("/components/schemas/ExecutionOutputVerificationRequest")
        .expect("output verification request schema");
    assert_eq!(
        inventory.pointer("/properties/execution_attempt/type"),
        Some(&serde_json::json!("integer"))
    );
    assert!(inventory
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.contains(&serde_json::json!("execution_attempt"))));
}

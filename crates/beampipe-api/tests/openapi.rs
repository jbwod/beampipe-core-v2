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

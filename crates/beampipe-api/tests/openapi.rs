#[test]
fn openapi_spec_generates() {
    use utoipa::OpenApi;
    let spec = beampipe_api::ApiDoc::openapi();
    assert!(spec.paths.paths.len() > 10);
}

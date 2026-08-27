#[test]
fn every_success_response_has_content_and_resolvable_schemas() {
    const HTTP_METHODS: &[&str] = &["get", "post", "put", "patch", "delete"];

    fn assert_schema_refs_resolve(
        value: &serde_json::Value,
        schemas: &serde_json::Map<String, serde_json::Value>,
        operation: &str,
    ) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(serde_json::Value::as_str) {
                    if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                        assert!(
                            schemas.contains_key(name),
                            "{operation} references unregistered response schema {name}"
                        );
                    }
                }
                for child in object.values() {
                    assert_schema_refs_resolve(child, schemas, operation);
                }
            }
            serde_json::Value::Array(array) => {
                for child in array {
                    assert_schema_refs_resolve(child, schemas, operation);
                }
            }
            _ => {}
        }
    }

    let spec = beampipe_api::export_openapi_json();
    let schemas = spec
        .pointer("/components/schemas")
        .and_then(serde_json::Value::as_object)
        .expect("components.schemas");
    let paths = spec
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .expect("paths");

    for (path, path_item) in paths {
        let operations = path_item.as_object().expect("path item");
        for method in HTTP_METHODS {
            let Some(operation) = operations.get(*method) else {
                continue;
            };
            let operation_name = format!("{} {}", method.to_uppercase(), path);
            let responses = operation
                .get("responses")
                .and_then(serde_json::Value::as_object)
                .unwrap_or_else(|| panic!("{operation_name} has no responses"));
            let success_responses = responses
                .iter()
                .filter(|(status, _)| status.starts_with('2'))
                .collect::<Vec<_>>();
            assert!(
                !success_responses.is_empty(),
                "{operation_name} has no success response"
            );

            for (status, response) in success_responses {
                if status == "204" {
                    continue;
                }
                let content = response
                    .get("content")
                    .and_then(serde_json::Value::as_object)
                    .unwrap_or_else(|| {
                        panic!("{operation_name} {status} has no response content")
                    });
                assert!(
                    !content.is_empty(),
                    "{operation_name} {status} has empty response content"
                );
                for (media_type, media) in content {
                    assert!(
                        media.get("schema").is_some(),
                        "{operation_name} {status} {media_type} has no schema"
                    );
                    assert_schema_refs_resolve(media, schemas, &operation_name);
                }
            }
        }
    }

    for required in [
        "PaginatedExecutions",
        "ExecutionRead",
        "SourceRegistryRow",
        "DeploymentProfileResponse",
        "ProjectConfigRow",
    ] {
        assert!(
            schemas.contains_key(required),
            "generated-client response component {required} is not registered"
        );
    }

    let wasm_schema = spec
        .pointer(
            "/paths/~1api~1v2~1project-configs~1{id}~1wasm~1{sha256}/get/responses/200/content/application~1wasm/schema",
        )
        .expect("WASM download schema");
    assert_eq!(wasm_schema.get("type"), Some(&serde_json::json!("string")));
    assert_eq!(
        wasm_schema.get("format"),
        Some(&serde_json::json!("binary"))
    );
    assert!(
        wasm_schema.get("$ref").is_none(),
        "WASM downloads must not reuse the JSON metadata schema",
    );
}

#[test]
fn query_bearing_operations_publish_their_query_contracts() {
    use std::collections::BTreeSet;

    let spec = beampipe_api::export_openapi_json();
    let expected: &[(&str, &[&str])] = &[
        ("/api/v2/diagnostics", &["profile"]),
        ("/api/v2/workers", &["include_stopped"]),
        (
            "/api/v2/workers/leases",
            &["include_expired", "worker_id"],
        ),
        ("/api/v2/scheduler/status", &["profile"]),
        ("/api/v2/scheduler/jobs", &["limit", "offset"]),
        ("/api/v2/daliuge/inspect", &["profile"]),
        ("/api/v2/daliuge/sessions", &["profile"]),
        (
            "/api/v2/executions",
            &["items_per_page", "page", "project_module", "status"],
        ),
        (
            "/api/v2/executions/{id}/ledger-snapshot",
            &["include_manifest"],
        ),
        ("/api/v2/sources", &["limit", "offset", "project_module"]),
        (
            "/api/v2/sources/{id}/executions",
            &["limit", "offset"],
        ),
        (
            "/api/v2/executions/{id}/observations",
            &["limit", "offset"],
        ),
        (
            "/api/v2/project-configs/{id}/wasm/{sha256}",
            &["download"],
        ),
        ("/api/v2/alert-deliveries", &["limit"]),
        ("/api/v2/executions/{id}/events", &["limit"]),
        ("/api/v2/sources/{id}/events", &["limit"]),
        (
            "/api/v2/projects/{module}/events",
            &["limit", "offset"],
        ),
    ];

    for (path, expected_names) in expected {
        let operation = spec
            .pointer(&format!("/paths/{}/get", path.replace('/', "~1")))
            .unwrap_or_else(|| panic!("GET {path} operation"));
        let query_parameters = operation
            .get("parameters")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|parameter| {
                parameter.get("in").and_then(serde_json::Value::as_str) == Some("query")
            })
            .collect::<Vec<_>>();
        let actual_names = query_parameters
            .iter()
            .filter_map(|parameter| parameter.get("name").and_then(serde_json::Value::as_str))
            .collect::<BTreeSet<_>>();
        let expected_names = expected_names.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(actual_names, expected_names, "GET {path} query parameters");
        assert!(
            query_parameters.iter().all(|parameter| {
                parameter
                    .get("required")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
            }),
            "GET {path} query parameters must remain optional",
        );
    }
}

#[test]
fn operation_parameters_are_unique_and_cover_path_placeholders() {
    use std::collections::{BTreeMap, BTreeSet};

    const HTTP_METHODS: &[&str] = &["get", "post", "put", "patch", "delete"];
    let spec = beampipe_api::export_openapi_json();
    let paths = spec
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .expect("paths");

    for (path, path_item) in paths {
        let placeholders = path
            .split('/')
            .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
            .collect::<BTreeSet<_>>();
        let operations = path_item.as_object().expect("path item");
        for method in HTTP_METHODS {
            let Some(operation) = operations.get(*method) else {
                continue;
            };
            let operation_name = format!("{} {}", method.to_uppercase(), path);
            let parameters = operation
                .get("parameters")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut unique = BTreeSet::new();
            let mut path_parameters = BTreeMap::new();
            for parameter in parameters {
                let parameter_in = parameter
                    .get("in")
                    .and_then(serde_json::Value::as_str)
                    .expect("parameter.in");
                let name = parameter
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .expect("parameter.name");
                assert!(
                    unique.insert((parameter_in, name)),
                    "{operation_name} duplicates {parameter_in} parameter {name}",
                );
                if parameter_in == "path" {
                    path_parameters.insert(
                        name,
                        parameter
                            .get("required")
                            .and_then(serde_json::Value::as_bool),
                    );
                }
            }

            assert_eq!(
                path_parameters.keys().copied().collect::<BTreeSet<_>>(),
                placeholders,
                "{operation_name} path parameters must match route placeholders",
            );
            assert!(
                path_parameters.values().all(|required| *required == Some(true)),
                "{operation_name} path parameters must be required",
            );
        }
    }
}

#[test]
fn execution_prepare_source_preview_has_a_concrete_contract() {
    use std::collections::BTreeSet;

    let spec = beampipe_api::export_openapi_json();
    assert_eq!(
        spec.pointer(
            "/components/schemas/ExecutionPrepareResponse/properties/sources_preview/items/$ref",
        )
        .and_then(serde_json::Value::as_str),
        Some("#/components/schemas/ExecutionPrepareSourcePreview"),
    );
    let preview = spec
        .pointer("/components/schemas/ExecutionPrepareSourcePreview")
        .expect("ExecutionPrepareSourcePreview schema");
    let properties = preview
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("preview properties");
    assert_eq!(
        properties.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        ["source_identifier", "group_count", "record_count"]
            .into_iter()
            .collect(),
    );
    assert_eq!(
        preview
            .get("required")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<BTreeSet<_>>(),
        ["source_identifier", "group_count", "record_count"]
            .into_iter()
            .collect(),
    );
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
fn publisher_callback_and_token_endpoints_are_absent() {
    let spec = beampipe_api::export_openapi_json();
    assert!(spec
        .pointer("/paths/~1api~1v2~1executions~1{id}~1outputs~1publisher-token")
        .is_none());
    assert!(spec
        .pointer("/paths/~1api~1v2~1executions~1{id}~1outputs~1verify")
        .is_none());
    for schema in [
        "ExecutionPublisherTokenRequest",
        "ExecutionPublisherTokenResponse",
        "ExecutionOutputVerificationRequest",
        "ExecutionOutputVerificationResponse",
    ] {
        assert!(spec
            .pointer(&format!("/components/schemas/{schema}"))
            .is_none());
    }
}

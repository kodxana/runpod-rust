//! Provision-time secret and registry reference validation.
use runpod::{
    Client, ClientConfig,
    apps::{AppsApiClient, registry::resolve_registry_auth, validate_secrets},
    json,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn client(server: &MockServer) -> AppsApiClient {
    AppsApiClient::new(
        Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn test_no_references_no_api_call() {
    let server = MockServer::start().await;
    let api = client(&server);
    validate_secrets(&[], &api).await.unwrap();
    assert_eq!(resolve_registry_auth(None, &api).await.unwrap(), None);
    assert_eq!(resolve_registry_auth(Some(""), &api).await.unwrap(), None);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_existing_pass_and_missing_raises_with_hint() {
    let server = MockServer::start().await;
    let api = client(&server);
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"myself":{"secrets":[{"name":"existing"}]}}})),
        )
        .expect(3)
        .mount(&server)
        .await;
    validate_secrets(&["existing".into()], &api).await.unwrap();
    assert_eq!(
        validate_secrets(&["z".into(), "a".into(), "z".into()], &api)
            .await
            .unwrap_err()
            .to_string(),
        "secrets not found: a, z. create with `rp secret add <name>`"
    );
    assert_eq!(
        validate_secrets(&["one".into()], &api)
            .await
            .unwrap_err()
            .to_string(),
        "secret not found: one. create with `rp secret add <name>`"
    );
}

#[tokio::test]
async fn test_registry_auth_resolution_and_error_hints() {
    let server = MockServer::start().await;
    let api = client(&server);
    Mock::given(path("/v2/registries"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"registries":[{"id":"b-id","name":"b"},{"id":"a-id","name":"a"}]}),
        ))
        .expect(3)
        .mount(&server)
        .await;
    assert_eq!(
        resolve_registry_auth(Some("a"), &api).await.unwrap(),
        Some("a-id".into())
    );
    assert_eq!(
        resolve_registry_auth(Some("b-id"), &api).await.unwrap(),
        Some("b-id".into())
    );
    assert_eq!(
        resolve_registry_auth(Some("missing"), &api)
            .await
            .unwrap_err()
            .to_string(),
        "registry credential 'missing' not found. available: a, b. create one with `rp registry add missing`"
    );
}

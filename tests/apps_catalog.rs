//! Ports of test_apps/test_gpu_catalog.py cache, cancellation, and authentication behavior.
use runpod::{Client, ClientConfig, apps::AppsApiClient, json};
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, path},
};

fn client(server: &MockServer, key: &str) -> AppsApiClient {
    AppsApiClient::new(
        Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new(key)
        })
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn test_cache_is_scoped_to_credentials_and_catalog_url() {
    let server = MockServer::start().await;
    for key in ["first", "second"] {
        Mock::given(path("/v2/catalog/gpus"))
            .and(header("authorization", format!("Bearer {key}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"gpus":[{"id":key,"pool":"AMPERE_24"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let first = client(&server, "first");
    let second = client(&server, "second");
    let (a, b) = tokio::join!(first.gpu_pool_members(), first.gpu_pool_members());
    assert_eq!(a, b);
    assert!(a.unwrap()["AMPERE_24"].contains("first"));
    assert!(second.gpu_pool_members().await.unwrap()["AMPERE_24"].contains("second"));
    assert!(first.clone().gpu_pool_members().await.unwrap()["AMPERE_24"].contains("first"));
}

#[tokio::test]
async fn test_failed_catalog_retries_after_cooldown() {
    for response in [
        ResponseTemplate::new(500),
        ResponseTemplate::new(200).set_body_json(json!({"gpus":[]})),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/v2/catalog/gpus"))
            .respond_with(response)
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/v2/catalog/gpus"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"gpus":[{"id":"MIG","pool":"AMPERE_24"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server, "key");
        assert!(c.gpu_pool_members().await.is_none());
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(c.gpu_pool_members().await.is_none());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::time::resume();
        assert!(c.gpu_pool_members().await.unwrap()["AMPERE_24"].contains("MIG"));
    }
}

#[tokio::test]
async fn test_catalog_wait_respects_provisioning_deadline() {
    let server = MockServer::start().await;
    Mock::given(path("/v2/catalog/gpus"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"gpus":[{"id":"L4","pool":"AMPERE_24"}]}))
                .set_delay(Duration::from_millis(80)),
        )
        .expect(2)
        .mount(&server)
        .await;
    let c = client(&server, "key");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), c.gpu_pool_members())
            .await
            .is_err()
    );
    // Cancellation must release the in-flight lock without caching a failure.
    assert!(c.gpu_pool_members().await.unwrap()["AMPERE_24"].contains("L4"));
}

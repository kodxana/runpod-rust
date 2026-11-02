use runpod::{apps::task_runner::PodTermination, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, path},
};

fn values(base: &str, kind: &str) -> BTreeMap<String, String> {
    [
        ("RUNPOD_API_BASE_URL", base),
        ("RUNPOD_POD_ID", "pod-1"),
        ("RUNPOD_API_KEY", "secret"),
        ("RUNPOD_RUNTIME_KIND", kind),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}

#[tokio::test]
async fn test_production_self_termination_uses_pod_auth() {
    for acknowledgment in [json!(true), json!(false), json!(null)] {
        let server = MockServer::start().await;
        Mock::given(path("/graphql")).and(header("Authorization","Bearer secret")).and(header("X-RunPod-Pod-Id","pod-1"))
            .and(body_json(json!({"query":"mutation flashTaskTerminate($input: FlashTaskTerminateInput!) { flashTaskTerminate(input: $input) }","variables":{"input":{"podId":"pod-1"}}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"flashTaskTerminate":acknowledgment}})))
            .expect(1).mount(&server).await;
        let deletion = PodTermination::from_values(&values(&server.uri(), "task")).unwrap();
        assert!(!format!("{deletion:?}").contains("secret"));
        assert_eq!(deletion.terminate().await.unwrap(), acknowledgment == true);
    }
}

#[tokio::test]
async fn test_deletion_requires_acknowledgment_and_recovers() {
    let server = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(path("/graphql"))
        .respond_with(
            move |_: &wiremock::Request| match calls.fetch_add(1, Ordering::SeqCst) {
                0 => ResponseTemplate::new(200).set_body_json(
                    json!({"errors":[{"message":"denied"}],"data":{"podTerminate":null}}),
                ),
                1 | 2 => ResponseTemplate::new(503),
                _ => {
                    ResponseTemplate::new(200).set_body_json(json!({"data":{"podTerminate":null}}))
                }
            },
        )
        .expect(4)
        .mount(&server)
        .await;
    let deletion = PodTermination::from_values(&values(&server.uri(), "queue")).unwrap();
    assert!(!deletion.terminate().await.unwrap());
    assert!(deletion.terminate().await.unwrap());
    for request in server.received_requests().await.unwrap() {
        assert!(!request.headers.contains_key("X-RunPod-Pod-Id"));
    }
}

#[tokio::test]
async fn missing_credentials_never_send_and_retry_budget_is_bounded() {
    let server = MockServer::start().await;
    let mut config = values(&server.uri(), "task");
    config.remove("RUNPOD_API_KEY");
    assert!(
        !PodTermination::from_values(&config)
            .unwrap()
            .terminate()
            .await
            .unwrap()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"errors":[{"extensions":{"code":"TIMEOUT"}}]})),
        )
        .expect(3)
        .mount(&server)
        .await;
    assert!(
        !PodTermination::from_values(&values(&server.uri(), "task"))
            .unwrap()
            .terminate()
            .await
            .unwrap()
    );
}

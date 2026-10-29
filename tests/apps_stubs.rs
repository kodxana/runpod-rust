use futures_util::StreamExt;
use runpod::{
    Client, ClientConfig,
    apps::{Api, Queue, ResourceRef, StubConfig, targets::JobOptions},
    json,
};
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn config() -> StubConfig {
    let mut config = StubConfig::new("other-app", ResourceRef::Name("chat".into()));
    config.environment = Some("production".into());
    config
}

#[test]
fn debug_does_not_disclose_bound_storage_credentials() {
    let queue = Queue::new(Client::new("token-secret").unwrap(), config())
        .unwrap()
        .with_options(&JobOptions {
            s3_config: Some(json!({"accessKeyId":"never-log-this"})),
            webhook: Some("https://hook?key=never-log-this".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(!format!("{queue:?}").contains("never-log-this"));
    assert!(!format!("{queue:?}").contains("token-secret"));
}
fn client(server: &MockServer) -> Client {
    Client::with_config(ClientConfig {
        endpoint_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap()
}

#[tokio::test]
async fn stub_options_accumulate_without_changing_original() {
    let server = MockServer::start().await;
    for payload in [
        json!({"input":{"__empty":true}}),
        json!({"input":{"prompt":"hi"},"webhook":"https://callback.example","policy":{"executionTimeout":0,"ttl":30,"lowPriority":false}}),
    ] {
        Mock::given(method("POST"))
            .and(path("/flash/runsync"))
            .and(header("X-Flash-App", "other-app"))
            .and(header("X-Flash-Environment", "production"))
            .and(header("X-Flash-Endpoint", "chat"))
            .and(body_json(payload))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"status":"COMPLETED","output":42})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let original = Queue::new(client(&server), config()).unwrap();
    let configured = original
        .with_options(&JobOptions {
            webhook: Some("https://callback.example".into()),
            execution_timeout: Some(0),
            ..Default::default()
        })
        .unwrap()
        .with_options(&JobOptions {
            ttl: Some(30),
            low_priority: Some(false),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        original.remote(&serde_json::Map::new()).await.unwrap(),
        json!(42)
    );
    assert_eq!(
        configured
            .remote(json!({"prompt":"hi"}).as_object().unwrap())
            .await
            .unwrap(),
        json!(42)
    );
}

#[tokio::test]
async fn stub_spawn_reconnect_and_lazy_stream_use_sentinel() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/run"))
        .and(header("X-Flash-Endpoint", "ep-id"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/flash/status/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","status":"COMPLETED","output":"finished"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/stream/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"COMPLETED","stream":[{"output":"chunk"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut cfg = config();
    cfg.resource = ResourceRef::Id("ep-id".into());
    let queue = Queue::new(client(&server), cfg).unwrap();
    let job = queue.spawn(&serde_json::Map::new()).await.unwrap();
    assert_eq!(job.id(), "job");
    assert!(!job.done());
    let mut resumed = queue.job(job.id());
    assert_eq!(resumed.status().await.unwrap(), "COMPLETED");
    assert_eq!(
        resumed.result(Duration::ZERO).await.unwrap(),
        json!("finished")
    );
    let mut stream = queue.stream(serde_json::Map::new());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert_eq!(stream.next().await.unwrap().unwrap(), json!("chunk"));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn api_stub_routes_all_verbs_with_headers_and_json_or_text() {
    let server = MockServer::start().await;
    for verb in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
        Mock::given(method(verb))
            .and(path("/route"))
            .and(header("X-Flash-App", "other-app"))
            .and(header("X-Flash-Endpoint", "chat"))
            .and(body_json(json!({"hello":"world"})))
            .respond_with(ResponseTemplate::new(200).set_body_string("plain text"))
            .expect(1)
            .mount(&server)
            .await;
    }
    let mut api = Api::new(client(&server), config()).unwrap();
    api.set_load_balancer_url(server.uri()).unwrap();
    let body = json!({"hello":"world"});
    let body = Some(&body);
    assert_eq!(api.get("/route", body).await.unwrap(), json!("plain text"));
    assert_eq!(api.post("/route", body).await.unwrap(), json!("plain text"));
    assert_eq!(api.put("/route", body).await.unwrap(), json!("plain text"));
    assert_eq!(
        api.delete("/route", body).await.unwrap(),
        json!("plain text")
    );
    assert_eq!(
        api.patch("/route", body).await.unwrap(),
        json!("plain text")
    );
}

#[test]
fn blocking_stubs_and_jobs_share_runtime_after_client_is_dropped() {
    let server_runtime = tokio::runtime::Runtime::new().unwrap();
    let server =
        server_runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(path("/flash/run"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"id":"job","status":"COMPLETED","output":42})),
                )
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(path("/flash/stream/job"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"status":"COMPLETED","stream":[{"output":1},{"output":2}]}),
                ))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(path("/route"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
                .expect(1)
                .mount(&server)
                .await;
            server
        });
    let uri = server.uri();
    let client = runpod::blocking::Client::with_config(ClientConfig {
        endpoint_base_url: uri.clone(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    let queue = client.queue(config()).unwrap();
    let mut api = client.api(config()).unwrap();
    api.set_load_balancer_url(uri).unwrap();
    drop(client);
    let mut job = queue.spawn(&serde_json::Map::new()).unwrap();
    drop(queue);
    assert_eq!(job.result(Duration::ZERO).unwrap(), json!(42));
    assert_eq!(job.status().unwrap(), "COMPLETED");
    assert_eq!(
        job.stream(Duration::from_secs(5))
            .collect::<runpod::Result<Vec<_>>>()
            .unwrap(),
        vec![json!(1), json!(2)]
    );
    assert_eq!(api.get("/route", None).unwrap(), json!({"ok":true}));
}

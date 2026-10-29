//! Python endpoint runner contracts, including the SDK's unusual envelope rules.
use futures_util::StreamExt;
use runpod::{Client, ClientConfig, Error, Job, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

async fn setup() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = Client::with_config(ClientConfig {
        endpoint_base_url: format!("{}/v2", server.uri()),
        poll_interval: Duration::from_millis(1),
        ..ClientConfig::new("key")
    })
    .unwrap();
    (server, client)
}

#[tokio::test]
async fn test_endpoint_run() {
    let (server, client) = setup().await;
    for input in [
        json!({"prompt":"hello"}),
        json!({"input":{}}),
        json!({"input":false}),
        json!({"input":{"prompt":"hello"},"webhook":"url"}),
    ] {
        let expected = if input["input"].as_object().is_some_and(|v| !v.is_empty()) {
            input.clone()
        } else {
            json!({"input":input})
        };
        Mock::given(method("POST"))
            .and(path("/v2/endpoint/run"))
            .and(body_json(expected))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            client.endpoint("endpoint").run(input).await.unwrap().id(),
            "job"
        );
        server.verify().await;
        server.reset().await;
    }
}

#[tokio::test]
async fn test_endpoint_run_sync() {
    let (server, client) = setup().await;
    Mock::given(path("/v2/endpoint/runsync"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"COMPLETED","output":{"result":"ok"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client
            .endpoint("endpoint")
            .run_sync(json!({}), Duration::from_secs(1))
            .await
            .unwrap(),
        json!({"result":"ok"})
    );
}

#[tokio::test]
async fn test_run_sync_with_timeout() {
    let (server, client) = setup().await;
    Mock::given(path("/v2/endpoint/runsync"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_PROGRESS"})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/v2/endpoint/status/job"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"IN_PROGRESS"})))
        .mount(&server)
        .await;
    assert!(matches!(
        client
            .endpoint("endpoint")
            .run_sync(json!({}), Duration::from_millis(50))
            .await,
        Err(Error::Timeout)
    ));
}

#[tokio::test]
async fn test_job_status() {
    let (server, client) = setup().await;
    let hits = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/v2/endpoint/status/job"))
        .respond_with(move |_: &wiremock::Request| {
            let first = hits.fetch_add(1, Ordering::SeqCst) == 0;
            ResponseTemplate::new(200).set_body_json(
                json!({"status":if first {"IN_PROGRESS"} else {"COMPLETED"},"output":"answer"}),
            )
        })
        .expect(2)
        .mount(&server)
        .await;
    let mut job = Job::new("endpoint", "job", client);
    assert_eq!(job.status().await.unwrap(), "IN_PROGRESS");
    assert_eq!(job.status().await.unwrap(), "COMPLETED");
    assert_eq!(job.status().await.unwrap(), "COMPLETED");
    assert_eq!(job.output(Duration::ZERO).await.unwrap(), json!("answer"));
}

#[tokio::test]
async fn test_output_with_sleep() {
    let (server, client) = setup().await;
    let hits = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .respond_with(move |_: &wiremock::Request| {
            let first = hits.fetch_add(1, Ordering::SeqCst) == 0;
            ResponseTemplate::new(200).set_body_json(if first {
                json!({"status":"IN_PROGRESS"})
            } else {
                json!({"status":"COMPLETED","output":false})
            })
        })
        .expect(2)
        .mount(&server)
        .await;
    assert_eq!(
        Job::new("endpoint", "job", client)
            .output(Duration::from_secs(1))
            .await
            .unwrap(),
        json!(false)
    );
}

#[tokio::test]
async fn test_job_stream() {
    let (server, client) = setup().await;
    let hits = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/v2/endpoint/stream/job"))
        .respond_with(move |_: &wiremock::Request| {
            let page = hits.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_json(match page {
                0 => json!({"status":"IN_PROGRESS","stream":[{"output":"one"},{"output":"two"}]}),
                1 => json!({"status":"COMPLETED","stream":[{"output":"last"}]}),
                _ => json!({"status":"COMPLETED","stream":[]}),
            })
        })
        .expect(3)
        .mount(&server)
        .await;
    let mut job = Job::new("endpoint", "job", client);
    let result: Vec<_> = Box::pin(job.stream()).map(|r| r.unwrap()).collect().await;
    assert_eq!(result, vec![json!("one"), json!("two"), json!("last")]);
}

#[tokio::test]
async fn test_cancel_health_and_purge_queue() {
    let (server, client) = setup().await;
    for (verb, route) in [
        ("POST", "/v2/endpoint/cancel/job"),
        ("POST", "/v2/endpoint/purge-queue"),
        ("GET", "/v2/endpoint/health"),
    ] {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
            .expect(1)
            .mount(&server)
            .await;
    }
    assert_eq!(
        Job::new("endpoint", "job", client.clone())
            .cancel()
            .await
            .unwrap(),
        json!({"ok":true})
    );
    assert_eq!(
        client.endpoint("endpoint").health().await.unwrap(),
        json!({"ok":true})
    );
    assert_eq!(
        client.endpoint("endpoint").purge_queue().await.unwrap(),
        json!({"ok":true})
    );
}

#[tokio::test]
async fn test_blocking_client_and_stream() {
    let (server, _) = setup().await;
    let hits = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/v2/endpoint/run"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"job"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/endpoint/stream/job"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                json!({"status":"IN_PROGRESS","stream":[{"output":1}]})
            } else {
                json!({"status":"COMPLETED","stream":[]})
            })
        })
        .expect(2)
        .mount(&server)
        .await;
    let base = format!("{}/v2", server.uri());
    let (done, completion) = tokio::sync::oneshot::channel();
    let thread = std::thread::spawn(move || {
        let client = runpod::blocking::Client::with_config(ClientConfig {
            endpoint_base_url: base,
            poll_interval: Duration::from_millis(1),
            ..ClientConfig::new("key")
        })
        .unwrap();
        let mut job = client.endpoint("endpoint").run(json!({})).unwrap();
        assert_eq!(
            job.stream().collect::<runpod::Result<Vec<_>>>().unwrap(),
            vec![json!(1)]
        );
        done.send(()).unwrap();
    });
    completion.await.unwrap();
    thread.join().unwrap();
}

#[tokio::test]
async fn test_asyncio_envelope_request_id_and_zero_timeout() {
    use wiremock::matchers::header;
    let (server, client) = setup().await;
    Mock::given(path("/v2/endpoint/run"))
        .and(body_json(json!({"input":{"input":{"x":1}}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"job"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/endpoint/status/job"))
        .and(header("X-Request-ID", "job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"COMPLETED","output":"ready"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut job = runpod::AsyncioEndpoint::new("endpoint", client)
        .run(json!({"input":{"x":1}}))
        .await
        .unwrap();
    assert!(matches!(
        job.output(Duration::ZERO).await,
        Err(Error::Timeout)
    ));
    assert_eq!(job.status().await.unwrap(), "COMPLETED");
    assert_eq!(job.output(Duration::ZERO).await.unwrap(), json!("ready"));
}

#[test]
fn clients_and_jobs_are_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<Client>();
    check::<Job>();
    check::<runpod::Endpoint>();
    check::<runpod::blocking::Client>();
}

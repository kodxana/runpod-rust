//! Queue invocation, submitted jobs, live responses, and source synchronization.
use futures_util::StreamExt;
use runpod::{
    Client, ClientConfig,
    apps::{
        AppError, Job, ResourceKind, ResourceSpec,
        serialization::FunctionSource,
        targets::{FunctionDefinition, ResourceAddress, Target},
    },
    json,
};
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

fn target(server: &MockServer, live: bool) -> Target {
    let client = Client::with_config(ClientConfig {
        endpoint_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    if live {
        Target::live(client, "ep123", "chat").unwrap()
    } else {
        Target::sentinel(client, ResourceAddress::new("demo", "production", "chat")).unwrap()
    }
}

#[test]
fn load_balancer_endpoint_ids_cannot_change_the_service_host() {
    let client = Client::new("key").unwrap();
    for id in [
        "",
        "attacker.example/path?",
        "user@attacker.example/",
        "a\\host",
        "../other",
        "a#fragment",
    ] {
        assert!(
            Target::live(client.clone(), id, "chat").is_err(),
            "accepted {id}"
        );
    }
}

#[test]
fn target_debug_does_not_disclose_source_or_environment() {
    let mut target = Target::live(Client::new("key").unwrap(), "ep123", "chat").unwrap();
    let mut spec = ResourceSpec::new(ResourceKind::Api, "chat");
    spec.env = Some(
        json!({"SECRET":"never-log-this"})
            .as_object()
            .unwrap()
            .clone(),
    );
    target
        .attach_source(
            FunctionSource::Text("never-log-this".into()),
            "chat",
            Some(spec),
        )
        .unwrap();
    assert!(!format!("{target:?}").contains("never-log-this"));
}

#[tokio::test]
async fn test_fast_invocation_returns_without_polling() {
    for live in [false, true] {
        let server = MockServer::start().await;
        let endpoint = if live { "ep123" } else { "flash" };
        let mut mock = Mock::given(method("POST"))
            .and(path(format!("/{endpoint}/runsync")))
            .and(query_param("wait", "9000"))
            .and(header("authorization", "Bearer key"));
        if !live {
            mock = mock
                .and(header("X-Flash-App", "demo"))
                .and(header("X-Flash-Environment", "production"))
                .and(header("X-Flash-Endpoint", "chat"));
        }
        mock.respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"id":"sync-job","status":"COMPLETED","output":{"answer":42}}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
        assert_eq!(
            target(&server, live)
                .invoke(&json!({"input":{}}), Duration::from_secs(10))
                .await
                .unwrap(),
            json!({"answer":42})
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_sync_wait_hands_off_the_same_job() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/flash/runsync"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"same","status":"IN_QUEUE"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flash/status/same"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"same","status":"COMPLETED","output":42})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        target(&server, false)
            .invoke(&json!({"input":{}}), Duration::from_secs(10))
            .await
            .unwrap(),
        json!(42)
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}

#[tokio::test]
async fn test_submission_is_not_repeated_after_ambiguous_failure() {
    for route in ["run", "runsync", "retry/job"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/flash/{route}")))
            .respond_with(ResponseTemplate::new(503).set_body_string("response lost"))
            .expect(1)
            .mount(&server)
            .await;
        let target = target(&server, false);
        let result = match route {
            "run" => target.submit(&json!({"input":{}})).await,
            "runsync" => {
                target
                    .invoke(&json!({"input":{}}), Duration::from_secs(1))
                    .await
            }
            _ => target.retry_job("job").await,
        };
        assert_eq!(result.unwrap_err().status_code(), Some(503));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_job_404_retried_during_propagation() {
    for route in ["status/job", "cancel/job"] {
        let server = MockServer::start().await;
        Mock::given(path(format!("/flash/{route}")))
            .respond_with(ResponseTemplate::new(404))
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(path(format!("/flash/{route}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"CANCELLED"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let target = target(&server, false);
        let data = if route.starts_with("status") {
            target.job_status("job").await
        } else {
            target.cancel_job("job").await
        };
        assert_eq!(data.unwrap()["status"], "CANCELLED");
    }
}

#[tokio::test]
async fn test_404_not_retried_maps_to_endpoint_not_found() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/run"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        matches!(target(&server,false).submit(&json!({"input":{}})).await,Err(runpod::Error::App(AppError::EndpointNotFound {app_name,resource_name})) if app_name=="demo" && resource_name=="chat")
    );
}

#[tokio::test]
async fn test_job_cancel_and_retry_update_state() {
    let server = MockServer::start().await;
    for (route, status) in [
        ("status", "IN_PROGRESS"),
        ("cancel", "CANCELLED"),
        ("retry", "IN_QUEUE"),
    ] {
        Mock::given(path(format!("/flash/{route}/job")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":status})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let mut job = Job::new(
        json!({"id":"job","status":"UNKNOWN"}),
        target(&server, false),
    )
    .unwrap();
    assert_eq!(job.status().await.unwrap(), "IN_PROGRESS");
    job.cancel().await.unwrap();
    assert!(job.done());
    assert_eq!(job.status().await.unwrap(), "CANCELLED");
    job.retry().await.unwrap();
    assert!(!job.done());
    assert_eq!(job.id(), "job");
}

#[tokio::test]
async fn test_job_result_reaches_terminal_state_and_output_alias() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/status/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","status":"COMPLETED","output":{"answer":42}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut job = Job::new(
        json!({"id":"job","status":"IN_QUEUE"}),
        target(&server, false),
    )
    .unwrap();
    assert_eq!(
        job.result(Duration::from_secs(10)).await.unwrap(),
        json!({"answer":42})
    );
    assert!(job.done());
    assert_eq!(job.status().await.unwrap(), "COMPLETED");
    assert_eq!(
        job.output(Duration::ZERO).await.unwrap(),
        json!({"answer":42})
    );
}

#[tokio::test]
async fn test_stream_job_unwraps_chunks_and_stops_on_terminal_page() {
    for live in [false, true] {
        let server = MockServer::start().await;
        let endpoint = if live { "ep123" } else { "flash" };
        let chunks = if live {
            json!([{"output":{"success":true,"__stream__":true,"json_result":"a"}},{"output":{"success":true,"__stream__":true,"json_result":"b"}}])
        } else {
            json!([{"output":"a"},{"output":"b"}])
        };
        Mock::given(path(format!("/{endpoint}/stream/job")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"status":"CANCELLED","stream":chunks})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut stream = target(&server, live).stream_job("job", Duration::from_secs(10));
        assert_eq!(stream.next().await.unwrap().unwrap(), json!("a"));
        assert_eq!(stream.next().await.unwrap().unwrap(), json!("b"));
        assert!(stream.next().await.is_none());
    }
}

#[tokio::test]
async fn test_stream_failed_and_wait_missing_id_or_timeout() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/stream/job"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"status":"FAILED","error":"boom"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let target = target(&server, false);
    assert!(
        target
            .stream_job("job", Duration::from_secs(1))
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("boom")
    );
    assert!(
        target
            .wait(json!({"status":"IN_QUEUE"}), Duration::from_secs(1), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("missing a job id")
    );
    assert!(
        target
            .wait(
                json!({"id":"job","status":"IN_QUEUE"}),
                Duration::ZERO,
                None
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("did not complete")
    );
}

#[tokio::test]
async fn test_live_payload_and_concurrent_requests_share_completed_source_sync() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/_runpod/sync")).and(body_json(json!({"source":"module source","resource":"chat","dependencies":["dep"],"system_dependencies":null})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"synced"})).set_delay(Duration::from_millis(50))).expect(1).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/calculate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":6})))
        .expect(2)
        .mount(&server)
        .await;
    let mut target = target(&server, true);
    target.set_load_balancer_url(server.uri()).unwrap();
    let mut spec = ResourceSpec::new(ResourceKind::Api, "chat");
    spec.dependencies = Some(vec!["dep".into()]);
    target
        .attach_source(
            FunctionSource::Text("module source".into()),
            "chat",
            Some(spec.clone()),
        )
        .unwrap();
    let function = FunctionDefinition {
        name: "fn".into(),
        parameters: vec!["prompt".into()],
        source: Some(FunctionSource::Text("module source".into())),
    };
    let payload = target
        .build_payload(&function, &spec, &[json!("hi")], &serde_json::Map::new())
        .await
        .unwrap();
    assert_eq!(payload["input"]["args"], json!(["hi"]));
    assert_eq!(payload["input"]["serialization_format"], "json");
    assert_eq!(payload["input"]["function_code"], "module source");
    let body = json!({"value":3});
    let (a, b) = tokio::join!(
        target.request("POST", "/calculate", Some(&body), Duration::from_secs(10)),
        target.request("POST", "/calculate", Some(&body), Duration::from_secs(10))
    );
    assert_eq!(a.unwrap(), json!({"result":6}));
    assert_eq!(b.unwrap(), json!({"result":6}));
    assert_eq!(
        server.received_requests().await.unwrap()[0].url.path(),
        "/_runpod/sync"
    );
}

#[tokio::test]
async fn test_failed_source_sync_prevents_execution_and_can_retry() {
    let server = MockServer::start().await;
    Mock::given(path("/_runpod/sync"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/_runpod/sync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"synced"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/calculate"))
        .respond_with(ResponseTemplate::new(200).set_body_string("plain text"))
        .expect(1)
        .mount(&server)
        .await;
    let mut target = target(&server, true);
    target.set_load_balancer_url(server.uri()).unwrap();
    target
        .attach_source(FunctionSource::Text("module source".into()), "chat", None)
        .unwrap();
    assert_eq!(
        target
            .request("GET", "/calculate", None, Duration::from_secs(10))
            .await
            .unwrap_err()
            .status_code(),
        Some(503)
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(
        target
            .request("GET", "/calculate", None, Duration::from_secs(10))
            .await
            .unwrap(),
        json!("plain text")
    );
}

#[tokio::test]
async fn persistent_job_failures_exhaust_seven_attempts() {
    let server = MockServer::start().await;
    for (id, status) in [("missing", 404), ("unavailable", 503)] {
        Mock::given(path(format!("/flash/status/{id}")))
            .respond_with(ResponseTemplate::new(status))
            .expect(7)
            .mount(&server)
            .await;
    }
    let target = target(&server, false);
    let (missing, unavailable) = tokio::join!(
        target.job_status("missing"),
        target.job_status("unavailable")
    );
    assert!(matches!(
        missing,
        Err(runpod::Error::App(AppError::EndpointNotFound { .. }))
    ));
    assert_eq!(unavailable.unwrap_err().status_code(), Some(503));
}

#[tokio::test]
async fn transient_reads_retry_but_client_errors_and_invalid_json_do_not() {
    let server = MockServer::start().await;
    for status in [520, 502] {
        Mock::given(path("/flash/status/retry"))
            .respond_with(ResponseTemplate::new(status))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/flash/status/retry"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"COMPLETED"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/status/invalid"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{bad", "application/json"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/status/bad-request"))
        .respond_with(ResponseTemplate::new(400))
        .expect(1)
        .mount(&server)
        .await;
    let target = target(&server, false);
    assert_eq!(
        target.job_status("retry").await.unwrap()["status"],
        "COMPLETED"
    );
    assert!(target.job_status("invalid").await.is_err());
    assert_eq!(
        target
            .job_status("bad-request")
            .await
            .unwrap_err()
            .status_code(),
        Some(400)
    );
}

#[tokio::test]
async fn status_observers_see_admission_and_completion_and_can_stop_polling() {
    let server = MockServer::start().await;
    Mock::given(path("/ep123/runsync"))
        .and(query_param("wait", "1000"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/ep123/status/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","status":"COMPLETED","output":42})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::with_config(ClientConfig {
        endpoint_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    let queue = runpod::apps::targets::QueueClient::new(client, "ep123").unwrap();
    let mut states = Vec::new();
    let mut observe = |v: &runpod::Value| {
        states.push(v["status"].clone());
        Ok(())
    };
    queue
        .invoke(
            &json!({"input":{}}),
            Duration::from_secs(5),
            Some(&mut observe),
        )
        .await
        .unwrap();
    assert_eq!(states, vec![json!("IN_QUEUE"), json!("COMPLETED")]);
    let mut reject =
        |_: &runpod::Value| Err(runpod::Error::InvalidInput("observer stopped".into()));
    assert!(
        queue
            .wait(
                json!({"id":"job","status":"IN_QUEUE"}),
                Duration::from_secs(5),
                Some(&mut reject)
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("observer stopped")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn failed_result_remains_cached_and_zero_stream_timeout_makes_no_request() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/status/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","status":"FAILED","error":"worker failed"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let target = target(&server, false);
    let mut job = Job::new(json!({"id":"job","status":"IN_QUEUE"}), target.clone()).unwrap();
    assert!(
        job.result(Duration::from_secs(5))
            .await
            .unwrap_err()
            .to_string()
            .contains("worker failed")
    );
    assert!(job.done());
    assert_eq!(job.status().await.unwrap(), "FAILED");
    assert!(job.output(Duration::ZERO).await.is_err());
    assert!(matches!(
        target
            .stream_job("job", Duration::ZERO)
            .next()
            .await
            .unwrap(),
        Err(runpod::Error::DeadlineExceeded(_))
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn changed_source_reuploads_and_dependency_only_change_preserves_source_cache() {
    let server = MockServer::start().await;
    Mock::given(path("/_runpod/sync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/route"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
        .expect(3)
        .mount(&server)
        .await;
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("module.py");
    std::fs::write(&source, "first source").unwrap();
    let mut target = target(&server, true);
    target.set_load_balancer_url(server.uri()).unwrap();
    target
        .attach_source(FunctionSource::File(source.clone()), "chat", None)
        .unwrap();
    target
        .request("GET", "/route", None, Duration::from_secs(5))
        .await
        .unwrap();
    std::fs::write(&source, "second source").unwrap();
    target
        .request("GET", "/route", None, Duration::from_secs(5))
        .await
        .unwrap();
    let mut spec = ResourceSpec::new(ResourceKind::Api, "chat");
    spec.dependencies = Some(vec!["changed".into()]);
    target
        .attach_source(FunctionSource::File(source), "chat", Some(spec))
        .unwrap();
    target
        .request("GET", "/route", None, Duration::from_secs(5))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let sources: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/_runpod/sync")
        .map(|r| r.body_json::<runpod::Value>().unwrap()["source"].clone())
        .collect();
    assert_eq!(sources, vec![json!("first source"), json!("second source")]);
}

#[tokio::test]
async fn cancelled_source_sync_releases_lock_and_does_not_cache_success() {
    let server = MockServer::start().await;
    let received = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify = received.clone();
    Mock::given(path("/_runpod/sync"))
        .respond_with(move |_: &wiremock::Request| {
            notify.notify_one();
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok":true}))
                .set_delay(Duration::from_secs(20))
        })
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/_runpod/sync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/route"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
        .expect(1)
        .mount(&server)
        .await;
    let mut target = target(&server, true);
    target.set_load_balancer_url(server.uri()).unwrap();
    target
        .attach_source(FunctionSource::Text("source".into()), "chat", None)
        .unwrap();
    let cancelled_target = target.clone();
    let task = tokio::spawn(async move {
        cancelled_target
            .request("GET", "/route", None, Duration::from_secs(60))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), received.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        target
            .request("GET", "/route", None, Duration::from_secs(5))
            .await
            .unwrap(),
        json!({"ok":true})
    );
}

#[tokio::test]
async fn sync_transport_timeout_does_not_resubmit() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/runsync"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"COMPLETED"}))
                .set_delay(Duration::from_secs(30)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = target(&server, false)
        .invoke(&json!({"input":{}}), Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(matches!(error,runpod::Error::Http(error) if error.is_timeout()));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn disconnected_submissions_are_not_replayed() {
    use tokio::io::AsyncReadExt;
    for operation in ["run", "runsync", "retry"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = connection.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
                if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .unwrap_or("0")
                        .trim()
                        .parse()
                        .unwrap();
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            // Replayed requests would get a different (connect) failure after this disconnect.
            drop(listener);
            drop(connection);
        });
        let client = Client::with_config(ClientConfig {
            endpoint_base_url: url,
            ..ClientConfig::new("key")
        })
        .unwrap();
        let target = Target::sentinel(client, ResourceAddress::new("a", "default", "q")).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(5), async {
            match operation {
                "run" => target.submit(&json!({"input":{}})).await,
                "runsync" => {
                    target
                        .invoke(&json!({"input":{}}), Duration::from_secs(2))
                        .await
                }
                _ => target.retry_job("job").await,
            }
        })
        .await
        .unwrap()
        .unwrap_err();
        assert!(matches!(error,runpod::Error::Http(error) if !error.is_connect()));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn zero_http_timeout_disables_transport_deadline_but_not_job_wait_deadline() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/runsync"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"COMPLETED","output":42}))
                .set_delay(Duration::from_millis(20)),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/route"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!(42))
                .set_delay(Duration::from_millis(20)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut target = target(&server, false);
    target.set_load_balancer_url(server.uri()).unwrap();
    assert_eq!(
        target
            .invoke(&json!({"input":{}}), Duration::ZERO)
            .await
            .unwrap(),
        json!(42)
    );
    assert_eq!(
        target
            .request("GET", "/route", None, Duration::ZERO)
            .await
            .unwrap(),
        json!(42)
    );
    assert!(matches!(
        target
            .wait(
                json!({"id":"job","status":"IN_QUEUE"}),
                Duration::ZERO,
                None
            )
            .await,
        Err(runpod::Error::DeadlineExceeded(_))
    ));
}

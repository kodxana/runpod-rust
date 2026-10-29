use runpod::{
    Client, ClientConfig, Value,
    apps::{
        AppsApiClient,
        events::EventSink,
        logs::{LogsClient, tail_summary},
        monitor::{PodLogStream, WorkerMonitor, filter_line, format_worker_counts, line_before},
    },
    json,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{path, query_param},
};

type Recorded = Arc<Mutex<Vec<(String, Vec<Value>)>>>;
fn sink() -> (
    EventSink,
    Recorded,
    tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = events.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (
        EventSink::new(move |name, args| {
            record.lock().unwrap().push((name.into(), args.to_vec()));
            let _ = tx.send(name.into());
            Ok(())
        }),
        events,
        rx,
    )
}
fn clients(server: &MockServer) -> (AppsApiClient, LogsClient) {
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    (
        AppsApiClient::new(client.clone()).unwrap(),
        LogsClient::with_hapi_base(client, server.uri()).unwrap(),
    )
}

#[test]
fn python_monitor_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/monitor.json")).unwrap();
    for case in cases {
        let input = &case["input"];
        let result = match case["operation"].as_str().unwrap() {
            "filter" => json!(filter_line(input.as_str().unwrap()).unwrap()),
            "format" => json!(format_worker_counts(input.as_object().unwrap())),
            "tail" => {
                json!(tail_summary(&input["logs"], input["lines"].as_i64().unwrap()).unwrap())
            }
            "before" => json!(
                line_before(
                    input["line"].as_str().unwrap(),
                    input["since"].as_str().unwrap()
                )
                .unwrap()
            ),
            "counts" => {
                let (sink, events, _) = sink();
                let client = Client::new("key").unwrap();
                let monitor = WorkerMonitor::new(
                    "ep",
                    "resource",
                    sink,
                    AppsApiClient::new(client.clone()).unwrap(),
                    LogsClient::new(client).unwrap(),
                );
                for data in input.as_array().unwrap() {
                    monitor.report_counts(data);
                }
                let events = events.lock().unwrap();
                json!(events.iter().map(|(_, args)| args).collect::<Vec<_>>())
            }
            other => panic!("unknown fixture {other}"),
        };
        assert_eq!(result, case["output"], "{case}");
    }
}

#[tokio::test]
async fn follow_emits_deduplicates_and_attach_is_idempotent() {
    let server = MockServer::start().await;
    let frames = "data: {\"ts\":\"t1\",\"line\":\"hello\"}\n\ndata: {\"ts\":\"t1\",\"line\":\"hello\"}\n\ndata: {\"ts\":\"t2\",\"line\":\"\"}\n\n";
    Mock::given(path("/v2/serverless/ep/workers/pod/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("data: {\"ts\":\"t1\",\"line\":\"hello\"}\n\ndata: {\"ts\":\"t3\",\"line\":\"world\"}\n\n","text/event-stream"))
        .up_to_n_times(1).expect(1).mount(&server).await;
    Mock::given(path("/v2/serverless/ep/workers/pod/logs"))
        .and(query_param("tail", "1000"))
        .and(query_param("source", "container"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(frames, "text/event-stream"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/serverless/ep/workers/pod/logs"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .mount(&server)
        .await;
    let (_, logs) = clients(&server);
    let (sink, events, mut ready) = sink();
    let mut stream = PodLogStream::new(logs, "pod", "calc", sink, Some("ep".into()));
    stream.attach().unwrap();
    stream.attach().unwrap();
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(5), ready.recv())
            .await
            .unwrap()
            .unwrap();
    }
    stream.stop().await;
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            ("worker_log".into(), vec![json!("calc"), json!("hello")]),
            ("worker_log".into(), vec![json!("calc"), json!("world")])
        ]
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.url.path().starts_with("/v2/"))
    );
}

#[tokio::test]
async fn stop_snapshots_without_a_stream_and_filters_old_and_runtime_lines() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/pod/pod/logs")).and(query_param("type","container")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"container":["2000-01-01T00:00:00.000000000Z stale","2099-01-01T00:00:00Z recovered","2099-01-01T00:00:00Z {\"message\":\"noise\",\"level\":\"INFO\"}","2099-01-01T00:00:00Z recovered"]}))).expect(2).mount(&server).await;
    let (_, logs) = clients(&server);
    let (sink, events, _) = sink();
    let mut stream = PodLogStream::new(logs, "pod", "calc", sink, None);
    stream.stop().await;
    stream.stop().await;
    assert_eq!(
        *events.lock().unwrap(),
        vec![("worker_log".into(), vec![json!("calc"), json!("recovered")])]
    );
}

#[tokio::test]
async fn monitor_reports_counts_attaches_each_worker_once_and_stops_pool_updates() {
    let server = MockServer::start().await;
    Mock::given(path("/v2/serverless/ep/workers"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"summary":{"initializing":1,"idle":2}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/pod/worker/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"container":[]})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/serverless/ep/workers/worker/logs"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let (api, logs) = clients(&server);
    let (sink, events, mut ready) = sink();
    let mut monitor = WorkerMonitor::new("ep", "calc", sink, api, logs);
    monitor.start().unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), ready.recv())
            .await
            .unwrap()
            .unwrap(),
        "worker_status"
    );
    monitor.on_status(&json!({"workerId":"worker"})).unwrap();
    monitor.on_status(&json!({"workerId":"worker"})).unwrap();
    monitor.report_counts(&json!({"summary":{"unhealthy":10}}));
    monitor.stop().await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].1[1]["ready"], 2);
    assert_eq!(
        events[1],
        ("worker_ready".into(), vec![json!("calc"), json!("worker")])
    );
}

#[tokio::test]
async fn live_invocation_recovers_logs_and_orders_completion_and_failure_events() {
    use runpod::apps::targets::Target;
    for failed in [false, true] {
        let server = MockServer::start().await;
        let reply = if failed {
            json!({"id":"job","workerId":"worker","status":"FAILED","error":"execution failed"})
        } else {
            json!({"id":"job","workerId":"worker","status":"COMPLETED","output":{"success":true,"json_result":42}})
        };
        Mock::given(path("/ep/runsync"))
            .and(query_param("wait", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(reply))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/v2/serverless/ep/workers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"summary":{"idle":1}})))
            .mount(&server)
            .await;
        Mock::given(path("/v2/serverless/ep/workers/worker/logs"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        Mock::given(path("/v1/pod/worker/logs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"container":["2099-01-01T00:00:00Z recovered output"]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (api, logs) = clients(&server);
        let (sink, events, _) = sink();
        let client = Client::with_config(ClientConfig {
            endpoint_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap();
        let mut target = Target::live(client, "ep", "calc").unwrap();
        target.set_events(sink).unwrap();
        target.set_monitor_clients(api, logs).unwrap();
        let result = target
            .invoke(&json!({"input":{}}), Duration::from_secs(5))
            .await;
        if failed {
            assert!(result.unwrap_err().to_string().contains("execution failed"));
        } else {
            assert_eq!(result.unwrap(), json!(42));
        }
        let events = events.lock().unwrap();
        let names: Vec<_> = events.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            if failed {
                vec!["dispatch", "worker_ready", "request_failed", "worker_log"]
            } else {
                vec![
                    "dispatch",
                    "worker_ready",
                    "worker_log",
                    "request_completed",
                ]
            }
        );
    }
}

#[tokio::test]
async fn monitor_stop_preserves_worker_assignment_order_for_snapshots() {
    let server = MockServer::start().await;
    for worker in ["second", "first"] {
        Mock::given(path(format!("/v1/pod/{worker}/logs")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"container":[format!("2099-01-01T00:00:00Z {worker}")]})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let (api, logs) = clients(&server);
    let (sink, events, _) = sink();
    let mut monitor = WorkerMonitor::new("ep", "calc", sink, api, logs);
    for worker in ["second", "first"] {
        monitor.on_status(&json!({"workerId":worker})).unwrap();
    }
    monitor.stop().await;
    let logs: Vec<_> = events
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name == "worker_log")
        .map(|(_, args)| args[1].clone())
        .collect();
    assert_eq!(logs, vec![json!("second"), json!("first")]);
}

#[tokio::test]
async fn canceled_invocation_aborts_its_active_log_connection() {
    use runpod::apps::targets::Target;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stream_server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        let mut request = Vec::new();
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"line\":\"attached\",\"ts\":\"t1\"}\n\n").await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    });
    let server = MockServer::start().await;
    Mock::given(path("/ep/runsync"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","workerId":"worker","status":"IN_PROGRESS"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/serverless/ep/workers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(path("/ep/status/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","workerId":"worker","status":"IN_PROGRESS"})),
        )
        .mount(&server)
        .await;
    let (api, _) = clients(&server);
    let logs = LogsClient::with_hapi_base(
        Client::with_config(ClientConfig {
            api_base_url: url.clone(),
            ..ClientConfig::new("key")
        })
        .unwrap(),
        url,
    )
    .unwrap();
    let (sink, _, mut ready) = sink();
    let mut target = Target::live(
        Client::with_config(ClientConfig {
            endpoint_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap(),
        "ep",
        "calc",
    )
    .unwrap();
    target.set_events(sink).unwrap();
    target.set_monitor_clients(api, logs).unwrap();
    let invocation = tokio::spawn(async move {
        target
            .invoke(&json!({"input":{}}), Duration::from_secs(60))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while ready.recv().await.as_deref() != Some("worker_log") {}
    })
    .await
    .unwrap();
    invocation.abort();
    assert!(invocation.await.unwrap_err().is_cancelled());
    stream_server.await.unwrap();
}

#[tokio::test]
async fn gateway_retry_and_hidden_runtime_lines_preserve_snapshot_suppression() {
    let server = MockServer::start().await;
    Mock::given(path("/v2/pods/pod/logs"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    let frame = json!({"line":"{\"message\":\"noise\",\"level\":\"INFO\"}","ts":"t1"});
    Mock::given(path("/v2/pods/pod/logs"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(format!("data: {frame}\n\n"), "text/event-stream"),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    let reconnected = Arc::new(tokio::sync::Notify::new());
    let notify = reconnected.clone();
    Mock::given(path("/v2/pods/pod/logs"))
        .respond_with(move |_: &wiremock::Request| {
            notify.notify_one();
            ResponseTemplate::new(200).set_delay(Duration::from_secs(60))
        })
        .expect(1)
        .mount(&server)
        .await;
    let (_, logs) = clients(&server);
    let (sink, events, _) = sink();
    let mut stream = PodLogStream::new(logs, "pod", "calc", sink, None);
    stream.attach().unwrap();
    tokio::time::timeout(Duration::from_secs(5), reconnected.notified())
        .await
        .unwrap();
    stream.stop().await;
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn snapshot_failure_does_not_fail_monitor_shutdown() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/pod/pod/logs"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let (_, logs) = clients(&server);
    let (sink, events, _) = sink();
    let mut stream = PodLogStream::new(logs, "pod", "calc", sink, None);
    stream.stop().await;
    assert!(events.lock().unwrap().is_empty());
}

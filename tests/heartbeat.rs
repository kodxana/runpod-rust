use runpod::{
    json,
    serverless::{Heartbeat, HeartbeatOptions, PingJobMirror, WorkerConfig},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

fn config(server: &MockServer) -> WorkerConfig {
    WorkerConfig {
        worker_id: "worker".into(),
        api_key: "test-key".into(),
        ping_url: Some(format!("{}/ping/$RUNPOD_POD_ID", server.uri())),
        ping_interval: Duration::from_millis(20),
        ..WorkerConfig::default()
    }
}

async fn wait_requests(server: &MockServer, count: usize) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while server.received_requests().await.unwrap().len() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("heartbeat requests arrived");
}

#[test]
fn test_heartbeat_initialization() {
    let values = BTreeMap::from([
        ("RUNPOD_POD_ID".into(), "worker".into()),
        ("RUNPOD_AI_API_KEY".into(), "test-key".into()),
        ("RUNPOD_PING_INTERVAL".into(), "5999".into()),
        (
            "RUNPOD_WEBHOOK_PING".into(),
            "http://localhost/$RUNPOD_POD_ID".into(),
        ),
    ]);
    let heartbeat = Heartbeat::new(WorkerConfig::from_values(&values).unwrap()).unwrap();
    assert_eq!(heartbeat.config().ping_interval, Duration::from_secs(5));
    assert!(!format!("{heartbeat:?}").contains("test-key"));
}

#[test]
fn test_heartbeat_initialization_defaults() {
    let hb = Heartbeat::new(WorkerConfig::from_values(&BTreeMap::new()).unwrap()).unwrap();
    assert_eq!(hb.config().ping_interval, Duration::from_secs(10));
    assert!(hb.config().ping_url.is_none());
    assert!(hb.config().api_key.is_empty());
}

#[tokio::test]
async fn test_start_ping_missing_api_key() {
    let server = MockServer::start().await;
    let mut cfg = config(&server);
    cfg.api_key.clear();
    assert!(
        Heartbeat::new(cfg)
            .unwrap()
            .start_ping(None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_start_ping_missing_pod_id() {
    let server = MockServer::start().await;
    let values = BTreeMap::from([
        ("RUNPOD_AI_API_KEY".into(), "test-key".into()),
        ("RUNPOD_WEBHOOK_PING".into(), server.uri()),
    ]);
    assert!(
        Heartbeat::new(WorkerConfig::from_values(&values).unwrap())
            .unwrap()
            .start_ping(None)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_start_ping_missing_webhook_url() {
    for url in [None, Some(String::new()), Some("PING_NOT_SET".into())] {
        let cfg = WorkerConfig {
            api_key: "key".into(),
            ping_url: url,
            ..WorkerConfig::default()
        };
        assert!(
            Heartbeat::new(cfg)
                .unwrap()
                .start_ping(None)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn test_start_ping_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let handle = Heartbeat::new(config(&server))
        .unwrap()
        .start_ping(None)
        .await
        .unwrap()
        .unwrap();
    wait_requests(&server, 1).await;
    handle.stop().await.unwrap();
    let count = server.received_requests().await.unwrap().len();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(server.received_requests().await.unwrap().len(), count);
}

#[tokio::test]
async fn test_start_ping_already_started() {
    let server = MockServer::start().await;
    let mut hb = Heartbeat::new(config(&server)).unwrap();
    let handle = hb.start_ping(None).await.unwrap().unwrap();
    assert!(hb.start_ping(None).await.unwrap().is_none());
    handle.stop().await.unwrap();
    assert!(
        hb.start_ping(None).await.unwrap().is_none(),
        "start-once is per instance"
    );
}

#[tokio::test]
async fn test_process_loop() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let mirror = PingJobMirror::new();
    mirror.set(Some("job1,job2"));
    let handle = Heartbeat::new(config(&server))
        .unwrap()
        .start_ping(Some(mirror))
        .await
        .unwrap()
        .unwrap();
    wait_requests(&server, 1).await;
    handle.stop().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[0]
            .url
            .query_pairs()
            .any(|(k, v)| k == "job_id" && v == "job1,job2")
    );
}

#[tokio::test]
async fn test_ping_loop_test_mode() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let now = tokio::time::Instant::now();
    Heartbeat::new(config(&server))
        .unwrap()
        .run_once(None)
        .await
        .unwrap();
    assert!(
        now.elapsed() >= Duration::from_millis(20),
        "single iteration includes its sleep"
    );
}

#[tokio::test]
async fn test_ping_loop_continuous() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Heartbeat::new(config(&server))
        .unwrap()
        .ping_loop(None, wait_requests(&server, 3))
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().len() >= 3);
}

#[tokio::test]
async fn test_send_ping_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mirror = PingJobMirror::new();
    mirror.set(Some("job1,job2,job3"));
    assert_eq!(
        Heartbeat::new(config(&server))
            .unwrap()
            .send_ping(Some(&mirror))
            .await
            .unwrap(),
        Some(200)
    );
    let requests = server.received_requests().await.unwrap();
    let r = &requests[0];
    assert_eq!(r.url.path(), "/ping/worker");
    assert_eq!(r.headers["authorization"], "test-key");
    let params: BTreeMap<_, _> = r.url.query_pairs().into_owned().collect();
    assert_eq!(params["job_id"], "job1,job2,job3");
    assert_eq!(params["runpod_version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn test_send_ping_no_jobs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Heartbeat::new(config(&server))
        .unwrap()
        .send_ping(None)
        .await
        .unwrap();
    assert!(
        !server.received_requests().await.unwrap()[0]
            .url
            .query_pairs()
            .any(|(k, _)| k == "job_id")
    );
}

#[tokio::test]
async fn test_send_ping_request_exception() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let cfg = WorkerConfig {
        api_key: "key".into(),
        ping_url: Some(url),
        ping_interval: Duration::from_millis(1),
        ..WorkerConfig::default()
    };
    let hb = Heartbeat::with_options(
        cfg,
        HeartbeatOptions {
            retries: 0,
            ..HeartbeatOptions::default()
        },
    )
    .unwrap();
    assert!(hb.send_ping(None).await.is_err());
    hb.run_once(None)
        .await
        .expect("loop logs request errors and still sleeps");
}

#[test]
fn test_custom_pool_connections() {
    let options = HeartbeatOptions {
        pool_connections: 20,
        retries: 5,
    };
    let hb = Heartbeat::with_options(WorkerConfig::default(), options).unwrap();
    assert_eq!(hb.options(), options);
}

#[tokio::test]
async fn test_http_adapter_configuration() {
    let fixture: runpod::Value =
        serde_json::from_str(include_str!("fixtures/heartbeat.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let server = MockServer::start().await;
        let responses = case["responses"].as_array().unwrap().clone();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        Mock::given(method("GET"))
            .respond_with(move |_: &wiremock::Request| {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                let response = &responses[n.min(responses.len() - 1)];
                let mut template =
                    ResponseTemplate::new(u16::try_from(response[0].as_u64().unwrap()).unwrap());
                if let Some(delay) = response[1].as_str() {
                    template = template.insert_header("Retry-After", delay);
                }
                template
            })
            .mount(&server)
            .await;
        let mut cfg = config(&server);
        cfg.ping_interval = Duration::from_secs(5);
        let mirror = PingJobMirror::new();
        mirror.set(Some("job 1,job/2"));
        let started = tokio::time::Instant::now();
        let result = Heartbeat::new(cfg).unwrap().send_ping(Some(&mirror)).await;
        assert_eq!(
            result.is_err(),
            case["failed"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            case["requests"].as_array().unwrap().len(),
            "{}",
            case["name"]
        );
        let delay: f64 = case["delays"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .sum();
        assert!(
            started.elapsed() >= Duration::from_secs_f64(delay),
            "{} backoff",
            case["name"]
        );
        for request in server.received_requests().await.unwrap() {
            let mut query: BTreeMap<_, _> = request
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), vec![v.into_owned()]))
                .collect();
            query.insert("runpod_version".into(), vec!["VERSION".into()]);
            assert_eq!(
                json!({"path": request.url.path(), "query": query, "authorization": request.headers["authorization"].to_str().unwrap()}),
                case["requests"][0]
            );
        }
    }
}

#[tokio::test]
async fn response_header_timeouts_retry_and_exhausted_errors_redact_url() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    Mock::given(method("GET"))
        .respond_with(move |_: &wiremock::Request| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_delay(Duration::from_secs(2))
            } else {
                ResponseTemplate::new(200)
            }
        })
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.ping_interval = Duration::from_millis(100);
    let hb = Heartbeat::new(cfg).unwrap();
    assert_eq!(hb.send_ping(None).await.unwrap(), Some(200));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.ping_url = Some(format!("{}/?secret=never-log", server.uri()));
    let hb = Heartbeat::with_options(
        cfg,
        HeartbeatOptions {
            retries: 0,
            ..HeartbeatOptions::default()
        },
    )
    .unwrap();
    let error = hb.send_ping(None).await.unwrap_err();
    assert_eq!(error.status_code(), Some(503));
    assert!(!error.to_string().contains("never-log"));
}

#[test]
fn malformed_and_subsecond_environment_intervals_fail_before_starting() {
    for value in ["bad", "-1"] {
        assert!(
            WorkerConfig::from_values(&BTreeMap::from([(
                "RUNPOD_PING_INTERVAL".into(),
                value.into()
            )]))
            .is_err()
        );
    }
    let cfg = WorkerConfig::from_values(&BTreeMap::from([(
        "RUNPOD_PING_INTERVAL".into(),
        "999".into(),
    )]))
    .unwrap();
    assert!(
        Heartbeat::new(cfg).is_err(),
        "source rounds below one second to an unusable zero timeout"
    );
}

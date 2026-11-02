use runpod::{
    apps::bootstrap::{PhaseError, QueueErrorReporter, serve_api_error},
    json,
};
use std::{collections::BTreeMap, time::Duration};
use tokio::{net::TcpListener, sync::oneshot};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[test]
fn test_queue_error_loop_exits_without_webhooks() {
    for values in [
        BTreeMap::new(),
        BTreeMap::from([(
            "RUNPOD_WEBHOOK_GET_JOB".into(),
            "http://localhost/get".into(),
        )]),
        BTreeMap::from([(
            "RUNPOD_WEBHOOK_POST_OUTPUT".into(),
            "http://localhost/out/$ID".into(),
        )]),
    ] {
        assert!(
            QueueErrorReporter::from_values(&values)
                .unwrap_err()
                .to_string()
                .contains("no job webhook env")
        );
    }
}

#[tokio::test]
async fn test_queue_error_loop_reports_jobs() {
    let server = MockServer::start().await;
    let (sent, received) = oneshot::channel();
    let sent = std::sync::Mutex::new(Some(sent));
    Mock::given(method("GET"))
        .and(path("/get"))
        .and(header("Authorization", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"job1"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/out/job1"))
        .and(header("Authorization", "secret"))
        .and(header("Content-Type", "application/x-www-form-urlencoded"))
        .respond_with(move |request: &wiremock::Request| {
            let body: runpod::Value = serde_json::from_slice(&request.body).unwrap();
            let inner: runpod::Value =
                serde_json::from_str(body["error"].as_str().unwrap()).unwrap();
            assert_eq!(inner, PhaseError::new("attach", "manifest gone").payload());
            if let Some(sent) = sent.lock().unwrap().take() {
                let _ = sent.send(());
            }
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    let values = BTreeMap::from([
        (
            "RUNPOD_WEBHOOK_GET_JOB".into(),
            format!("{}/get", server.uri()),
        ),
        (
            "RUNPOD_WEBHOOK_POST_OUTPUT".into(),
            format!("{}/out/$ID", server.uri()),
        ),
        ("RUNPOD_AI_API_KEY".into(), "secret".into()),
    ]);
    let reporter = QueueErrorReporter::from_values(&values).unwrap();
    assert!(!format!("{reporter:?}").contains("secret"));
    tokio::time::timeout(
        Duration::from_secs(3),
        reporter.run(&PhaseError::new("attach", "manifest gone"), async {
            received.await.unwrap();
        }),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn api_error_responder_answers_all_supported_methods_without_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async {
        serve_api_error(
            listener,
            &PhaseError::new("locate", "artifact missing"),
            async {
                let _ = stopped.await;
            },
        )
        .await
    });
    let client = reqwest::Client::new();
    for method in [
        reqwest::Method::GET,
        reqwest::Method::POST,
        reqwest::Method::PUT,
        reqwest::Method::DELETE,
        reqwest::Method::PATCH,
    ] {
        let response = client
            .request(method.clone(), format!("{url}/arbitrary?query=yes"))
            .body("not JSON")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 500, "{method}");
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(
            response.json::<runpod::Value>().await.unwrap(),
            PhaseError::new("locate", "artifact missing").payload()
        );
    }
    for method in [reqwest::Method::HEAD, reqwest::Method::OPTIONS] {
        assert_eq!(
            client.request(method, &url).send().await.unwrap().status(),
            501
        );
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn api_shutdown_closes_an_incomplete_request() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async {
        serve_api_error(
            listener,
            &PhaseError::new("attach", "bad manifest"),
            async {
                let _ = stopped.await;
            },
        )
        .await
    });
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    socket
        .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100000\r\n\r\n{")
        .await
        .unwrap();
    // Receiving headers proves the server accepted this connection before shutdown.
    let mut response = [0; 1024];
    let count = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&response[..count]).starts_with("HTTP/1.1 500"));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn dropping_api_server_closes_accepted_connections() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct DropNotice(Option<oneshot::Sender<()>>);
    impl Drop for DropNotice {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (notice, dropped) = oneshot::channel();
    let guard = DropNotice(Some(notice));
    let task = tokio::spawn(async {
        serve_api_error(
            listener,
            &PhaseError::new("attach", "missing"),
            async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            },
        )
        .await
    });
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10000\r\n\r\n{")
        .await
        .unwrap();
    let mut header = [0; 1024];
    let count = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut header))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&header[..count]).starts_with("HTTP/1.1 500"));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), dropped)
            .await
            .is_ok(),
        "dropping the server leaked its shutdown future"
    );
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut rest)).await;
    assert!(
        closed.is_ok(),
        "dropping the server left its connection alive"
    );
}

#[tokio::test]
async fn shutdown_cancels_an_outstanding_queue_poll() {
    let server = MockServer::start().await;
    let (started, polling) = oneshot::channel();
    let started = std::sync::Mutex::new(Some(started));
    Mock::given(path("/get"))
        .respond_with(move |_: &wiremock::Request| {
            if let Some(started) = started.lock().unwrap().take() {
                let _ = started.send(());
            }
            ResponseTemplate::new(200).set_delay(Duration::from_secs(90))
        })
        .mount(&server)
        .await;
    let reporter = QueueErrorReporter::from_values(&BTreeMap::from([
        (
            "RUNPOD_WEBHOOK_GET_JOB".into(),
            format!("{}/get", server.uri()),
        ),
        (
            "RUNPOD_WEBHOOK_POST_OUTPUT".into(),
            format!("{}/out/$ID", server.uri()),
        ),
    ]))
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        reporter.run(&PhaseError::new("locate", "missing"), async {
            polling.await.unwrap();
        }),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn queue_retries_failures_and_non_200_polls_before_reporting() {
    let server = MockServer::start().await;
    let times = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls = times.clone();
    Mock::given(path("/get"))
        .respond_with(move |_: &wiremock::Request| {
            let mut calls = calls.lock().unwrap();
            calls.push(std::time::Instant::now());
            match calls.len() {
                1 => ResponseTemplate::new(503),
                2 => ResponseTemplate::new(202),
                _ => ResponseTemplate::new(200).set_body_json(json!({"id":"recovered"})),
            }
        })
        .mount(&server)
        .await;
    let (sent, done) = oneshot::channel();
    let sent = std::sync::Mutex::new(Some(sent));
    Mock::given(path("/out/recovered"))
        .respond_with(move |_: &wiremock::Request| {
            if let Some(sent) = sent.lock().unwrap().take() {
                let _ = sent.send(());
            }
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    let reporter = QueueErrorReporter::from_values(&BTreeMap::from([
        (
            "RUNPOD_WEBHOOK_GET_JOB".into(),
            format!("{}/get", server.uri()),
        ),
        (
            "RUNPOD_WEBHOOK_POST_OUTPUT".into(),
            format!("{}/out/$ID", server.uri()),
        ),
    ]))
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(12),
        reporter.run(&PhaseError::new("system", "package unavailable"), async {
            done.await.unwrap();
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let times = times.lock().unwrap();
    assert!(times[1].duration_since(times[0]) >= Duration::from_millis(4900));
    assert!(times[2].duration_since(times[1]) >= Duration::from_millis(1900));
}

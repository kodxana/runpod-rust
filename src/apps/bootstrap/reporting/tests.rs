use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn source_queue_error_cycles() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/bootstrap_reporting.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if case["network_error"] == true {
            continue; // Exercised with real request timeouts below.
        }
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/get"))
            .respond_with(
                ResponseTemplate::new(u16::try_from(case["get_status"].as_u64().unwrap()).unwrap())
                    .set_body_string(case["body"].as_str().unwrap()),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(
                u16::try_from(case["post_status"].as_u64().unwrap()).unwrap(),
            ))
            .mount(&server)
            .await;
        let reporter = QueueErrorReporter::from_values(&BTreeMap::from([
            (
                "RUNPOD_WEBHOOK_GET_JOB".into(),
                format!("{}/get", server.uri()),
            ),
            (
                "RUNPOD_WEBHOOK_POST_OUTPUT".into(),
                format!("{}/out/$ID/again/$ID", server.uri()),
            ),
            ("RUNPOD_AI_API_KEY".into(), "fixture-secret".into()),
        ]))
        .unwrap();
        let payload = serde_json::to_vec(
            &json!({"error":PhaseError::new("attach","manifest missing").payload().to_string()}),
        )
        .unwrap();
        let delay = reporter
            .cycle(&payload)
            .await
            .unwrap_or(Duration::from_secs(5));
        let expected = case["delays"]
            .as_array()
            .unwrap()
            .first()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        assert_eq!(delay.as_secs(), expected, "{name}");
        let requests = server.received_requests().await.unwrap();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(requests.len(), expected.len(), "{name}");
        for (request, expected) in requests.iter().zip(expected) {
            assert_eq!(
                request.method.as_str(),
                expected["method"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(
                request.url.path(),
                expected["url"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("http://fixture")
                    .unwrap(),
                "{name}"
            );
            for (key, value) in expected["headers"].as_object().unwrap() {
                assert_eq!(
                    request.headers[key.as_str()].to_str().unwrap(),
                    value.as_str().unwrap(),
                    "{name}: {key}"
                );
            }
            if let Some(body) = expected.get("body") {
                let mut actual: Value = serde_json::from_slice(&request.body).unwrap();
                actual["error"] = serde_json::from_str(actual["error"].as_str().unwrap()).unwrap();
                assert_eq!(&actual, body, "{name}");
            }
        }
    }
}

#[test]
fn test_report_error_routes_by_kind() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/bootstrap_reporting.json"
    ))
    .unwrap();
    for case in fixture["routes"].as_array().unwrap() {
        let route = ErrorRoute::from_kind(case["kind"].as_str());
        let expected = match route {
            ErrorRoute::Queue => "queue",
            ErrorRoute::Api => "api",
            ErrorRoute::Task => "task",
        };
        assert_eq!(expected, case["route"].as_str().unwrap(), "{case}");
        assert_eq!(
            PhaseError::new("locate", "missing").payload(),
            case["payload"]
        );
    }
}

#[tokio::test]
async fn transport_failure_is_retryable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let close = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        drop(socket);
    });
    let reporter = QueueErrorReporter::from_values(&BTreeMap::from([
        (
            "RUNPOD_WEBHOOK_GET_JOB".into(),
            format!("http://{address}/get"),
        ),
        (
            "RUNPOD_WEBHOOK_POST_OUTPUT".into(),
            format!("http://{address}/out/$ID"),
        ),
    ]))
    .unwrap();
    assert!(matches!(reporter.cycle(b"{}").await, Err(Error::Http(_))));
    close.await.unwrap();
}

#[tokio::test]
async fn request_timeouts_match_source() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/bootstrap_reporting.json"
    ))
    .unwrap();
    for post in [false, true] {
        let server = MockServer::start().await;
        let (sent, started) = tokio::sync::oneshot::channel();
        let sent = std::sync::Mutex::new(Some(sent));
        let delayed = if post { "POST" } else { "GET" };
        Mock::given(method(delayed))
            .respond_with(move |_: &wiremock::Request| {
                if let Some(sent) = sent.lock().unwrap().take() {
                    let _ = sent.send(());
                }
                ResponseTemplate::new(200).set_delay(Duration::from_secs(120))
            })
            .mount(&server)
            .await;
        if post {
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"job"})))
                .mount(&server)
                .await;
        }
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
        let began = tokio::time::Instant::now();
        let request = tokio::spawn(async move { reporter.cycle(b"{}").await });
        tokio::time::timeout(Duration::from_secs(2), started)
            .await
            .unwrap()
            .unwrap();
        let seconds = fixture["cases"][0]["requests"][usize::from(post)]["timeout"]
            .as_u64()
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(seconds - 1)).await;
        tokio::task::yield_now().await;
        assert!(!request.is_finished(), "{delayed} timed out too soon");
        tokio::time::advance(Duration::from_secs(2)).await;
        let error = request.await.unwrap().unwrap_err();
        assert!(
            began.elapsed() < Duration::from_secs(seconds + 2),
            "{delayed} exceeded its timeout"
        );
        assert!(
            matches!(error,Error::Http(error) if error.is_timeout()),
            "{delayed}"
        );
        tokio::time::resume();
    }
}

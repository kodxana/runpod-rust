use runpod::{
    Error, Value, json,
    serverless::{Handler, WorkerApi, WorkerConfig},
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::{net::TcpListener, sync::oneshot};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const FIXED_ID: &str = "test-12345678-1234-4234-8234-123456789abc";

#[tokio::test]
async fn webhook_boolean_result_redirects_and_failure() {
    use runpod::serverless::send_webhook;
    use wiremock::matchers::path;
    let server = MockServer::start().await;
    Mock::given(path("/ok"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(path("/fail"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(path("/redirect"))
        .respond_with(ResponseTemplate::new(303).insert_header("location", "/ok"))
        .mount(&server)
        .await;
    let payload = json!({"output":null});
    assert!(send_webhook(&format!("{}/ok", server.uri()), &payload).await);
    assert!(send_webhook(&format!("{}/redirect", server.uri()), &payload).await);
    assert!(!send_webhook(&format!("{}/fail", server.uri()), &payload).await);
    assert!(!send_webhook("invalid URL", &payload).await);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    assert!(
        requests
            .iter()
            .all(|r| r.headers.get("authorization").is_none())
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
        payload
    );
    assert!(
        requests
            .iter()
            .any(|r| r.method == "GET" && r.url.path() == "/ok")
    );
}

fn normalize(value: &mut Value, callback: &str) {
    match value {
        Value::String(text) => {
            if text.starts_with("test-") && text.len() == 41 {
                *text = FIXED_ID.into();
            }
            *text = text.replace(callback, "http://callback.invalid");
        }
        Value::Array(values) => values.iter_mut().for_each(|v| normalize(v, callback)),
        Value::Object(values) => values.values_mut().for_each(|v| normalize(v, callback)),
        _ => {}
    }
}

#[tokio::test]
async fn prestart_gates_requests_while_heartbeats_track_realtime_and_queued_jobs() {
    let ping = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&ping)
        .await;
    let ready = Arc::new(Notify::new());
    let begun = Arc::new(Notify::new());
    let handling = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let signal = handling.clone();
    let done = finish.clone();
    let handler = Handler::new(move |_| {
        let signal = signal.clone();
        let done = done.clone();
        async move {
            signal.notify_one();
            done.notified().await;
            Ok(json!(42))
        }
    });
    let config = WorkerConfig {
        api_key: "worker-token".into(),
        ping_url: Some(ping.uri()),
        ping_interval: Duration::from_millis(15),
        ..WorkerConfig::default()
    };
    let mut api = WorkerApi::new(config, handler)
        .unwrap()
        .with_endpoint_id("ep");
    let progress = api.progress();
    let gate = ready.clone();
    let entered = begun.clone();
    api.prestart_hooks_mut()
        .register_prestart_hook("load", move || {
            let gate = gate.clone();
            let entered = entered.clone();
            async move {
                entered.notify_one();
                gate.notified().await;
                Ok(())
            }
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stopped.await;
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), begun.notified())
        .await
        .unwrap();
    let http = reqwest::Client::new();
    let queued = http
        .post(format!("{address}/run"))
        .json(&json!({"input":{}}))
        .send();
    tokio::pin!(queued);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut queued)
            .await
            .is_err()
    );
    assert!(progress.is_empty());
    tokio::time::timeout(Duration::from_secs(3), async {
        while ping.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("heartbeat runs before readiness");
    ready.notify_one();
    let queued: Value = queued.await.unwrap().json().await.unwrap();
    let queued_id = queued["id"].as_str().unwrap();
    let request = http
        .post(format!("{address}/ep/realtime"))
        .json(&json!({"id":"live","input":false}))
        .send();
    let request = tokio::spawn(request);
    tokio::time::timeout(Duration::from_secs(3), handling.notified())
        .await
        .unwrap();
    assert_eq!(progress.get_job_count(), 2);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let requests = ping.received_requests().await.unwrap();
            if requests.iter().any(|r| {
                r.url.query_pairs().any(|(key, ids)| {
                    key == "job_id" && ids.contains(queued_id) && ids.contains("live")
                })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    assert!(!server.is_finished(), "shutdown drains active requests");
    finish.notify_one();
    assert_eq!(
        request
            .await
            .unwrap()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        json!({"output":42})
    );
    server.await.unwrap().unwrap();
    assert!(progress.get("live").is_none());
    assert!(progress.get(queued_id).is_some());
}

#[tokio::test]
async fn startup_failure_and_cancellation_never_accept_work() {
    for fail in [true, false] {
        let mut api = WorkerApi::new(
            WorkerConfig::default(),
            Handler::sync(|_| panic!("handler before startup")),
        )
        .unwrap();
        let entered = Arc::new(Notify::new());
        let signal = entered.clone();
        api.prestart_hooks_mut()
            .register_prestart_hook("load", move || {
                let signal = signal.clone();
                async move {
                    signal.notify_one();
                    if fail {
                        Err(Error::InvalidInput("weights unavailable".into()))
                    } else {
                        std::future::pending().await
                    }
                }
            });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            api.serve(listener, async {
                let _ = stopped.await;
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        if !fail {
            stop.send(()).unwrap();
        }
        let result = server.await.unwrap();
        if fail {
            assert!(matches!(result, Err(Error::Prestart(_))));
        } else {
            result.unwrap();
        }
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }
}

#[tokio::test]
async fn docs_schema_redirects_validation_and_large_inputs() {
    let api = WorkerApi::new(WorkerConfig::default(), Handler::sync(|job| Ok(job.input))).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stopped.await;
        })
        .await
    });
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for path in ["/", "/redoc"] {
        let response = http.get(format!("{address}{path}")).send().await.unwrap();
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains("text/html")
        );
        assert!(response.text().await.unwrap().contains("/openapi.json"));
    }
    let docs = http.get(format!("{address}/docs")).send().await.unwrap();
    assert_eq!(docs.status(), 307);
    assert_eq!(docs.headers()["location"], "/");
    for (path, suffix) in [
        ("/run/?a=1", "/run?a=1"),
        ("/status/%C3%A9/", "/status/%C3%A9"),
    ] {
        let response = http.post(format!("{address}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 307);
        assert_eq!(response.headers()["location"], format!("{address}{suffix}"));
    }
    let schema: Value = http
        .get(format!("{address}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let original: Value =
        serde_json::from_str(include_str!("fixtures/http_worker_openapi.json")).unwrap();
    for path in ["/run", "/runsync", "/status/{job_id}", "/stream/{job_id}"] {
        assert_eq!(schema["paths"][path], original["paths"][path]);
    }
    assert!(schema["paths"].get("/endpoint/realtime").is_none());
    assert_eq!(schema["info"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        http.head(format!("{address}/openapi.json"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        ""
    );
    assert_eq!(
        http.post(format!("{address}/endpoint/realtime"))
            .json(&json!({"id":"x","input":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let invalid: Value = http
        .post(format!("{address}/run"))
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(invalid["detail"][0]["type"], "json_invalid");
    let large = json!({"payload":"x".repeat(2 * 1024 * 1024 + 1)});
    let response: Value = http
        .post(format!("{address}/runsync"))
        .json(&json!({"input":large}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["output"], large);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn explicit_shared_tracker_and_mode_guards() {
    let tracker = runpod::serverless::JobsProgress::new();
    tracker.add(runpod::serverless::WorkerJob::new("shared", json!({"x":1})));
    let api = WorkerApi::with_progress(
        WorkerConfig::default(),
        Handler::sync(|job| Ok(job.input)),
        tracker.clone(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/status/shared", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stopped.await;
        })
        .await
    });
    let result: Value = reqwest::Client::new()
        .post(url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["output"], json!({"x":1}));
    assert!(tracker.is_empty());
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    for realtime in [true, false] {
        let mut api =
            WorkerApi::new(WorkerConfig::default(), Handler::sync(|_| unreachable!())).unwrap();
        api.prestart_hooks_mut()
            .register_prestart_hook("load", || async { panic!("validation before hook") });
        let error = if realtime {
            api.start_realtime(0, 1).await
        } else {
            api.start("127.0.0.1", 0, 2).await
        }
        .unwrap_err();
        assert!(error.to_string().contains(if realtime {
            "not supported in realtime"
        } else {
            "rp_api_concurrency=1"
        }));
    }
}

#[tokio::test]
async fn handler_errors_capture_logs_and_failed_callbacks_do_not_change_results() {
    use runpod::serverless::capture::{self, CaptureMode};
    let callbacks = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&callbacks)
        .await;
    let handler = Handler::sync(|job| {
        if job.input["fail"] == true {
            capture::stdout().write_text("HTTP captured log\n")?;
            Err(Error::InvalidInput("HTTP handler failed".into()))
        } else {
            Ok(json!({"ok":true}))
        }
    });
    let config = WorkerConfig {
        capture_mode: CaptureMode::All,
        ..WorkerConfig::default()
    };
    let api = WorkerApi::new(config, handler)
        .unwrap()
        .with_endpoint_id("ep");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stopped.await;
        })
        .await
    });
    let http = reqwest::Client::new();
    for path in ["/runsync", "/ep/realtime"] {
        let body: Value = http
            .post(format!("{address}{path}"))
            .json(&json!({"id":"error","input":{"fail":true},"webhook":callbacks.uri()}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let error: Value = serde_json::from_str(body["error"].as_str().unwrap()).unwrap();
        assert_eq!(error["error_message"], "HTTP handler failed");
        assert_eq!(error["logs"], "HTTP captured log\n");
        if path == "/runsync" {
            assert_eq!(body["status"], "FAILED");
        }
    }
    assert!(callbacks.received_requests().await.unwrap().is_empty());
    let response: Value = http
        .post(format!("{address}/runsync"))
        .json(&json!({"input":{},"webhook":callbacks.uri()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["status"], "COMPLETED");
    tokio::time::timeout(Duration::from_secs(3), async {
        while callbacks.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    callbacks.verify().await;
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn python_http_contract_fixtures() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/http_worker.json")).unwrap();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let output = case["output"].clone();
        let kind = case["handler"].as_str().unwrap().to_owned();
        let handler = if kind.starts_with("stream") {
            Handler::streaming(move |job| {
                observed
                    .lock()
                    .unwrap()
                    .push(serde_json::to_value(job).unwrap());
                let mut values: Vec<_> =
                    output.as_array().unwrap().iter().cloned().map(Ok).collect();
                if kind == "stream_error" {
                    values.push(Err(Error::InvalidInput("broken stream".into())));
                }
                futures_util::stream::iter(values)
            })
        } else {
            Handler::sync(move |job| {
                let job = serde_json::to_value(job)?;
                observed.lock().unwrap().push(job.clone());
                Ok(if kind == "echo" { job } else { output.clone() })
            })
        };
        let callbacks = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&callbacks)
            .await;
        let api = WorkerApi::new(WorkerConfig::default(), handler)
            .unwrap()
            .with_endpoint_id("endpoint");
        let progress = api.progress();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            api.serve(listener, async {
                let _ = stopped.await;
            })
            .await
        });
        let mut id = FIXED_ID.to_string();
        for (request, expected) in case["requests"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["responses"].as_array().unwrap())
        {
            let path = request["path"].as_str().unwrap().replace(FIXED_ID, &id);
            let mut sent = http.request(
                request
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("POST")
                    .parse()
                    .unwrap(),
                format!("{address}{path}"),
            );
            if let Some(body) = request.get("json") {
                let body = body
                    .to_string()
                    .replace("http://callback.invalid", &callbacks.uri());
                sent = sent.header("content-type", "application/json").body(body);
            }
            let response = sent.send().await.unwrap();
            assert_eq!(
                response.status().as_u16(),
                expected["status"].as_u64().unwrap() as u16,
                "{name}: {path}"
            );
            let text = response.text().await.unwrap();
            let mut body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
            if path == "/run"
                && let Some(new_id) = body["id"].as_str()
            {
                id = new_id.into();
            }
            normalize(&mut body, &callbacks.uri());
            assert_eq!(body, expected["body"], "{name}: {path}");
            assert_eq!(
                progress.get_job_count(),
                expected["jobs"].as_u64().unwrap() as usize,
                "{name}"
            );
            assert_eq!(
                calls.lock().unwrap().len(),
                expected["calls"].as_u64().unwrap() as usize,
                "{name}"
            );
        }
        let mut observed = json!(*calls.lock().unwrap());
        normalize(&mut observed, &callbacks.uri());
        assert_eq!(observed, case["calls"], "{name}: handler arguments");
        let expected_count = case["webhooks"].as_array().unwrap().len();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if callbacks.received_requests().await.unwrap().len() >= expected_count {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut deliveries: Value = callbacks.received_requests().await.unwrap().into_iter().map(|r| {
            assert!(r.headers.get("authorization").is_none(), "{name}: credential leakage");
            json!({"url":format!("http://callback.invalid{}", r.url.path()), "payload":serde_json::from_slice::<Value>(&r.body).unwrap()})
        }).collect::<Vec<_>>().into();
        normalize(&mut deliveries, &callbacks.uri());
        assert_eq!(deliveries, case["webhooks"], "{name}: callbacks");
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }
}

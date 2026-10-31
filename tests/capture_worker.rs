use futures_util::StreamExt;
use runpod::{
    Error, Value, json,
    serverless::{
        Handler, PrestartError, Worker, WorkerConfig, WorkerJob, build_prestart_cancelled_payload,
        build_prestart_failed_payload,
        capture::{CaptureMode, MAX_CAPTURED_CHARS, TeeWriter},
        run_job_with_capture,
    },
};
use std::{collections::BTreeMap, io, time::Duration};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

fn emit(text: &str) {
    TeeWriter::new(io::sink()).write_text(text).unwrap();
}

#[tokio::test]
async fn generator_wrapper_preserves_partials_and_reports_errors_with_logs() {
    use runpod::serverless::run_job_generator_with_capture;
    let handler = Handler::streaming(|_| {
        emit("factory ");
        async_stream::stream! {
            yield Ok(json!("first"));
            emit("failure diagnostic");
            yield Err(Error::InvalidInput("stream failed".into()));
            yield Ok(json!("unreachable"));
        }
    });
    let result: Vec<_> =
        run_job_generator_with_capture(WorkerJob::new("job", json!({})), CaptureMode::All, handler)
            .collect()
            .await;
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], json!({"output":"first"}));
    let error = result[1]["error"].as_str().unwrap();
    assert!(error.contains("stream failed"));
    assert!(error.ends_with("\nlogs:\nfactory failure diagnostic"));
    let handler = Handler::streaming(|_| {
        futures_util::stream::iter([Ok(json!(1)), Ok(json!({"error":"caller value"}))])
    });
    let result: Vec<_> =
        run_job_generator_with_capture(WorkerJob::new("job", json!({})), CaptureMode::Off, handler)
            .collect()
            .await;
    assert_eq!(
        result,
        vec![
            json!({"output":1}),
            json!({"output":{"error":"caller value"}})
        ]
    );
    let handler = Handler::sync(|_| Err(Error::InvalidInput("factory failed".into())));
    let result: Vec<_> =
        run_job_generator_with_capture(WorkerJob::new("job", json!({})), CaptureMode::All, handler)
            .collect()
            .await;
    assert_eq!(result.len(), 1);
    assert!(
        result[0]["error"]
            .as_str()
            .unwrap()
            .contains("factory failed")
    );
    let handler = Handler::streaming(|_| {
        async_stream::stream! {
            yield Ok(json!("first"));
            tokio::task::yield_now().await;
            yield Ok(json!("second"));
        }
    });
    let result: Vec<_> =
        run_job_generator_with_capture(WorkerJob::new("job", json!({})), CaptureMode::Off, handler)
            .collect()
            .await;
    assert_eq!(
        result,
        vec![json!({"output":"first"}), json!({"output":"second"})]
    );
}
fn decode(result: &Value) -> Value {
    serde_json::from_str(result["error"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn handler_capture_is_opt_in_and_does_not_clip_handler_errors() {
    for mode in [CaptureMode::Auto, CaptureMode::Off, CaptureMode::All] {
        let huge = "\u{1f680}".repeat(MAX_CAPTURED_CHARS * 3);
        let expected = huge.clone();
        let result = run_job_with_capture(WorkerJob::new("job", json!({})), mode, |_| {
            emit("factory ");
            async move {
                emit(&"x".repeat(MAX_CAPTURED_CHARS * 2));
                emit("tail");
                Err(Error::InvalidInput(huge))
            }
        })
        .await;
        let error = decode(&result);
        assert_eq!(error["error_message"], expected);
        assert!(
            error["error_traceback"]
                .as_str()
                .unwrap()
                .contains(&expected)
        );
        if mode == CaptureMode::All {
            let logs = error["logs"].as_str().unwrap();
            assert_eq!(logs.chars().count(), MAX_CAPTURED_CHARS);
            assert!(logs.ends_with("tail"));
        } else {
            assert!(error.get("logs").is_none());
        }
    }
}

#[tokio::test]
async fn successful_handler_output_is_unchanged() {
    let result = run_job_with_capture(
        WorkerJob::new("job", json!({})),
        CaptureMode::All,
        |_| async {
            emit("successful log");
            Ok(json!({"ok":true}))
        },
    )
    .await;
    assert_eq!(result, json!({"output":{"ok":true}}));
    let result = run_job_with_capture(
        WorkerJob::new("job", json!({})),
        CaptureMode::All,
        |_| async {
            emit("caller-owned error");
            Ok(json!({"error":"custom error"}))
        },
    )
    .await;
    assert_eq!(result, json!({"error":"custom error"}));
}

#[test]
fn prestart_payload_retains_original_failure_hook_and_bounded_unicode_fields() {
    let huge = "\u{1f680}".repeat(MAX_CAPTURED_CHARS * 3);
    let error = PrestartError::Failed {
        hook: "load_model".into(),
        source: Box::new(Error::InvalidInput(huge)),
    };
    let payload =
        build_prestart_failed_payload(&error, &"\u{4e2d}".repeat(MAX_CAPTURED_CHARS * 3), "worker");
    assert_eq!(payload["event"], "prestart_failed");
    assert_eq!(payload["error_type"], "runpod::Error");
    assert_eq!(payload["hook"], "load_model");
    assert_eq!(payload["worker_id"], "worker");
    assert_eq!(payload["runpod_version"], env!("CARGO_PKG_VERSION"));
    for name in ["error_message", "error_traceback"] {
        assert!(payload[name].as_str().unwrap().chars().count() <= MAX_CAPTURED_CHARS + 100);
        assert!(payload[name].as_str().unwrap().contains("truncated"));
    }
    assert_eq!(
        payload["logs"].as_str().unwrap().chars().count(),
        MAX_CAPTURED_CHARS
    );
    let empty = PrestartError::Failed {
        hook: String::new(),
        source: Box::new(Error::InvalidInput("bad config".into())),
    };
    let payload = build_prestart_failed_payload(&empty, "", "worker");
    assert_eq!(payload["error_message"], "bad config");
    assert!(
        payload["error_traceback"]
            .as_str()
            .unwrap()
            .contains("runpod::Error")
    );
    assert!(payload.get("logs").is_none());
    assert!(payload.get("hook").is_none());
    let timeout = build_prestart_failed_payload(
        &PrestartError::Timeout {
            hook: "load".into(),
            timeout: Duration::from_secs(1),
        },
        "",
        "worker",
    );
    assert_eq!(timeout["error_type"], "PrestartTimeout");
    assert_eq!(timeout["hook"], "load");
    assert_eq!(
        build_prestart_cancelled_payload("worker"),
        json!({
            "event":"prestart_cancelled", "error_type":"PrestartCancelled",
            "error_message":"worker shutdown began before prestart completed",
            "worker_id":"worker", "runpod_version":env!("CARGO_PKG_VERSION")
        })
    );
}

#[test]
fn worker_mode_comes_from_owned_environment_values() {
    for (value, expected) in [
        (" ALL ", CaptureMode::All),
        ("off", CaptureMode::Off),
        ("unknown", CaptureMode::Auto),
    ] {
        let values = BTreeMap::from([("RUNPOD_LOG_CAPTURE".into(), value.into())]);
        assert_eq!(
            WorkerConfig::from_values(&values).unwrap().capture_mode,
            expected
        );
    }
    assert_eq!(WorkerConfig::default().capture_mode, CaptureMode::Auto);
}

#[tokio::test]
async fn local_worker_captures_synchronous_handler_factories_only_when_enabled() {
    for mode in [CaptureMode::Auto, CaptureMode::Off, CaptureMode::All] {
        let worker = Worker::new(
            WorkerConfig {
                capture_mode: mode,
                ..WorkerConfig::default()
            },
            Handler::sync(|_| {
                emit("sync handler logs");
                Err(Error::InvalidInput("kernel panic".into()))
            }),
        )
        .unwrap();
        let payload = decode(&worker.run_local(json!({"input":{}})).await.unwrap());
        assert_eq!(payload["error_message"], "kernel panic");
        assert_eq!(payload.get("logs").is_some(), mode == CaptureMode::All);
        if mode == CaptureMode::All {
            assert_eq!(payload["logs"], "sync handler logs");
        }
    }
}

#[tokio::test]
async fn concurrent_jobs_and_streamed_failures_keep_their_own_log_tails() {
    let server = MockServer::start().await;
    for id in ["a", "b", "stream"] {
        Mock::given(path(format!("/job-done/{id}")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/job-stream/stream"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let config = WorkerConfig {
        capture_mode: CaptureMode::All,
        job_done_url: Some(format!("{}/job-done/$ID", server.uri())),
        job_stream_url: Some(format!("{}/job-stream/$ID", server.uri())),
        ..WorkerConfig::default()
    };
    let worker = Worker::new(
        config.clone(),
        Handler::new(|job| async move {
            for _ in 0..3 {
                emit(&job.id);
                tokio::task::yield_now().await;
            }
            Err(Error::InvalidInput("failed".into()))
        }),
    )
    .unwrap();
    let (a, b) = tokio::join!(
        worker.handle_job(WorkerJob::new("a", json!({}))),
        worker.handle_job(WorkerJob::new("b", json!({})))
    );
    a.unwrap();
    b.unwrap();
    let streaming = Worker::new(
        config,
        Handler::streaming(|_| {
            emit("factory ");
            async_stream::stream! {
                emit("first ");
                yield Ok(json!(1));
                emit(&"x".repeat(MAX_CAPTURED_CHARS * 2));
                emit("tail");
                yield Err(Error::InvalidInput("error ".repeat(MAX_CAPTURED_CHARS)));
            }
        }),
    )
    .unwrap();
    streaming
        .handle_job(WorkerJob::new("stream", json!({})))
        .await
        .unwrap();
    for request in server.received_requests().await.unwrap() {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        match request.url.path() {
            "/job-done/a" => assert_eq!(decode(&body)["logs"], "aaa"),
            "/job-done/b" => assert_eq!(decode(&body)["logs"], "bbb"),
            "/job-done/stream" => {
                let error = body["error"].as_str().unwrap();
                assert!(error.starts_with("handler: "));
                assert!(error.contains(&"error ".repeat(MAX_CAPTURED_CHARS)));
                let logs = error.split_once("\nlogs:\n").unwrap().1;
                assert_eq!(logs.chars().count(), MAX_CAPTURED_CHARS);
                assert!(logs.ends_with("tail"));
            }
            "/job-stream/stream" => {}
            other => panic!("unexpected request {other}"),
        }
    }
}

#[tokio::test]
async fn prestart_failure_reports_all_hooks_and_respects_capture_off() {
    for mode in [CaptureMode::Auto, CaptureMode::All, CaptureMode::Off] {
        let server = MockServer::start().await;
        Mock::given(path("/job-take/worker"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"held","input":{}})))
            .mount(&server)
            .await;
        Mock::given(path("/job-stop/worker"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(path("/job-done/held"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let config = WorkerConfig {
            worker_id: "worker".into(),
            capture_mode: mode,
            job_get_url: Some(format!("{}/job-take/$ID", server.uri())),
            job_done_url: Some(format!("{}/job-done/$ID", server.uri())),
            ..WorkerConfig::default()
        };
        let mut worker =
            Worker::new(config, Handler::sync(|_| panic!("handler gate opened"))).unwrap();
        worker
            .prestart_hooks_mut()
            .register_sync_prestart_hook("download", || {
                emit("downloaded ");
                Ok(())
            });
        worker
            .prestart_hooks_mut()
            .register_prestart_hook("warm", || async {
                emit("warmed ");
                Ok(())
            });
        worker
            .prestart_hooks_mut()
            .register_sync_prestart_hook("engine", || {
                emit("engine");
                Err(Error::InvalidInput("engine refused to start".into()))
            });
        let error =
            tokio::time::timeout(Duration::from_secs(5), worker.run(std::future::pending()))
                .await
                .unwrap()
                .unwrap_err();
        assert!(matches!(error, Error::Prestart(_)));
        let requests = server.received_requests().await.unwrap();
        let report = requests
            .iter()
            .find(|r| r.url.path() == "/job-done/held")
            .unwrap();
        let payload = decode(&serde_json::from_slice::<Value>(&report.body).unwrap());
        assert_eq!(payload["hook"], "engine");
        assert_eq!(payload["error_message"], "engine refused to start");
        if mode == CaptureMode::Off {
            assert!(payload.get("logs").is_none());
        } else {
            assert_eq!(payload["logs"], "downloaded warmed engine");
        }
    }
}

use runpod::{
    Error, json,
    serverless::{Handler, PrestartError, Worker, WorkerConfig, health::FitnessChecks},
};
use std::{collections::BTreeMap, process::Stdio, time::Duration};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

async fn child(mode: &str, server: Option<&str>) -> std::process::Output {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "worker_process_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNPOD_RUST_WORKER_PROCESS", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(server) = server {
        command.env("RUNPOD_RUST_WORKER_SERVER", server);
    }
    tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("worker process must finish its bounded failure path")
        .unwrap()
}

#[tokio::test]
async fn library_prestart_errors_keep_the_original_typed_cause() {
    let mut worker =
        Worker::new(WorkerConfig::default(), Handler::sync(|_| unreachable!())).unwrap();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("load", || async {
            Err(Error::Io(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            )))
        });
    let error = worker.run_local(json!({"input":{}})).await.unwrap_err();
    let Error::Prestart(PrestartError::Failed { hook, source }) = error else {
        panic!("missing typed prestart cause: {error:?}")
    };
    assert_eq!(hook, "load");
    assert!(
        matches!(*source, Error::Io(ref error) if error.kind()==std::io::ErrorKind::PermissionDenied)
    );
}

#[tokio::test]
async fn queue_prestart_failure_reports_held_job_before_hard_exit() {
    let server = MockServer::start().await;
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"held","input":{}})))
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/held"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let result = child("held", Some(&server.uri())).await;
    assert_eq!(result.status.code(), Some(1), "{result:?}");
    let requests = server.received_requests().await.unwrap();
    let report = requests
        .iter()
        .find(|request| request.url.path() == "/job-done/held")
        .unwrap();
    let body: runpod::Value = serde_json::from_slice(&report.body).unwrap();
    let failure: runpod::Value = serde_json::from_str(body["error"].as_str().unwrap()).unwrap();
    assert_eq!(failure["event"], "prestart_failed");
    assert!(
        failure["error_message"]
            .as_str()
            .unwrap()
            .contains("weights unavailable")
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains("worker returned"));
}

#[tokio::test]
async fn empty_queue_and_stalled_reporting_do_not_prevent_hard_exit() {
    for mode in [
        "empty",
        "slow_report",
        "timeout",
        "sync_failure",
        "sync_timeout",
        "local",
        "http",
    ] {
        let server = MockServer::start().await;
        let take = if mode == "slow_report" {
            ResponseTemplate::new(200).set_body_json(json!({"id":"held","input":{}}))
        } else {
            ResponseTemplate::new(204).set_delay(Duration::from_secs(30))
        };
        Mock::given(path("/job-take/worker"))
            .respond_with(take)
            .mount(&server)
            .await;
        Mock::given(path("/job-stop/worker"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        Mock::given(path("/job-done/held"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let result = child(mode, Some(&server.uri())).await;
        assert_eq!(result.status.code(), Some(1), "{mode}: {result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("prestart_failed"),
            "{mode}: {result:?}"
        );
        assert!(!String::from_utf8_lossy(&result.stdout).contains("worker returned"));
        if mode == "local" {
            assert!(server.received_requests().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn fallback_delivery_error_keeps_precedence_over_the_prestart_exit() {
    let server = MockServer::start().await;
    Mock::given(path("/job-take/worker"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"held","input":{}}))
                .set_delay(Duration::from_millis(10)),
        )
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/held"))
        .respond_with(ResponseTemplate::new(400))
        .expect(1)
        .mount(&server)
        .await;
    let result = child("transport", Some(&server.uri())).await;
    assert_eq!(result.status.code(), Some(0), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stdout).contains("transport error retained"));
}

#[tokio::test]
async fn only_production_startup_replaces_the_process_panic_hook() {
    for mode in ["construct", "local_success", "production_success"] {
        let result = child(mode, None).await;
        assert_eq!(result.status.code(), Some(101), "{mode}: {result:?}");
        let logs = String::from_utf8_lossy(&result.stderr);
        if mode == "production_success" {
            assert!(logs.contains("Uncaught exception"), "{logs}");
            assert!(logs.contains("after startup"), "{logs}");
            assert!(!logs.contains("original panic hook"), "{logs}");
        } else {
            assert!(logs.contains("original panic hook"), "{logs}");
        }
    }
}

#[tokio::test]
async fn environment_start_selects_realtime_without_local_input_or_fitness() {
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    drop(reservation);
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "worker_process_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNPOD_RUST_WORKER_PROCESS", "realtime")
        .env("RUNPOD_RUST_WORKER_SERVER", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let http = reqwest::Client::new();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(response) = http
                .post(format!("http://127.0.0.1:{port}/native/realtime"))
                .json(&json!({"id":"process","input":{"native":true}}))
                .send()
                .await
            {
                break response;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "realtime child exited before serving"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
    .json::<runpod::Value>()
    .await
    .unwrap();
    assert_eq!(result, json!({"output":{"native":true}}));
    child.kill().await.unwrap();
}

#[derive(Debug)]
struct LogToStderr;
impl tracing::Subscriber for LogToStderr {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields;
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                eprintln!("{field}={value:?}");
            }
        }
        event.record(&mut Fields);
    }
}

#[test]
#[ignore = "child process for startup hard exits and process-global panic hooks"]
fn worker_process_child() {
    let Ok(mode) = std::env::var("RUNPOD_RUST_WORKER_PROCESS") else {
        return;
    };
    std::panic::set_hook(Box::new(|_| eprintln!("original panic hook")));
    tracing::subscriber::set_global_default(LogToStderr).unwrap();
    if mode == "http" {
        let mut api = runpod::serverless::WorkerApi::new(
            WorkerConfig::default(),
            Handler::sync(|_| unreachable!()),
        )
        .unwrap();
        api.prestart_hooks_mut()
            .register_sync_prestart_hook("load", || {
                std::thread::spawn(|| {
                    loop {
                        std::thread::park();
                    }
                });
                Err(Error::InvalidInput("weights unavailable".into()))
            });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(api.start("127.0.0.1", 0, 1))
            .unwrap();
        panic!("HTTP process returned after startup failure");
    }
    if mode == "realtime" {
        let port = std::env::var("RUNPOD_RUST_WORKER_SERVER").unwrap();
        let mut fitness = FitnessChecks::new(BTreeMap::from([
            ("RUNPOD_REALTIME_PORT".into(), port),
            ("RUNPOD_ENDPOINT_ID".into(), "native".into()),
        ]));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(runpod::serverless::start_with_fitness(
                Handler::sync(|job| Ok(job.input)),
                &mut fitness,
            ))
            .unwrap();
        return;
    }
    let server = std::env::var("RUNPOD_RUST_WORKER_SERVER").ok();
    let config = WorkerConfig {
        worker_id: "worker".into(),
        job_get_url: server.as_ref().map(|url| format!("{url}/job-take/$ID")),
        job_done_url: server.as_ref().map(|url| format!("{url}/job-done/$ID")),
        prestart_timeout: (mode == "timeout" || mode == "sync_timeout")
            .then_some(Duration::from_millis(20)),
        prestart_claim_timeout: if mode == "transport" {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(50)
        },
        prestart_failure_report_timeout: if mode == "slow_report" {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(3)
        },
        ..WorkerConfig::default()
    };
    let mut worker = Worker::new(config, Handler::sync(|_| Ok(json!("handler")))).unwrap();
    if mode == "construct" {
        panic!("after startup");
    }
    let control = worker.control();
    let progress = worker.progress();
    let hook_mode = mode.clone();
    if mode == "sync_failure" || mode == "sync_timeout" {
        worker
            .prestart_hooks_mut()
            .register_sync_prestart_hook("load", move || {
                std::thread::spawn(|| {
                    loop {
                        std::thread::park();
                    }
                });
                if hook_mode == "sync_timeout" {
                    loop {
                        std::thread::park();
                    }
                }
                Err(Error::InvalidInput("weights unavailable".into()))
            });
    } else if mode != "local_success" {
        worker
            .prestart_hooks_mut()
            .register_prestart_hook("load", move || {
                let mode = hook_mode.clone();
                let control = control.clone();
                let progress = progress.clone();
                async move {
                    if mode == "production_success" {
                        control.kill_worker();
                        return Ok(());
                    }
                    std::thread::spawn(|| {
                        loop {
                            std::thread::park();
                        }
                    });
                    if mode == "timeout" {
                        std::future::pending::<()>().await;
                    }
                    if mode == "held" || mode == "slow_report" {
                        while progress.is_empty() {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    }
                    Err(Error::InvalidInput("weights unavailable".into()))
                }
            });
    }
    let mut fitness = FitnessChecks::new(BTreeMap::from([(
        "RUNPOD_SKIP_FITNESS_CHECKS".into(),
        "true".into(),
    )]));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            if mode == "local" || mode == "local_success" {
                worker.start_local(json!({"input":{}})).await.unwrap();
            } else {
                let result = worker.start(&mut fitness).await;
                if mode == "transport" {
                    assert!(matches!(result, Err(Error::Http(_))), "{result:?}");
                    println!("transport error retained");
                } else {
                    result.unwrap();
                }
            }
        });
    if mode == "production_success" || mode == "local_success" {
        panic!("after startup");
    }
    println!("worker returned");
}

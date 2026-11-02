use super::*;
use crate::apps::{App, NativeFunction, ResourceSpec, targets::FunctionDefinition};

fn handle(body: NativeFunction) -> FunctionHandle {
    App::new("task-tests")
        .unwrap()
        .task(
            ResourceSpec::default(),
            FunctionDefinition {
                name: "work".into(),
                parameters: vec![],
                source: None,
            },
            body,
        )
        .unwrap()
}

async fn compare_cases(prefix: &str) {
    let mut matched = 0;
    let data: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/task_runner.json")).unwrap();
    assert_eq!(data["cases"].as_array().unwrap().len(), 48);
    for case in data["cases"].as_array().unwrap() {
        if !case["name"].as_str().unwrap().starts_with(prefix) {
            continue;
        }
        matched += 1;
        let task = handle(NativeFunction::sync(|args| {
            Ok(json!({"args":args.args,"kwargs":args.kwargs}))
        }));
        let (jobs, mut receiver) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            token: "token".into(),
            config: TaskRunnerConfig::default(),
            jobs,
            wake: Notify::new(),
            state: Mutex::new(RunnerState {
                phase: match case["phase"].as_str().unwrap() {
                    "ready" => Phase::Ready(task),
                    "initializing" => Phase::Initializing,
                    _ => Phase::Failed(bootstrap_error("private failure".into())),
                },
                job: if case["inline"] == true {
                    JobState::Inline
                } else {
                    match case["job"].as_str().unwrap() {
                        "RUNNING" => JobState::Running,
                        "DONE" => JobState::Done(None),
                        _ => JobState::None,
                    }
                },
                ..RunnerState::default()
            }),
        });
        let executing = shared.clone();
        let worker = tokio::spawn(async move {
            while let Some(job) = receiver.recv().await {
                execute_job(executing.clone(), job).await.unwrap();
            }
        });
        let mut request = Request::builder()
            .method(case["method"].as_str().unwrap())
            .uri(case["path"].as_str().unwrap());
        if let Some(token) = case["token"].as_str() {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = dispatch(
            &shared,
            request
                .body(Body::from(case["raw"].as_str().unwrap().to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            u64::from(response.status().as_u16()),
            case["status"].as_u64().unwrap(),
            "{}",
            case["name"]
        );
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body, case["output"], "{}", case["name"]);
        let state = shared.lock().unwrap();
        assert_eq!(
            json!(state.last_contact.is_some()),
            case["touched"],
            "{}",
            case["name"]
        );
        assert_eq!(
            json!(state.deadline.is_some()),
            case["deadline"],
            "{}",
            case["name"]
        );
        worker.abort();
    }
    assert!(matched > 0, "fixture prefix matched no cases: {prefix}");
}

#[tokio::test]
async fn source_http_captures() {
    compare_cases("").await;
}
#[tokio::test]
async fn test_ping_unauthenticated() {
    compare_cases("ready GET /ping").await;
}
#[tokio::test]
async fn test_execute_requires_auth() {
    compare_cases("unauthorized /execute").await;
}
#[tokio::test]
async fn test_unknown_path_404() {
    compare_cases("ready GET /unknown").await;
    compare_cases("POST unknown").await;
}
#[tokio::test]
async fn test_invalid_timeout_is_rejected() {
    for prefix in ["timeout -1", "timeout True", "timeout '60'"] {
        compare_cases(prefix).await;
    }
}
#[tokio::test]
async fn test_timeout_requires_auth() {
    compare_cases("unauthorized /timeout").await;
}
#[tokio::test]
async fn test_production_rejects_request_overrides() {
    compare_cases("override ").await;
}
#[tokio::test]
async fn test_production_rejects_malformed_body() {
    compare_cases("nonobject ").await;
    compare_cases("bad JSON").await;
}
#[tokio::test]
async fn test_readiness_error_details_require_authenticated_route() {
    compare_cases("failed ").await;
}
#[tokio::test]
async fn test_no_policy_cap_does_not_add_deadline() {
    compare_cases("execute ").await;
}

#[tokio::test(start_paused = true)]
async fn test_timeout_cannot_be_extended_by_requests() {
    let mut state = RunnerState::default();
    state.set_timeout(Some(Duration::from_secs(60)));
    let deadline = state.deadline;
    tokio::time::advance(Duration::from_secs(1)).await;
    state.set_timeout(Some(Duration::from_secs(600)));
    state.set_timeout(None);
    assert_eq!(state.deadline, deadline);
    state.set_timeout(Some(Duration::ZERO));
    assert!(state.deadline < deadline);
}

#[tokio::test]
async fn test_registered_task_errors_preserve_stdout() {
    for body in [
        NativeFunction::sync(|_| {
            crate::serverless::capture::TeeWriter::new(std::io::sink())
                .write_text(&"x".repeat(20000))?;
            crate::serverless::capture::stderr().write_text("private diagnostic")?;
            Err(Error::InvalidInput("task failed".into()))
        }),
        NativeFunction::streaming(|_| {
            async_stream::try_stream! {
                crate::serverless::capture::TeeWriter::new(std::io::sink()).write_text(&"x".repeat(20000))?;
                yield json!(1);
                Err(Error::InvalidInput("task failed".into()))?;
            }
        }),
    ] {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            token: "token".into(),
            config: TaskRunnerConfig::default(),
            jobs: sender,
            wake: Notify::new(),
            state: Mutex::new(RunnerState::default()),
        });
        execute_job(
            shared.clone(),
            Job {
                handle: handle(body),
                args: vec![],
                kwargs: serde_json::Map::new(),
                reply: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            shared.lock().unwrap().result(),
            json!({"status":"DONE","response":{"success":false,"error":"task failed","stdout":"x".repeat(20000)}})
        );
    }
}

#[tokio::test]
async fn test_registered_generators_aggregate_json() {
    let (sender, _) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        token: "token".into(),
        config: TaskRunnerConfig::default(),
        jobs: sender,
        wake: Notify::new(),
        state: Mutex::new(RunnerState::default()),
    });
    let task = handle(NativeFunction::streaming(|_| {
        async_stream::try_stream! {
            crate::serverless::capture::TeeWriter::new(std::io::sink()).write_text("chunks\n")?;
            yield json!(1);yield json!(2);
        }
    }));
    execute_job(
        shared.clone(),
        Job {
            handle: task,
            args: vec![],
            kwargs: serde_json::Map::new(),
            reply: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        shared.lock().unwrap().result(),
        json!({"status":"DONE","response":{"success":true,"json_result":[1,2],"stdout":"chunks\n"}})
    );
}

#[tokio::test(start_paused = true)]
async fn watchdog_uses_idle_contact_but_execution_deadline_always_wins() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    tokio::time::advance(Duration::from_secs(1000)).await;
    for (job, contact, deadline, expected) in [
        (JobState::None, None, false, 0),
        (JobState::None, Some(Duration::from_secs(601)), false, 1),
        (
            JobState::Done(None),
            Some(Duration::from_secs(600)),
            false,
            0,
        ),
        (
            JobState::Done(None),
            Some(Duration::from_secs(601)),
            false,
            1,
        ),
        (JobState::Running, Some(Duration::from_secs(601)), false, 0),
        (JobState::Inline, Some(Duration::from_secs(601)), false, 0),
        (JobState::Running, Some(Duration::ZERO), true, 1),
        (JobState::Inline, Some(Duration::ZERO), true, 1),
    ] {
        let (sender, _) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            token: "token".into(),
            config: TaskRunnerConfig::default(),
            jobs: sender,
            wake: Notify::new(),
            state: Mutex::new(RunnerState {
                phase: Phase::Failed(bootstrap_error("failed".into())),
                job,
                last_contact: contact.map(|elapsed| Instant::now() - elapsed),
                deadline: deadline.then(Instant::now),
                init_timeout: false,
            }),
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let task = tokio::spawn(async move {
            watchdog(&shared, &mut || {
                called.fetch_add(1, Ordering::SeqCst);
                async { Ok(true) }
            })
            .await
        });
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), expected);
        task.abort();
    }
}

#[tokio::test(start_paused = true)]
async fn unchanged_deadlines_do_not_accelerate_failed_deletion_retries() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (sender, _) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        token: "token".into(),
        config: TaskRunnerConfig::default(),
        jobs: sender,
        wake: Notify::new(),
        state: Mutex::new(RunnerState {
            phase: Phase::Ready(handle(NativeFunction::sync(|_| Ok(Value::Null)))),
            deadline: Some(Instant::now()),
            ..RunnerState::default()
        }),
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let observing = shared.clone();
    let watcher = tokio::spawn(async move {
        watchdog(&observing, &mut || {
            called.fetch_add(1, Ordering::SeqCst);
            async { Ok(false) }
        })
        .await
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for timeout in [Value::Null, json!(600)] {
        let request = Request::builder()
            .method("POST")
            .uri("/timeout")
            .header("Authorization", "Bearer token")
            .body(Body::from(json!({"timeout":timeout}).to_string()))
            .unwrap();
        assert_eq!(dispatch(&shared, request).await.unwrap().status(), 200);
        tokio::task::yield_now().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an unchanged deadline must not wake the watchdog"
        );
    }
    watcher.abort();
}

#[test]
fn task_stdout_survives_a_closed_container_log() {
    struct Closed;
    impl std::io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }
    let capture = Capture::task_stdout();
    capture.scope_sync(|| {
        let mut writer = crate::serverless::capture::TeeWriter::new(Closed);
        assert_eq!(writer.write_text("kept\n").unwrap(), 5);
    });
    assert_eq!(capture.text(), "kept\n");
}

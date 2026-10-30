use runpod::{
    Error,
    serverless::health::{
        EARLY_CHECKS_DONE_ENV, EarlyCheckStatus, FitnessCheck, FitnessChecks, FitnessPass,
        report_unhealthy,
    },
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

fn environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("RUNPOD_SKIP_GPU_CHECK".into(), "true".into()),
        ("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS".into(), "true".into()),
    ])
}
fn early_environment() -> BTreeMap<String, String> {
    let mut env = environment();
    env.insert("RUNPOD_ENDPOINT_ID".into(), "endpoint".into());
    env.insert(
        "RUNPOD_WEBHOOK_GET_JOB".into(),
        "http://worker.invalid/job".into(),
    );
    env
}

#[tokio::test]
async fn sync_async_registration_returns_original_and_preserves_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut registry = FitnessChecks::new(environment());
    registry.check(FitnessPass::WorkerStart).await.unwrap();
    let captured = events.clone();
    let first = FitnessCheck::sync("first", move || {
        captured.lock().unwrap().push(1);
        Ok(())
    });
    assert_eq!(registry.register_fitness_check(first.clone()), first);
    let captured = events.clone();
    registry.register_fitness_check(FitnessCheck::new("second", move || {
        let captured = captured.clone();
        async move {
            captured.lock().unwrap().push(2);
            Ok(())
        }
    }));
    registry.check(FitnessPass::WorkerStart).await.unwrap();
    assert_eq!(*events.lock().unwrap(), vec![1, 2]);
    assert_eq!(
        registry
            .checks()
            .iter()
            .map(FitnessCheck::name)
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    first.run().await.unwrap();
    assert_eq!(*events.lock().unwrap(), vec![1, 2, 1]);
}

#[tokio::test]
async fn duplicate_identity_runs_twice_initially_then_completed_checks_are_skipped() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let check = FitnessCheck::sync("same", move || {
        captured.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let mut registry = FitnessChecks::new(environment());
    registry.register_fitness_check(check.clone());
    registry.register_fitness_check(check);
    registry.check(FitnessPass::WorkerStart).await.unwrap();
    registry.check(FitnessPass::WorkerStart).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let captured = calls.clone();
    registry.register_fitness_check(FitnessCheck::sync("same", move || {
        captured.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    registry.check(FitnessPass::WorkerStart).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    registry.clear_fitness_checks();
    registry.clear_fitness_checks();
    assert!(registry.checks().is_empty());
}

#[tokio::test]
async fn failure_retains_check_error_and_stops_later_checks() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = FitnessChecks::new(environment());
    let captured = calls.clone();
    registry.register_fitness_check(FitnessCheck::sync("first", move || {
        captured.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    registry.register_fitness_check(FitnessCheck::new("broken", || async {
        Err(Error::InvalidInput("bad threshold".into()))
    }));
    registry.register_fitness_check(FitnessCheck::sync("unreached", || {
        panic!("must stop at first failure")
    }));
    let failure = registry.check(FitnessPass::WorkerStart).await.unwrap_err();
    assert_eq!(failure.check, "broken");
    assert_eq!(failure.reason(), "ValueError: bad threshold");
    assert!(std::error::Error::source(&failure).is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn skip_flag_bypasses_custom_checks_and_setup_errors() {
    for flag in ["1", " true ", "YES", "on"] {
        let mut env = environment();
        env.insert("RUNPOD_SKIP_FITNESS_CHECKS".into(), flag.into());
        env.remove("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS");
        env.insert("RUNPOD_MIN_MEMORY_GB".into(), "invalid".into());
        let mut registry = FitnessChecks::new(env);
        registry.register_fitness_check(FitnessCheck::sync("never", || panic!("skipped")));
        registry.check(FitnessPass::WorkerStart).await.unwrap();
    }
}

#[test]
fn early_pass_sets_an_owned_child_marker_and_leaves_custom_checks_for_worker_start() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let mut registry = FitnessChecks::new(early_environment());
    registry.register_fitness_check(
        FitnessCheck::sync("customer", move || {
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .defer_to_worker_start(),
    );
    assert_eq!(
        registry.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Passed
    );
    assert_eq!(
        registry
            .environment()
            .get(EARLY_CHECKS_DONE_ENV)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        registry.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Skipped
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(registry.check(FitnessPass::WorkerStart))
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn early_gate_respects_local_realtime_defer_skip_and_inherited_markers() {
    for (key, value) in [
        ("RUNPOD_REALTIME_PORT", "8000"),
        ("RUNPOD_DEFER_FITNESS_CHECKS", "true"),
        ("RUNPOD_SKIP_FITNESS_CHECKS", "1"),
        (EARLY_CHECKS_DONE_ENV, "0"),
    ] {
        let mut env = early_environment();
        env.insert(key.into(), value.into());
        let mut checks = FitnessChecks::new(env);
        assert_eq!(
            checks.run_startup_fitness_checks(&[]),
            EarlyCheckStatus::Skipped
        );
    }
    for arg in ["--test_input={}", "--rp_serve_api"] {
        let mut checks = FitnessChecks::new(early_environment());
        assert_eq!(
            checks.run_startup_fitness_checks(&[arg.into()]),
            EarlyCheckStatus::Skipped
        );
    }
    let mut checks = FitnessChecks::new(environment());
    assert_eq!(
        checks.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Skipped
    );
}

#[tokio::test]
async fn early_sync_entrypoint_defers_inside_an_async_runtime() {
    let mut checks = FitnessChecks::new(early_environment());
    assert_eq!(
        checks.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Deferred
    );
    assert!(!checks.environment().contains_key(EARLY_CHECKS_DONE_ENV));
}

#[test]
fn early_setup_failure_does_not_latch_or_mark_and_can_be_retried() {
    let mut env = early_environment();
    env.remove("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS");
    env.insert("RUNPOD_MIN_MEMORY_GB".into(), "invalid".into());
    let mut registry = FitnessChecks::new(env);
    assert_eq!(
        registry.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Deferred
    );
    assert!(!registry.environment().contains_key(EARLY_CHECKS_DONE_ENV));
    assert!(registry.checks().is_empty());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(registry.check(FitnessPass::WorkerStart))
        .unwrap_err();
    assert_eq!(error.check, "fitness_check_setup");
    registry
        .environment_mut()
        .insert("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS".into(), "true".into());
    runtime
        .block_on(registry.check(FitnessPass::WorkerStart))
        .unwrap();
}

#[tokio::test]
async fn unhealthy_report_uses_raw_auth_substitution_and_unicode_character_cap() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping/podABC"))
        .and(header("authorization", "key-123"))
        .and(query_param("status", "unhealthy"))
        .and(query_param("check", "disk"))
        .and(query_param("reason", "é".repeat(256)))
        .and(query_param("runpod_version", env!("CARGO_PKG_VERSION")))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let values = BTreeMap::from([
        (
            "RUNPOD_WEBHOOK_PING".into(),
            format!("{}/ping/$RUNPOD_POD_ID", server.uri()),
        ),
        ("RUNPOD_AI_API_KEY".into(), "key-123".into()),
        ("RUNPOD_POD_ID".into(), "podABC".into()),
    ]);
    report_unhealthy(&values, "disk", &"é".repeat(300)).await;
}

#[tokio::test]
async fn unhealthy_report_skips_incomplete_settings_and_swallows_errors() {
    let server = MockServer::start().await;
    for values in [
        BTreeMap::new(),
        BTreeMap::from([("RUNPOD_WEBHOOK_PING".into(), server.uri())]),
        BTreeMap::from([
            (
                "RUNPOD_WEBHOOK_PING".into(),
                format!("{}/$RUNPOD_POD_ID", server.uri()),
            ),
            ("RUNPOD_AI_API_KEY".into(), "key".into()),
        ]),
        BTreeMap::from([
            ("RUNPOD_WEBHOOK_PING".into(), "PING_NOT_SET".into()),
            ("RUNPOD_AI_API_KEY".into(), "key".into()),
        ]),
    ] {
        report_unhealthy(&values, "check", "reason").await;
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    report_unhealthy(
        &BTreeMap::from([
            ("RUNPOD_WEBHOOK_PING".into(), "not a URL".into()),
            ("RUNPOD_AI_API_KEY".into(), "key".into()),
        ]),
        "check",
        "reason",
    )
    .await;
}

#[tokio::test]
async fn unhealthy_reporting_is_bounded_before_process_exit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let values = BTreeMap::from([
        ("RUNPOD_WEBHOOK_PING".into(), server.uri()),
        ("RUNPOD_AI_API_KEY".into(), "key".into()),
    ]);
    tokio::time::timeout(
        Duration::from_secs(3),
        report_unhealthy(&values, "check", "reason"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn worker_waits_for_fitness_before_intake_heartbeat_hooks_and_handlers() {
    use runpod::{
        json,
        serverless::{Handler, Worker, WorkerConfig},
    };
    use tokio::sync::Notify;
    let server = MockServer::start().await;
    let claimed = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/job-take-batch/worker"))
        .respond_with(move |_: &wiremock::Request| {
            if claimed.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_json(json!([{"id":"a","input":{}}]))
            } else {
                ResponseTemplate::new(204)
            }
        })
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let done = Arc::new(Notify::new());
    let completed = done.clone();
    Mock::given(path("/job-done/a"))
        .respond_with(move |_: &wiremock::Request| {
            completed.notify_one();
            ResponseTemplate::new(200)
        })
        .expect(1)
        .mount(&server)
        .await;
    let hooks = Arc::new(AtomicUsize::new(0));
    let invoked = Arc::new(AtomicUsize::new(0));
    let called = invoked.clone();
    let mut worker = Worker::new(
        WorkerConfig {
            worker_id: "worker".into(),
            api_key: "worker-key".into(),
            concurrency: 2,
            job_get_url: Some(format!("{}/job-take/$ID", server.uri())),
            job_done_url: Some(format!("{}/job-done/$ID", server.uri())),
            ping_url: Some(format!("{}/ping", server.uri())),
            idle_interval: Duration::from_millis(10),
            ..WorkerConfig::default()
        },
        Handler::sync(move |_| {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(json!("ready"))
        }),
    )
    .unwrap();
    let seen = hooks.clone();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("init", move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        });
    let mut fitness = FitnessChecks::new(environment());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (started, ready) = (entered.clone(), release.clone());
    fitness.register_fitness_check(FitnessCheck::new("wait", move || {
        let (started, ready) = (started.clone(), ready.clone());
        async move {
            started.notify_one();
            ready.notified().await;
            Ok(())
        }
    }));
    let task =
        tokio::spawn(async move { worker.run_with_fitness(&mut fitness, done.notified()).await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    assert_eq!(hooks.load(Ordering::SeqCst), 0);
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
    assert!(server.received_requests().await.unwrap().is_empty());
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(hooks.load(Ordering::SeqCst), 1);
    assert_eq!(invoked.load(Ordering::SeqCst), 1);
}

#[test]
fn worker_configuration_uses_the_fitness_registrys_owned_environment() {
    use runpod::serverless::WorkerConfig;
    let mut values = environment();
    values.insert("RUNPOD_POD_ID".into(), "owned-worker".into());
    values.insert("RUNPOD_PING_INTERVAL".into(), "25".into());
    values.insert(
        "RUNPOD_WEBHOOK_GET_JOB".into(),
        "https://example.invalid/job".into(),
    );
    let registry = FitnessChecks::new(values);
    let config = WorkerConfig::from_values(registry.environment()).unwrap();
    assert_eq!(config.worker_id, "owned-worker");
    assert_eq!(config.ping_interval, Duration::from_millis(25));
    assert!(config.job_get_url.is_some());
}

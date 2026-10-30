use runpod::{
    json,
    serverless::{ConcurrencyModifier, Handler, Worker, WorkerConfig},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::path};

fn config(server: &MockServer) -> WorkerConfig {
    WorkerConfig {
        worker_id: "worker".into(),
        job_get_url: Some(format!("{}/job-take/$ID", server.uri())),
        job_done_url: Some(format!("{}/job-done/$ID", server.uri())),
        idle_interval: Duration::from_millis(10),
        ..WorkerConfig::default()
    }
}

#[tokio::test]
async fn failed_prestart_cancels_intake_and_owns_one_fresh_fallback_claim() {
    let server = MockServer::start().await;
    let first = Arc::new(Notify::new());
    let seen = first.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    Mock::given(path("/job-take/worker"))
        .respond_with(move |_: &Request| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                seen.notify_one();
                ResponseTemplate::new(204).set_delay(Duration::from_secs(30))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"id":"fallback","input":{}}))
            }
        })
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/fallback"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut worker = Worker::new(config(&server), Handler::sync(|_| unreachable!())).unwrap();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("bad", move || {
            let first = first.clone();
            async move {
                first.notified().await;
                Err(runpod::Error::InvalidInput("startup failure".into()))
            }
        });
    let control = worker.control();
    let error = tokio::time::timeout(Duration::from_secs(2), worker.run(std::future::pending()))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("startup failure"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!control.is_alive());
    assert_eq!(control.current_occupancy(), 0);
}

#[tokio::test]
async fn fallback_report_error_propagates_like_python() {
    let server = MockServer::start().await;
    let first = Arc::new(Notify::new());
    let signal = first.clone();
    let count = AtomicUsize::new(0);
    Mock::given(path("/job-take/worker"))
        .respond_with(move |_: &Request| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                signal.notify_one();
                ResponseTemplate::new(204).set_delay(Duration::from_secs(30))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"id":"fallback","input":{}}))
            }
        })
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/fallback"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    let mut worker = Worker::new(config(&server), Handler::sync(|_| unreachable!())).unwrap();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("bad", move || {
            let first = first.clone();
            async move {
                first.notified().await;
                Err(runpod::Error::InvalidInput("startup error".into()))
            }
        });
    let error = tokio::time::timeout(Duration::from_secs(2), worker.run(std::future::pending()))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.status_code(), Some(403));
    assert!(worker.progress().is_empty());
}

#[tokio::test]
async fn canceled_prestart_held_job_does_not_cause_another_failure_claim() {
    let server = MockServer::start().await;
    let fail = Arc::new(Notify::new());
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"held","input":{}})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let mut settings = config(&server);
    settings.concurrency_modifier = Some(ConcurrencyModifier::new(move |_| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            1
        } else {
            0
        }
    }));
    let mut worker = Worker::new(settings, Handler::sync(|_| unreachable!())).unwrap();
    let signal = fail.clone();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("bad", move || {
            let fail = signal.clone();
            async move {
                fail.notified().await;
                Err(runpod::Error::InvalidInput("bad".into()))
            }
        });
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while control.current_concurrency() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(control.stop_job("held"));
    fail.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path().starts_with("/job-done/"))
    );
}

#[tokio::test]
async fn stalled_prestart_failure_report_is_bounded_and_preserves_startup_reason() {
    let server = MockServer::start().await;
    let fail = Arc::new(Notify::new());
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"held","input":{}})))
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/held"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .expect(1)
        .mount(&server)
        .await;
    let mut settings = config(&server);
    settings.prestart_failure_report_timeout = Duration::from_millis(40);
    let mut worker = Worker::new(settings, Handler::sync(|_| unreachable!())).unwrap();
    let signal = fail.clone();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("bad", move || {
            let fail = signal.clone();
            async move {
                fail.notified().await;
                Err(runpod::Error::InvalidInput("original reason".into()))
            }
        });
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while control.current_occupancy() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fail.notify_one();
    let error = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("original reason"));
    assert_eq!(control.current_occupancy(), 0);
}

#[tokio::test]
async fn fallback_claim_timeout_bounds_a_silent_server() {
    let server = MockServer::start().await;
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let mut settings = config(&server);
    settings.prestart_claim_timeout = Duration::from_millis(20);
    let mut worker = Worker::new(settings, Handler::sync(|_| unreachable!())).unwrap();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("bad", || async {
            Err(runpod::Error::InvalidInput("bad".into()))
        });
    assert!(
        tokio::time::timeout(Duration::from_secs(2), worker.run(std::future::pending()))
            .await
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn shutdown_fails_held_jobs_and_cancels_blocked_prestart() {
    struct Dropped(Arc<AtomicUsize>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let server = MockServer::start().await;
    let begun = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"held","input":{}})))
        .expect(1)
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
    let mut worker = Worker::new(
        config(&server),
        Handler::sync(|_| unreachable!("handler must remain gated")),
    )
    .unwrap();
    let signal = begun.clone();
    let end = dropped.clone();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("blocked", move || {
            let begun = signal.clone();
            let end = end.clone();
            async move {
                let _guard = Dropped(end);
                begun.notify_one();
                std::future::pending::<()>().await;
                Ok(())
            }
        });
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), begun.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while control.current_occupancy() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    control.kill_worker();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(control.current_occupancy(), 0);
    let sent = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path() == "/job-done/held")
        .unwrap();
    let outer: runpod::Value = serde_json::from_slice(&sent.body).unwrap();
    let inner: runpod::Value = serde_json::from_str(outer["error"].as_str().unwrap()).unwrap();
    assert_eq!(inner["event"], "prestart_cancelled");
    assert_eq!(inner["error_type"], "PrestartCancelled");
}

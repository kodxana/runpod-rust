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

#[tokio::test(start_paused = true)]
async fn zero_concurrency_waits_instead_of_spinning_and_control_wakes_shutdown() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let worker = Worker::new(
        WorkerConfig {
            concurrency_modifier: Some(ConcurrencyModifier::new(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                0
            })),
            ..WorkerConfig::default()
        },
        Handler::sync(|_| unreachable!()),
    )
    .unwrap();
    let control = worker.control();
    assert!(control.is_alive());
    assert!(!control.stop_job("unknown"));
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(control.current_concurrency(), 0);
    assert_eq!(control.current_occupancy(), 0);
    control.kill_worker();
    tokio::time::timeout(Duration::from_millis(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!control.is_alive());
}

#[tokio::test]
async fn changing_scale_waits_for_existing_work_before_taking_a_new_batch() {
    let server = MockServer::start().await;
    let first_started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let third_batch = Arc::new(Notify::new());
    let fetched = Arc::new(AtomicUsize::new(0));
    let seen = fetched.clone();
    let next = third_batch.clone();
    Mock::given(path("/job-take-batch/worker"))
        .respond_with(move |request: &Request| {
            let n: usize = request
                .url
                .query_pairs()
                .find(|(k, _)| k == "batch_size")
                .unwrap()
                .1
                .parse()
                .unwrap();
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                assert_eq!(n, 2);
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"id":"a","input":{}},{"id":"b","input":{}}]))
            } else {
                assert_eq!(n, 3);
                next.notify_one();
                ResponseTemplate::new(204)
            }
        })
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    for id in ["a", "b"] {
        Mock::given(path(format!("/job-done/{id}")))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    let mut settings = config(&server);
    settings.scale_wait_interval = Duration::from_millis(10);
    settings.concurrency_modifier =
        Some(ConcurrencyModifier::new(
            |current| if current == 1 { 2 } else { 3 },
        ));
    let started = first_started.clone();
    let gate = release.clone();
    let worker = Worker::new(
        settings,
        Handler::new(move |_| {
            let started = started.clone();
            let gate = gate.clone();
            async move {
                let wait = gate.notified();
                tokio::pin!(wait);
                wait.as_mut().enable();
                started.fetch_add(1, Ordering::SeqCst);
                wait.await;
                Ok(json!("done"))
            }
        }),
    )
    .unwrap();
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while first_started.load(Ordering::SeqCst) < 2 || control.current_concurrency() != 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fetched.load(Ordering::SeqCst), 1);
    assert_eq!(control.current_occupancy(), 2);
    release.notify_waiters();
    tokio::time::timeout(Duration::from_secs(2), third_batch.notified())
        .await
        .unwrap();
    control.kill_worker();
    task.await.unwrap().unwrap();
    assert_eq!(control.current_occupancy(), 0);
}

async fn full_batch_case(partial: bool) {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let took = Arc::new(Notify::new());
    let take = took.clone();
    let count = calls.clone();
    Mock::given(path("/job-take-batch/worker")).respond_with(move |request:&Request| {
        let n:usize=request.url.query_pairs().find(|(k,_)|k=="batch_size").unwrap().1.parse().unwrap();
        if count.fetch_add(1,Ordering::SeqCst)==0 { assert_eq!(n,4); ResponseTemplate::new(200).set_body_json(json!(
            [{"id":"a","input":{}},{"id":"b","input":{}},{"id":"c","input":{}},{"id":"d","input":{}}]))
        } else { assert_eq!(n,if partial {3}else{4}); take.notify_one(); ResponseTemplate::new(204) }
    }).mount(&server).await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    for id in ["a", "b", "c", "d"] {
        Mock::given(path(format!("/job-done/{id}")))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
    }
    let started = Arc::new(AtomicUsize::new(0));
    let first = Arc::new(Notify::new());
    let last = Arc::new(Notify::new());
    let mut settings = config(&server);
    settings.concurrency = 4;
    settings.full_wait_interval = if partial {
        Duration::from_millis(200)
    } else {
        Duration::from_secs(10)
    };
    let begin = started.clone();
    let first_gate = first.clone();
    let last_gate = last.clone();
    let worker = Worker::new(
        settings,
        Handler::new(move |job| {
            let started = begin.clone();
            let gate = if job.id == "d" {
                last_gate.clone()
            } else {
                first_gate.clone()
            };
            async move {
                let wait = gate.notified();
                tokio::pin!(wait);
                wait.as_mut().enable();
                started.fetch_add(1, Ordering::SeqCst);
                wait.await;
                Ok(json!("done"))
            }
        }),
    )
    .unwrap();
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while started.load(Ordering::SeqCst) < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.notify_waiters();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "must not take while only partly drained"
    );
    if !partial {
        last.notify_waiters();
    }
    tokio::time::timeout(Duration::from_secs(2), took.notified())
        .await
        .unwrap();
    control.kill_worker();
    last.notify_waiters();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn full_worker_takes_a_full_batch_as_soon_as_it_drains() {
    full_batch_case(false).await;
}
#[tokio::test]
async fn full_worker_takes_freed_slots_after_the_wait_bound() {
    full_batch_case(true).await;
}

#[tokio::test]
async fn control_cancels_only_the_named_handler_and_shutdown_drains_the_other() {
    cancellation_case(false).await;
}
#[tokio::test]
async fn canceled_job_does_not_request_refresh() {
    cancellation_case(true).await;
}
async fn cancellation_case(refresh: bool) {
    let server = MockServer::start().await;
    let began = Arc::new(AtomicUsize::new(0));
    let finish = Arc::new(Notify::new());
    Mock::given(path("/job-take-batch/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"id":"cancel","input":{}},{"id":"finish","input":{}}])))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/finish"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut settings = config(&server);
    settings.concurrency = 2;
    settings.refresh_worker = refresh;
    let started = began.clone();
    let release = finish.clone();
    let worker = Worker::new(
        settings,
        Handler::new(move |job| {
            let started = started.clone();
            let release = release.clone();
            async move {
                let wait = release.notified();
                tokio::pin!(wait);
                wait.as_mut().enable();
                started.fetch_add(1, Ordering::SeqCst);
                if job.id == "cancel" {
                    std::future::pending::<()>().await;
                } else {
                    wait.await;
                }
                Ok(json!("done"))
            }
        }),
    )
    .unwrap();
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while began.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(control.stop_job("cancel"));
    assert!(!control.stop_job("missing"));
    if refresh {
        tokio::time::timeout(Duration::from_secs(2), async {
            while control.current_occupancy() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            control.is_alive(),
            "canceling one job must not refresh the worker"
        );
    } else {
        control.kill_worker();
    }
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    finish.notify_waiters();
    task.await.unwrap().unwrap();
    assert_eq!(control.current_occupancy(), 0);
    assert!(!control.stop_job("cancel"));
}

#[tokio::test]
async fn result_delivery_error_is_isolated_from_sibling_work() {
    let server = MockServer::start().await;
    let failed = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    Mock::given(path("/job-take-batch/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"id":"bad","input":{}},{"id":"good","input":{}}])))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let sent = failed.clone();
    Mock::given(path("/job-done/bad"))
        .respond_with(move |_: &Request| {
            sent.notify_one();
            ResponseTemplate::new(403)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-done/good"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut settings = config(&server);
    settings.concurrency = 2;
    let release = finish.clone();
    let worker = Worker::new(
        settings,
        Handler::new(move |job| {
            let release = release.clone();
            async move {
                if job.id == "good" {
                    release.notified().await;
                }
                Ok(json!("done"))
            }
        }),
    )
    .unwrap();
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), failed.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while control.current_occupancy() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(control.is_alive());
    assert!(!task.is_finished());
    control.kill_worker();
    finish.notify_one();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn aborting_run_cancels_handlers_and_clears_shared_state() {
    struct Finished(Arc<AtomicUsize>);
    impl Drop for Finished {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let server = MockServer::start().await;
    let started = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"pending","input":{}})))
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let active = started.clone();
    let end = dropped.clone();
    let worker = Worker::new(
        config(&server),
        Handler::new(move |_| {
            let active = active.clone();
            let end = end.clone();
            async move {
                let _guard = Finished(end);
                active.notify_one();
                std::future::pending::<()>().await;
                Ok(json!(null))
            }
        }),
    )
    .unwrap();
    let control = worker.control();
    let task = tokio::spawn(async move { worker.run(std::future::pending()).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(!control.is_alive());
    assert_eq!(control.current_occupancy(), 0);
    assert!(!control.stop_job("pending"));
}

#[tokio::test]
async fn concurrent_run_is_rejected_without_clearing_the_active_worker() {
    let started = Arc::new(Notify::new());
    let signal = started.clone();
    let worker = Arc::new(
        Worker::new(
            WorkerConfig {
                concurrency_modifier: Some(ConcurrencyModifier::new(move |_| {
                    signal.notify_one();
                    0
                })),
                ..WorkerConfig::default()
            },
            Handler::sync(|_| unreachable!()),
        )
        .unwrap(),
    );
    let first = worker.clone();
    let task = tokio::spawn(async move { first.run(std::future::pending()).await });
    started.notified().await;
    assert!(
        worker
            .run(std::future::pending())
            .await
            .unwrap_err()
            .to_string()
            .contains("already running")
    );
    assert!(worker.control().is_alive());
    worker.control().kill_worker();
    task.await.unwrap().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn os_sigterm_wakes_an_idle_worker() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "signal_child", "--ignored", "--nocapture"])
        .env("RUNPOD_RUST_SIGNAL_CHILD", "1")
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(line) = output.next_line().await.unwrap() {
            if line.contains("worker-ready") {
                return;
            }
        }
        panic!("child exited before signal registration");
    })
    .await
    .unwrap();
    let pid = child.id().unwrap().to_string();
    assert!(
        tokio::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .await
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn duplicate_job_ids_preserve_both_queued_invocations() {
    let server = MockServer::start().await;
    Mock::given(path("/job-take-batch/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"id":"same","input":1},{"id":"same","input":2}])))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/same"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&server)
        .await;
    let mut settings = config(&server);
    settings.concurrency = 2;
    settings.refresh_worker = true;
    let worker = Worker::new(settings, Handler::sync(|job| Ok(job.input))).unwrap();
    tokio::time::timeout(Duration::from_secs(2), worker.run(std::future::pending()))
        .await
        .unwrap()
        .unwrap();
    let mut outputs: Vec<u64> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/job-done/same")
        .map(|r| {
            serde_json::from_slice::<runpod::Value>(&r.body).unwrap()["output"]
                .as_u64()
                .unwrap()
        })
        .collect();
    outputs.sort_unstable();
    assert_eq!(outputs, vec![1, 2]);
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "child process for the SIGTERM integration test"]
async fn signal_child() {
    if std::env::var_os("RUNPOD_RUST_SIGNAL_CHILD").is_none() {
        return;
    }
    let mut worker = Worker::new(
        WorkerConfig {
            concurrency: 0,
            ..WorkerConfig::default()
        },
        Handler::sync(|_| unreachable!()),
    )
    .unwrap();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("ready", || async {
            println!("worker-ready");
            Ok(())
        });
    worker.run_until_signal().await.unwrap();
}

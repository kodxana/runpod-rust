use runpod::{
    json,
    serverless::{Handler, Worker, WorkerConfig},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, path, query_param},
};

fn config(server: &MockServer) -> WorkerConfig {
    WorkerConfig {
        worker_id: "worker".into(),
        job_get_url: Some(format!("{}/job-take/$ID?key=test", server.uri())),
        job_done_url: Some(format!("{}/job-done/$ID", server.uri())),
        job_stream_url: Some(format!("{}/job-stream/$ID", server.uri())),
        idle_interval: Duration::from_millis(1),
        ..WorkerConfig::default()
    }
}

#[tokio::test]
async fn prestart_allows_intake_but_gates_handlers() {
    let server = MockServer::start().await;
    let taken = Arc::new(Notify::new());
    let ready = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(Notify::new());
    let hits = Arc::new(AtomicUsize::new(0));
    let observed = taken.clone();
    Mock::given(path("/job-take/worker"))
        .respond_with(move |_: &wiremock::Request| {
            if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                observed.notify_one();
                ResponseTemplate::new(200).set_body_json(json!({"id":"job","input":{"value":1}}))
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
    let observed = finished.clone();
    Mock::given(path("/job-done/job"))
        .and(body_json(json!({"output":{"value":1}})))
        .respond_with(move |_: &wiremock::Request| {
            observed.notify_one();
            ResponseTemplate::new(200)
        })
        .expect(1)
        .mount(&server)
        .await;
    let handler_ready = ready.clone();
    let mut worker = Worker::new(
        config(&server),
        Handler::new(move |job| {
            let ready = handler_ready.clone();
            async move {
                assert!(ready.load(Ordering::SeqCst), "handler ran before prestart");
                Ok(job.input)
            }
        }),
    )
    .unwrap();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("warmup", move || {
            let taken = taken.clone();
            let ready = ready.clone();
            async move {
                taken.notified().await;
                ready.store(true, Ordering::SeqCst);
                Ok(())
            }
        });
    tokio::time::timeout(Duration::from_secs(2), worker.run(finished.notified()))
        .await
        .unwrap()
        .unwrap();
    assert!(worker.progress().is_empty());
}

#[tokio::test]
async fn failed_prestart_reports_claimed_job_without_running_handler() {
    let server = MockServer::start().await;
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"job","input":{}})))
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(path("/job-done/job"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut worker = Worker::new(
        config(&server),
        Handler::sync(|_| panic!("handler must not run after prestart failure")),
    )
    .unwrap();
    worker
        .prestart_hooks_mut()
        .register_prestart_hook("load", || async {
            Err(runpod::Error::InvalidInput("weights unavailable".into()))
        });
    let error = tokio::time::timeout(Duration::from_secs(2), worker.run(std::future::pending()))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("weights unavailable"));
    let requests = server.received_requests().await.unwrap();
    let delivered = requests
        .iter()
        .find(|r| r.url.path() == "/job-done/job")
        .unwrap();
    let result: runpod::Value = serde_json::from_slice(&delivered.body).unwrap();
    let error: runpod::Value = serde_json::from_str(result["error"].as_str().unwrap()).unwrap();
    assert_eq!(error["event"], "prestart_failed");
}

#[tokio::test]
async fn streamed_worker_emits_chunks_and_aggregate() {
    let server = MockServer::start().await;
    let mut configuration = config(&server);
    configuration.return_aggregate_stream = true;
    configuration.refresh_worker = true;
    let worker = Worker::new(
        configuration,
        Handler::streaming(|_| {
            futures_util::stream::iter(vec![Ok(json!("one")), Ok(json!("two"))])
        }),
    )
    .unwrap();
    Mock::given(path("/job-stream/job"))
        .and(query_param("isStream", "false"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/job-done/job"))
        .and(query_param("isStream", "true"))
        .and(body_json(json!({"output":["one","two"],"stopPod":true})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    worker
        .handle_job(runpod::serverless::WorkerJob::new("job", json!({})))
        .await
        .unwrap();
}

struct Cancelled(Arc<AtomicBool>);
impl Drop for Cancelled {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn stop_channel_cancels_one_job_without_cancelling_others() {
    let server = MockServer::start().await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicBool::new(false));
    Mock::given(path("/job-take-batch/worker"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"id":"a","input":{}},{"id":"b","input":{}}])),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-take/worker"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let active = started.clone();
    Mock::given(path("/job-stop/worker"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(if active.load(Ordering::SeqCst) {
                json!({"jobsToStop":["a"]})
            } else {
                json!({"jobsToStop":[]})
            })
        })
        .mount(&server)
        .await;
    let observed = sent.clone();
    Mock::given(path("/job-done/b"))
        .and(body_json(json!({"output":"done"})))
        .respond_with(move |_: &wiremock::Request| {
            observed.store(true, Ordering::SeqCst);
            ResponseTemplate::new(200)
        })
        .expect(1)
        .mount(&server)
        .await;
    let flag = cancelled.clone();
    let mut configuration = config(&server);
    configuration.concurrency = 2;
    let worker = Worker::new(
        configuration,
        Handler::new(move |job| {
            let cancelled = flag.clone();
            let started = started.clone();
            async move {
                if job.id == "a" {
                    let _guard = Cancelled(cancelled);
                    started.store(true, Ordering::SeqCst);
                    std::future::pending::<()>().await;
                }
                Ok(json!("done"))
            }
        }),
    )
    .unwrap();
    let shutdown = async {
        while !(cancelled.load(Ordering::SeqCst) && sent.load(Ordering::SeqCst)) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(2), worker.run(shutdown))
        .await
        .unwrap()
        .unwrap();
    assert!(worker.progress().is_empty());
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path() == "/job-done/a")
    );
}

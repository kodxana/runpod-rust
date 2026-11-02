use runpod::{
    Error, Value,
    apps::{App, FunctionHandle, NativeFunction, ResourceSpec, targets::FunctionDefinition},
    json,
    serverless::{WorkerConfig, WorkerJob},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, path, query_param},
};

fn register(body: NativeFunction) -> FunctionHandle {
    App::new("worker-tests")
        .unwrap()
        .queue(
            ResourceSpec::default(),
            FunctionDefinition {
                name: "echo".into(),
                parameters: vec!["x".into()],
                source: None,
            },
            body,
        )
        .unwrap()
}

fn config(server: &MockServer) -> WorkerConfig {
    WorkerConfig {
        worker_id: "worker".into(),
        job_get_url: Some(format!("{}/job-take/$ID", server.uri())),
        job_done_url: Some(format!("{}/job-done/$ID", server.uri())),
        job_stream_url: Some(format!("{}/job-stream/$ID", server.uri())),
        idle_interval: Duration::from_millis(1),
        ..WorkerConfig::default()
    }
}

#[tokio::test]
async fn init_completes_before_intake_and_runs_once_for_multiple_jobs() {
    let server = MockServer::start().await;
    let init_calls = Arc::new(AtomicUsize::new(0));
    let observed = init_calls.clone();
    let handle = register(NativeFunction::new(move |call| {
        let observed = observed.clone();
        async move {
            assert_eq!(call.args, Vec::<Value>::new());
            assert!(!call.kwargs.contains_key("__empty"));
            Ok(json!({"value":call.kwargs["x"], "init":observed.load(Ordering::SeqCst)}))
        }
    }));
    let entering = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let entered = entering.clone();
    let proceed = release.clone();
    let initialized = init_calls.clone();
    handle
        .init(NativeFunction::new(move |_| {
            let entered = entered.clone();
            let proceed = proceed.clone();
            let initialized = initialized.clone();
            async move {
                entered.notify_one();
                proceed.notified().await;
                initialized.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Null)
            }
        }))
        .unwrap();
    let configuration = config(&server);
    let building = tokio::spawn(async move { handle.queue_worker(configuration).await });
    tokio::time::timeout(Duration::from_secs(3), entering.notified())
        .await
        .unwrap();
    assert!(!building.is_finished());
    assert!(server.received_requests().await.unwrap().is_empty());
    release.notify_one();
    let worker = building.await.unwrap().unwrap();
    let taken = AtomicUsize::new(0);
    Mock::given(path("/job-take/worker"))
        .respond_with(move |_: &wiremock::Request| {
            let n = taken.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":n.to_string(),"input":{"x":n,"__empty":false}}))
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
    let finished = Arc::new(Notify::new());
    for n in 0..2 {
        let finished = finished.clone();
        Mock::given(path(format!("/job-done/{n}")))
            .and(body_json(json!({"output":{"value":n,"init":1}})))
            .respond_with(move |_: &wiremock::Request| {
                if n == 1 {
                    finished.notify_one();
                }
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;
    }
    tokio::time::timeout(Duration::from_secs(3), worker.run(finished.notified()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(init_calls.load(Ordering::SeqCst), 1);
    assert!(worker.progress().is_empty());
}

#[tokio::test]
async fn generator_delivers_incremental_outputs_and_final_aggregate() {
    let server = MockServer::start().await;
    let delivered = Arc::new(Notify::new());
    let ready = delivered.clone();
    let handle = register(NativeFunction::streaming(move |_| {
        let delivered = delivered.clone();
        async_stream::try_stream! {
            yield json!("one");
            delivered.notified().await;
            yield json!("two");
        }
    }));
    Mock::given(path("/job-stream/job"))
        .and(body_json(json!({"output":"one"})))
        .and(query_param("isStream", "false"))
        .respond_with(move |_: &wiremock::Request| {
            ready.notify_one();
            ResponseTemplate::new(200)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-stream/job"))
        .and(body_json(json!({"output":"two"})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-done/job"))
        .and(body_json(json!({"output":["one","two"]})))
        .and(query_param("isStream", "true"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let worker = handle.queue_worker(config(&server)).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        worker.handle_job(WorkerJob::new("job", json!({"__empty":true}))),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn generator_failure_preserves_sent_chunks_and_reports_terminal_error() {
    let server = MockServer::start().await;
    let handle = register(NativeFunction::streaming(|_| {
        futures_util::stream::iter([
            Ok(json!("partial")),
            Err(Error::InvalidInput("generator failed".into())),
            Ok(json!("unreachable")),
        ])
    }));
    Mock::given(path("/job-stream/job"))
        .and(body_json(json!({"output":"partial"})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/job-done/job"))
        .and(query_param("isStream", "true"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    handle
        .queue_worker(config(&server))
        .await
        .unwrap()
        .handle_job(WorkerJob::new("job", json!({})))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let result: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("generator failed")
    );
    assert!(result.get("output").is_none());
}

#[tokio::test]
async fn init_failure_retains_typed_cause_and_never_starts_transport_or_body() {
    let server = MockServer::start().await;
    let handle = register(NativeFunction::sync(|_| panic!("uninitialized body ran")));
    handle
        .init(NativeFunction::new(|_| async {
            Err(Error::Io(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            )))
        }))
        .unwrap();
    let error = handle.queue_worker(config(&server)).await.unwrap_err();
    assert!(matches!(error, Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn cancelled_initialization_drops_owned_future_without_starting_worker() {
    struct Dropped(Arc<AtomicUsize>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let server = MockServer::start().await;
    let entering = Arc::new(Notify::new());
    let started = entering.clone();
    let drops = Arc::new(AtomicUsize::new(0));
    let dropped = drops.clone();
    let handle = register(NativeFunction::sync(|_| panic!("cancelled init ran body")));
    handle
        .init(NativeFunction::new(move |_| {
            let started = started.clone();
            let dropped = dropped.clone();
            async move {
                let _guard = Dropped(dropped);
                started.notify_one();
                std::future::pending::<runpod::Result<Value>>().await
            }
        }))
        .unwrap();
    let configuration = config(&server);
    let building = tokio::spawn(async move { handle.queue_worker(configuration).await });
    tokio::time::timeout(Duration::from_secs(3), entering.notified())
        .await
        .unwrap();
    building.abort();
    assert!(building.await.unwrap_err().is_cancelled());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn constructed_worker_retains_body_and_init_snapshot() {
    let handle = register(NativeFunction::sync(|call| Ok(json!(call.kwargs))));
    let worker = handle.queue_worker(WorkerConfig::default()).await.unwrap();
    handle
        .init(NativeFunction::sync(|_| {
            panic!("new init leaked into prepared worker")
        }))
        .unwrap();
    drop(handle);
    assert_eq!(
        worker.run_local(json!({"input":{"x":3}})).await.unwrap(),
        json!({"output":{"x":3}})
    );
}

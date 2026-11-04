use runpod::{
    json,
    serverless::{Handler, Heartbeat, HeartbeatOptions, Worker, WorkerApi, WorkerConfig},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use tokio::sync::{Notify, oneshot};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

fn config(server: &MockServer) -> WorkerConfig {
    WorkerConfig {
        worker_id: "worker".into(),
        api_key: "test-token".into(),
        ping_url: Some(format!("{}/ping", server.uri())),
        ping_interval: Duration::from_millis(30),
        job_get_url: Some(format!("{}/take", server.uri())),
        job_done_url: Some(format!("{}/done/$ID", server.uri())),
        idle_interval: Duration::from_millis(10),
        ..WorkerConfig::default()
    }
}

// Always release the intentionally blocked runtime, including during assertion unwinding.
struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

fn blocking_handler(entered: Arc<Notify>) -> (Handler, Release) {
    let (sender, receiver) = mpsc::channel();
    let receiver = Arc::new(Mutex::new(receiver));
    let handler = Handler::new(move |_| {
        let receiver = receiver.clone();
        let entered = entered.clone();
        async move {
            entered.notify_one();
            receiver
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .expect("test released stalled handler");
            Ok(json!(42))
        }
    });
    (handler, Release(Some(sender)))
}

async fn pings_with_job(server: &MockServer, id: &str, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let requests = server.received_requests().await.unwrap();
            let pings = requests
                .iter()
                .filter(|request| {
                    request.url.path() == "/ping"
                        && request
                            .url
                            .query_pairs()
                            .any(|(k, v)| k == "job_id" && v == id)
                })
                .count();
            if pings >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("independent heartbeat continues on a stalled worker");
}

async fn assert_stopped(server: &MockServer) {
    let count = server.received_requests().await.unwrap().len();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        count,
        "worker owns heartbeat lifetime"
    );
}

#[tokio::test]
async fn queue_heartbeat_survives_stalled_async_handler_and_stops_with_worker() {
    let server = MockServer::start().await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(path("/take"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"job", "input":null})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/done/job"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let entered = Arc::new(Notify::new());
    let (handler, release) = blocking_handler(entered.clone());
    let cfg = config(&server);
    let (stop, stopped) = oneshot::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                Worker::new(cfg, handler)
                    .unwrap()
                    .run(async {
                        let _ = stopped.await;
                    })
                    .await
            })
    });
    tokio::time::timeout(Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    pings_with_job(&server, "job", 3).await;
    stop.send(()).unwrap();
    drop(release);
    tokio::task::spawn_blocking(move || thread.join().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_stopped(&server).await;
}

#[tokio::test]
async fn http_heartbeat_survives_stalled_async_handler_and_stops_with_server() {
    let server = MockServer::start().await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let entered = Arc::new(Notify::new());
    let (handler, release) = blocking_handler(entered.clone());
    let cfg = config(&server);
    let (stop, stopped) = oneshot::channel();
    let (address, bound) = oneshot::channel();
    let thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                address.send(listener.local_addr().unwrap()).unwrap();
                WorkerApi::new(cfg, handler)
                    .unwrap()
                    .with_endpoint_id("ep")
                    .serve(listener, async {
                        let _ = stopped.await;
                    })
                    .await
            })
    });
    let address = bound.await.unwrap();
    let request = tokio::spawn(
        reqwest::Client::new()
            .post(format!("http://{address}/ep/realtime"))
            .json(&json!({"id":"live", "input":false}))
            .send(),
    );
    tokio::time::timeout(Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    pings_with_job(&server, "live", 3).await;
    stop.send(()).unwrap();
    drop(release);
    assert!(request.await.unwrap().unwrap().status().is_success());
    tokio::task::spawn_blocking(move || thread.join().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_stopped(&server).await;
}

#[tokio::test]
async fn stop_interrupts_request_retry_and_interval_without_waiting_for_deadlines() {
    for response in [
        ResponseTemplate::new(200).set_delay(Duration::from_secs(30)),
        ResponseTemplate::new(503).insert_header("Retry-After", "21600"),
        ResponseTemplate::new(200),
    ] {
        let server = MockServer::start().await;
        let seen = Arc::new(Notify::new());
        let signal = seen.clone();
        Mock::given(path("/ping"))
            .respond_with(move |_: &wiremock::Request| {
                signal.notify_one();
                response.clone()
            })
            .mount(&server)
            .await;
        let mut cfg = config(&server);
        cfg.ping_interval = Duration::from_secs(60);
        let handle = Heartbeat::new(cfg)
            .unwrap()
            .start_ping(None)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(30), seen.notified())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::time::timeout(Duration::from_secs(2), handle.stop())
            .await
            .expect("prompt stop")
            .unwrap();
    }
}

#[tokio::test]
async fn dropping_worker_future_cancels_heartbeat() {
    let server = MockServer::start().await;
    let seen = Arc::new(Notify::new());
    let signal = seen.clone();
    Mock::given(path("/ping"))
        .respond_with(move |_: &wiremock::Request| {
            signal.notify_one();
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    let cfg = config(&server);
    let worker = tokio::spawn(async move {
        Worker::new(cfg, Handler::sync(|_| Ok(json!(null))))
            .unwrap()
            .run(std::future::pending())
            .await
    });
    tokio::time::timeout(Duration::from_secs(30), seen.notified())
        .await
        .unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    // Drop signals a nonblocking stop; allow an in-flight request to arrive before counting.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_stopped(&server).await;
}

#[tokio::test]
async fn child_reconstructs_default_retries_and_mirror_snapshot_is_fixed_across_retries() {
    let server = MockServer::start().await;
    let mirror = runpod::serverless::PingJobMirror::new();
    mirror.set(Some("original"));
    let changed = mirror.clone();
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    Mock::given(path("/ping"))
        .respond_with(move |_: &wiremock::Request| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                changed.set(Some("changed"));
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200)
            }
        })
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.ping_interval = Duration::from_secs(60);
    let handle = Heartbeat::with_options(
        cfg,
        HeartbeatOptions {
            retries: 0,
            pool_connections: 1,
        },
    )
    .unwrap()
    .start_ping(Some(mirror))
    .await
    .unwrap()
    .unwrap();
    pings_with_job(&server, "original", 2).await;
    handle.stop().await.unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 2);
}

use runpod::{
    Value,
    apps::{
        App, FunctionHandle, NativeFunction, ResourceSpec,
        targets::FunctionDefinition,
        task_runner::{TaskExit, TaskRunner, TaskRunnerConfig},
    },
    json,
};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Notify, oneshot},
    task::JoinHandle,
};

fn handle(body: NativeFunction) -> FunctionHandle {
    App::new("tasks")
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
struct Server {
    url: String,
    stop: oneshot::Sender<()>,
    task: JoinHandle<runpod::Result<TaskExit>>,
}

#[tokio::test]
async fn deadline_termination_does_not_wait_for_an_unfinished_request_body() {
    use tokio::io::AsyncWriteExt;
    let task = handle(NativeFunction::sync(|_| Ok(json!(42))));
    let mut server = Server::spawn(
        TaskRunner::new("token", TaskRunnerConfig::default()).unwrap(),
        async { Ok(task) },
        || async { Ok(true) },
    )
    .await;
    assert_eq!(server.ready().await.0, 200);
    assert_eq!(
        server
            .request("/timeout", Some(json!({"timeout":0.1})))
            .await
            .0,
        200
    );
    let mut connection =
        tokio::net::TcpStream::connect(server.url.strip_prefix("http://").unwrap())
            .await
            .unwrap();
    connection.write_all(b"POST /execute HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer token\r\nContent-Length: 10000\r\n\r\n{").await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(1), &mut server.task).await;
    drop(connection);
    assert_eq!(
        outcome
            .expect("deletion must not wait for the client body")
            .unwrap()
            .unwrap(),
        TaskExit::Terminated
    );
}
impl Server {
    async fn spawn<F, T, TF>(runner: TaskRunner, initialize: F, terminate: T) -> Self
    where
        F: Future<Output = runpod::Result<FunctionHandle>> + Send + 'static,
        T: FnMut() -> TF + Send + 'static,
        TF: Future<Output = runpod::Result<bool>> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(runner.serve(listener, initialize, terminate, async {
            let _ = stopped.await;
        }));
        Self { url, stop, task }
    }
    async fn request(&self, path: &str, body: Option<Value>) -> (u16, Value) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let request = if let Some(body) = body {
            client.post(format!("{}{path}", self.url)).json(&body)
        } else {
            client.get(format!("{}{path}", self.url))
        };
        let result = request.bearer_auth("token").send().await.unwrap();
        (result.status().as_u16(), result.json().await.unwrap())
    }
    async fn ready(&self) -> (u16, Value) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let response = self.request("/ping", None).await;
                if response.0 != 503 {
                    return response;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
    async fn close(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

async fn invalid_cap(key: &str) {
    for value in ["", "0", "-1", "nan", "inf", "text"] {
        let values = BTreeMap::from([
            ("RUNPOD_TASK_TOKEN".into(), "token".into()),
            (key.into(), value.into()),
        ]);
        let runner = TaskRunner::from_values(&values).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let server = Server::spawn(
            runner,
            async move {
                called.fetch_add(1, Ordering::SeqCst);
                Ok(handle(NativeFunction::sync(|_| Ok(Value::Null))))
            },
            || async { Ok(false) },
        )
        .await;
        assert_eq!(
            server.ready().await,
            (500, json!({"ready":false,"error_type":"BootstrapError"})),
            "{key}={value}"
        );
        assert_eq!(
            server.request("/result", None).await.1["error_message"],
            format!("task initialization failed: {key} must be finite and positive")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.close().await;
    }
}
#[tokio::test]
async fn test_invalid_configured_cap_fails_readiness() {
    invalid_cap("RUNPOD_TASK_EXECUTION_TIMEOUT_SECONDS").await;
}
#[tokio::test]
async fn test_invalid_initialization_timeout_fails_before_loading() {
    invalid_cap("RUNPOD_TASK_INIT_TIMEOUT").await;
}

#[tokio::test]
async fn initialization_timeout_keeps_private_error_and_retries_deletion() {
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let config = TaskRunnerConfig {
        init_timeout: Duration::from_millis(40),
        watchdog_interval: Duration::from_millis(20),
        ..TaskRunnerConfig::default()
    };
    let server = Server::spawn(
        TaskRunner::new("token", config).unwrap(),
        std::future::pending(),
        move || {
            called.fetch_add(1, Ordering::SeqCst);
            async { Ok(false) }
        },
    )
    .await;
    assert_eq!(
        server.ready().await,
        (500, json!({"ready":false,"error_type":"BootstrapError"}))
    );
    assert_eq!(
        server.request("/result", None).await.1["error_message"],
        "task initialization timed out"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    server.close().await;
}

#[tokio::test]
async fn test_idle_watchdog_waits_for_initialization() {
    let release = Arc::new(Notify::new());
    let proceed = release.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let config = TaskRunnerConfig {
        idle_timeout: Duration::ZERO,
        watchdog_interval: Duration::from_millis(10),
        ..TaskRunnerConfig::default()
    };
    let server = Server::spawn(
        TaskRunner::new("token", config).unwrap(),
        async move {
            proceed.notified().await;
            Ok(handle(NativeFunction::sync(|_| Ok(Value::Null))))
        },
        move || {
            called.fetch_add(1, Ordering::SeqCst);
            async { Ok(false) }
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(server.request("/ping", None).await.0, 503);
    release.notify_one();
    assert_eq!(server.ready().await.0, 200);
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    server.close().await;
}

#[tokio::test]
async fn inline_disconnect_preserves_execution_and_single_shot_state() {
    use tokio::io::AsyncWriteExt;
    let entered = Arc::new(Notify::new());
    let running = entered.clone();
    let release = Arc::new(Notify::new());
    let proceed = release.clone();
    let task = handle(NativeFunction::new(move |_| {
        let entered = entered.clone();
        let proceed = proceed.clone();
        async move {
            entered.notify_one();
            proceed.notified().await;
            Ok(json!(42))
        }
    }));
    let server = Server::spawn(
        TaskRunner::new("token", TaskRunnerConfig::default()).unwrap(),
        async { Ok(task) },
        || async { Ok(false) },
    )
    .await;
    assert_eq!(server.ready().await.0, 200);
    let mut connection =
        tokio::net::TcpStream::connect(server.url.strip_prefix("http://").unwrap())
            .await
            .unwrap();
    connection.write_all(b"POST /execute HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer token\r\nContent-Length: 2\r\n\r\n{}").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), running.notified())
        .await
        .unwrap();
    drop(connection);
    assert_eq!(
        server.request("/result", None).await.1,
        json!({"status":"NONE","response":null})
    );
    assert_eq!(server.request("/submit", Some(json!({}))).await.0, 409);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.request("/result", None).await.1["status"] != "DONE" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        server.request("/result", None).await.1,
        json!({"status":"DONE","response":null})
    );
    server.close().await;
}

#[tokio::test]
async fn simultaneous_submissions_execute_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let called = calls.clone();
    let task = handle(NativeFunction::sync(move |_| {
        called.fetch_add(1, Ordering::SeqCst);
        Ok(json!(42))
    }));
    let server = Server::spawn(
        TaskRunner::new("token", TaskRunnerConfig::default()).unwrap(),
        async { Ok(task) },
        || async { Ok(false) },
    )
    .await;
    assert_eq!(server.ready().await.0, 200);
    let responses =
        futures_util::future::join_all((0..12).map(|_| server.request("/submit", Some(json!({})))))
            .await;
    assert_eq!(responses.iter().filter(|r| r.0 == 200).count(), 1);
    assert_eq!(responses.iter().filter(|r| r.0 == 409).count(), 11);
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.request("/result", None).await.1["status"] != "DONE" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.close().await;
}

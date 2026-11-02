use runpod::{
    Error, Value,
    apps::{
        App, FunctionHandle, NativeFunction, ResourceSpec,
        targets::FunctionDefinition,
        task_runner::{TaskExit, TaskRunner, TaskRunnerConfig},
    },
    json,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Notify, oneshot},
};

fn handle(function: NativeFunction) -> FunctionHandle {
    App::new("tasks")
        .unwrap()
        .task(
            ResourceSpec::default(),
            FunctionDefinition {
                name: "work".into(),
                parameters: vec![],
                source: None,
            },
            function,
        )
        .unwrap()
}

async fn request(
    base: &str,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (u16, Value) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut request = client.request(method.parse().unwrap(), format!("{base}{path}"));
    if method == "POST" {
        request = request.json(&body);
    }
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

async fn wait_ready(base: &str) -> (u16, Value) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let result = request(base, "GET", "/ping", Value::Null, None).await;
            if result.0 != 503 {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn test_registered_task_submit_result_and_single_shot() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = handle(NativeFunction::sync(|call| {
        runpod::serverless::capture::stdout().write_text("task output\n")?;
        Ok(json!({"value":call.args[0]}))
    }));
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(
        TaskRunner::new("token", TaskRunnerConfig::default())
            .unwrap()
            .serve(
                listener,
                async { Ok(task) },
                || async { Ok(false) },
                async {
                    let _ = stopped.await;
                },
            ),
    );
    assert_eq!(wait_ready(&base).await, (200, json!({"ready":true})));
    assert_eq!(
        request(
            &base,
            "POST",
            "/submit",
            json!({"args":[42]}),
            Some("token")
        )
        .await,
        (200, json!({"status":"RUNNING"}))
    );
    let response = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (_, body) = request(&base, "GET", "/result", Value::Null, Some("token")).await;
            if body["status"] == "DONE" {
                return body;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        response,
        json!({"status":"DONE","response":{"success":true,"json_result":{"value":42},"stdout":"task output\n"}})
    );
    assert_eq!(
        request(&base, "POST", "/submit", json!({}), Some("token"))
            .await
            .0,
        409
    );
    assert_eq!(
        request(&base, "POST", "/execute", json!({}), Some("token"))
            .await
            .0,
        409
    );
    stop.send(()).unwrap();
    assert_eq!(server.await.unwrap().unwrap(), TaskExit::Shutdown);
}

#[tokio::test]
async fn test_production_readiness_waits_for_init() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let release = Arc::new(Notify::new());
    let proceed = release.clone();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(
        TaskRunner::new("token", TaskRunnerConfig::default())
            .unwrap()
            .serve(
                listener,
                async move {
                    proceed.notified().await;
                    Ok(handle(NativeFunction::sync(|_| Ok(json!(42)))))
                },
                || async { Ok(false) },
                async {
                    let _ = stopped.await;
                },
            ),
    );
    assert_eq!(
        request(&base, "GET", "/ping", Value::Null, None).await,
        (503, json!({"ready":false}))
    );
    assert_eq!(
        request(&base, "POST", "/submit", json!({}), Some("token"))
            .await
            .0,
        503
    );
    release.notify_one();
    assert_eq!(wait_ready(&base).await.0, 200);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_init_failure_has_task_error_surface() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(
        TaskRunner::new("token", TaskRunnerConfig::default())
            .unwrap()
            .serve(
                listener,
                async { Err(Error::InvalidInput("private failure".into())) },
                || async { Ok(false) },
                async {
                    let _ = stopped.await;
                },
            ),
    );
    assert_eq!(
        wait_ready(&base).await,
        (500, json!({"ready":false,"error_type":"BootstrapError"}))
    );
    assert_eq!(
        request(&base, "GET", "/result", Value::Null, Some("token")).await,
        (
            500,
            json!({"ready":false,"error_type":"BootstrapError","error_message":"task initialization failed: private failure"})
        )
    );
    assert_eq!(
        request(&base, "POST", "/submit", json!({}), Some("wrong"))
            .await
            .0,
        401
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn timeout_survives_client_exit_and_failed_termination() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let entered = Arc::new(Notify::new());
    let running = entered.clone();
    let task = handle(NativeFunction::new(move |_| {
        let entered = entered.clone();
        async move {
            entered.notify_one();
            std::future::pending::<runpod::Result<Value>>().await
        }
    }));
    let calls = Arc::new(AtomicUsize::new(0));
    let attempts = calls.clone();
    let config = TaskRunnerConfig {
        watchdog_interval: Duration::from_millis(20),
        ..TaskRunnerConfig::default()
    };
    let server = tokio::spawn(TaskRunner::new("token", config).unwrap().serve(
        listener,
        async { Ok(task) },
        move || {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move { Ok(n > 0) }
        },
        std::future::pending(),
    ));
    assert_eq!(wait_ready(&base).await.0, 200);
    assert_eq!(
        request(
            &base,
            "POST",
            "/submit",
            json!({"timeout":0.08}),
            Some("token")
        )
        .await
        .0,
        200
    );
    running.notified().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        TaskExit::Terminated
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

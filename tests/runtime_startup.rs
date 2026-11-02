use runpod::{
    Error, Value,
    apps::{
        App, AppRegistry, NativeApi, NativeFunction, ResourceSpec, RouteSpec,
        deploy::SymbolReference, runtime::DeployedContext, targets::FunctionDefinition,
    },
    json,
    serverless::{WorkerConfig, WorkerJob},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Notify, oneshot},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, path},
};

fn context(kind: &str, backend: &str, mount: &str) -> (tempfile::TempDir, DeployedContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("runpod_manifest.json"), json!({
        "app":"example", "resources":[{"name":"work","kind":kind,"module":"user","qualname":"work"}]
    }).to_string()).unwrap();
    let values = BTreeMap::from([
        ("RUNPOD_APP_DIR".into(), dir.path().to_str().unwrap().into()),
        ("FLASH_RESOURCE_NAME".into(), "work".into()),
        ("FLASH_APP_NAME".into(), "example".into()),
        ("FLASH_ENVIRONMENT".into(), "default".into()),
        ("RUNPOD_BUILD_ID".into(), "build".into()),
        ("RUNPOD_RUNTIME_KIND".into(), kind.into()),
        (
            "RUNPOD_MOUNTS".into(),
            json!([{"kind":backend,"reference":"data","id":"volume-id","path":mount}]).to_string(),
        ),
    ]);
    let context = DeployedContext::from_values(&values).unwrap();
    (dir, context)
}

fn registry(app: App) -> AppRegistry {
    let mut registry = AppRegistry::new();
    registry.register(app);
    registry
}
fn definition() -> FunctionDefinition {
    FunctionDefinition {
        name: "work".into(),
        parameters: vec![],
        source: None,
    }
}
fn route(method: &str, path: &str, name: &str) -> RouteSpec {
    RouteSpec {
        method: method.into(),
        path: path.into(),
        handler_name: name.into(),
    }
}

// test_queue_worker.py::TestMain.test_deployed_mode, through the real Rust worker transport.
#[tokio::test]
async fn test_deployed_mode() {
    let server = MockServer::start().await;
    let (_dir, context) = context("queue", "global", "/shared");
    let events = Arc::new(Mutex::new(Vec::new()));
    let handle = context
        .load_queue(|module, mounts| {
            assert_eq!(module, "user");
            let imported = mounts.path("global", "data")?.to_owned();
            events.lock().unwrap().push(imported.clone());
            let app = App::new("example")?;
            let mounted = mounts.clone();
            let handle = app.queue(
                ResourceSpec::default(),
                definition(),
                NativeFunction::streaming(move |call| {
                    let imported = imported.clone();
                    let mounted = mounted.clone();
                    async_stream::try_stream! {
                        yield json!({"name":call.kwargs["name"],"imported":imported});
                        let path = mounted.path("global", "volume-id")?;
                        yield json!({"mounted":path});
                    }
                }),
            )?;
            let mounted = mounts.clone();
            let initialized = events.clone();
            handle.init(NativeFunction::sync(move |_| {
                initialized
                    .lock()
                    .unwrap()
                    .push(mounted.path("global", "data")?.into());
                Ok(Value::Null)
            }))?;
            Ok(registry(app))
        })
        .unwrap();
    assert_eq!(*events.lock().unwrap(), vec!["/shared"]);
    drop(context);
    let worker = handle
        .queue_worker(WorkerConfig {
            worker_id: "worker".into(),
            job_done_url: Some(format!("{}/job-done/$ID", server.uri())),
            job_stream_url: Some(format!("{}/job-stream/$ID", server.uri())),
            ..WorkerConfig::default()
        })
        .await
        .unwrap();
    assert_eq!(*events.lock().unwrap(), vec!["/shared", "/shared"]);
    let first = json!({"name":"reader","imported":"/shared"});
    let second = json!({"mounted":"/shared"});
    for chunk in [&first, &second] {
        Mock::given(path("/job-stream/job"))
            .and(body_json(json!({"output":chunk})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/job-done/job"))
        .and(body_json(json!({"output":[first,second]})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    tokio::time::timeout(
        Duration::from_secs(3),
        worker.handle_job(WorkerJob::new(
            "job",
            json!({"name":"reader","__empty":false}),
        )),
    )
    .await
    .unwrap()
    .unwrap();
}

// test_api_server.py::TestDeployedClassApi.test_routes_and_init, using real loopback HTTP.
#[tokio::test]
async fn test_routes_and_init() {
    let (_dir, context) = context("api", "network", "/data");
    let state = Arc::new(Mutex::new(Vec::<String>::new()));
    let api = context
        .load_api(|_, mounts| {
            let imported = mounts.path("network", "data")?.to_owned();
            let app = App::new("example")?;
            let mut api = NativeApi::new("work");
            let mounted = mounts.clone();
            let state_init = state.clone();
            api.init(
                "setup",
                NativeFunction::new(move |_| {
                    let mounted = mounted.clone();
                    let state = state_init.clone();
                    let imported = imported.clone();
                    async move {
                        *state.lock().unwrap() =
                            vec![imported, mounted.path("network", "data")?.into()];
                        Ok(Value::Null)
                    }
                }),
            )?;
            let mounted = mounts.clone();
            let state_health = state.clone();
            api.route(
                route("GET", "/health", "health"),
                NativeFunction::sync(move |_| {
                    let mut paths = state_health.lock().unwrap().clone();
                    paths.push(mounted.path("network", "volume-id")?.into());
                    Ok(json!({"model":"loaded", "mounts":paths}))
                }),
            )?;
            api.route(
                route("POST", "/generate", "generate"),
                NativeFunction::new(|call| async move {
                    Ok(json!({"echo":call.args[0],"model":"loaded"}))
                }),
            )?;
            app.api(ResourceSpec::default(), api)?;
            Ok(registry(app))
        })
        .unwrap();
    assert!(state.lock().unwrap().is_empty());
    drop(context);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stopped.await;
        })
        .await
    });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert_eq!(
        http.get(format!("{address}/ping"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        json!({"status":"healthy"})
    );
    assert_eq!(
        http.get(format!("{address}/health"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        json!({"model":"loaded","mounts":["/data","/data","/data"]})
    );
    assert_eq!(
        http.post(format!("{address}/generate"))
            .json(&json!({"prompt":"hi"}))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        json!({"echo":{"prompt":"hi"},"model":"loaded"})
    );
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), serving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn task_init_waits_and_retains_async_state_for_execution() {
    let (_dir, context) = context("task", "network", "/data");
    let entering = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let ready = Arc::new(Notify::new());
    let entered = entering.clone();
    let proceed = release.clone();
    let loading = tokio::spawn(async move {
        context
            .load_task(|_, mounts| {
                let app = App::new("example")?;
                let ready_body = ready.clone();
                let mounted = mounts.clone();
                let handle = app.task(
                    ResourceSpec::default(),
                    definition(),
                    NativeFunction::new(move |_| {
                        let ready = ready_body.clone();
                        let mounted = mounted.clone();
                        async move {
                            ready.notified().await;
                            Ok(json!(mounted.path("network", "data")?))
                        }
                    }),
                )?;
                handle.set_symbol(SymbolReference {
                    module: "user".into(),
                    qualname: "work".into(),
                })?;
                handle.init(NativeFunction::new(move |_| {
                    let entered = entered.clone();
                    let proceed = proceed.clone();
                    let ready = ready.clone();
                    async move {
                        entered.notify_one();
                        proceed.notified().await;
                        ready.notify_one();
                        Ok(Value::Null)
                    }
                }))?;
                Ok(registry(app))
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), entering.notified())
        .await
        .unwrap();
    assert!(!loading.is_finished());
    release.notify_one();
    let handle = loading.await.unwrap().unwrap();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(3),
            handle.local(vec![], serde_json::Map::new())
        )
        .await
        .unwrap()
        .unwrap(),
        json!("/data")
    );
}

#[tokio::test]
async fn task_init_failure_preserves_native_error() {
    let (_dir, context) = context("task", "network", "/data");
    let error = context
        .load_task(|_, _| {
            let app = App::new("example")?;
            let handle = app.task(
                ResourceSpec::default(),
                definition(),
                NativeFunction::sync(|_| panic!("failed init must prevent execution")),
            )?;
            handle.set_symbol(SymbolReference {
                module: "user".into(),
                qualname: "work".into(),
            })?;
            handle.init(NativeFunction::new(|_| async {
                Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "init denied").into())
            }))?;
            Ok(registry(app))
        })
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied));
}

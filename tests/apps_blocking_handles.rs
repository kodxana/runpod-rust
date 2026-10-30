use runpod::{
    Value,
    apps::{
        AppConfig, ExecutionContext, NativeApi, NativeFunction, ResourceSpec, RouteSpec,
        targets::{FunctionDefinition, JobOptions},
    },
    json,
};

fn definition(name: &str) -> FunctionDefinition {
    FunctionDefinition {
        name: name.into(),
        parameters: vec!["x".into()],
        source: None,
    }
}

#[test]
fn blocking_local_bodies_and_streams_keep_runtime_after_app_is_dropped() {
    let app = runpod::blocking::apps::App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::default()),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let function = app
        .queue(
            ResourceSpec::default(),
            definition("echo"),
            NativeFunction::new(|call| async move { Ok(json!(call.args)) }),
        )
        .unwrap();
    let generator = app
        .queue(
            ResourceSpec::default(),
            definition("gen"),
            NativeFunction::streaming(|_| futures_util::stream::iter([Ok(json!(1)), Ok(json!(2))])),
        )
        .unwrap();
    let output = generator.local_stream(vec![], Default::default());
    let task = app
        .task(
            ResourceSpec::default(),
            definition("task"),
            NativeFunction::sync(|_| Ok(json!("task result"))),
        )
        .unwrap();
    drop(app);
    drop(generator);
    assert_eq!(
        task.local(vec![], Default::default()).unwrap(),
        json!("task result")
    );
    assert_eq!(
        function.local(vec![json!(42)], Default::default()).unwrap(),
        json!([42])
    );
    assert_eq!(
        output.collect::<runpod::Result<Vec<_>>>().unwrap(),
        vec![json!(1), json!(2)]
    );
}

#[test]
fn blocking_worker_uses_native_body_and_initializes_only_when_requested() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let app = runpod::blocking::apps::App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::from_values([
                ("RUNPOD_POD_ID", "pod"),
                ("FLASH_RESOURCE_NAME", "echo"),
            ])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let function = app
        .queue(
            ResourceSpec::default(),
            definition("echo"),
            NativeFunction::sync(|call| Ok(json!(call.args))),
        )
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let initialized = count.clone();
    function
        .as_async()
        .init(NativeFunction::sync(move |_| {
            initialized.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        }))
        .unwrap();
    assert_eq!(
        function
            .remote(vec![json!(42)], Default::default())
            .unwrap(),
        json!([42])
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    function.initialize().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn blocking_app_creation_rejects_a_nested_runtime() {
    let error = runpod::blocking::apps::App::new("demo").unwrap_err();
    assert!(error.to_string().contains("outside a Tokio runtime"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_function_submit_reconnect_and_options_use_owned_runtime() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, path},
    };
    let server = MockServer::start().await;
    Mock::given(path("/flash/runsync"))
        .and(body_json(json!({"input":{"x":2},"policy":{"ttl":7}})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"status":"COMPLETED","output":4})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/run"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/status/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","status":"COMPLETED","output":4})),
        )
        .expect(2)
        .mount(&server)
        .await;
    let uri = server.uri();
    std::thread::spawn(move || {
        let client = runpod::Client::with_config(runpod::ClientConfig {
            endpoint_base_url: uri,
            ..runpod::ClientConfig::new("key")
        })
        .unwrap();
        let app = runpod::blocking::apps::App::with_config(
            "demo",
            AppConfig {
                client: Some(client),
                context: Some(ExecutionContext::default()),
                ..AppConfig::default()
            },
        )
        .unwrap();
        let function = app
            .queue(
                ResourceSpec::default(),
                definition("echo"),
                NativeFunction::sync(|_| panic!("remote")),
            )
            .unwrap();
        let bounded = function
            .with_options(&JobOptions {
                ttl: Some(7),
                ..JobOptions::default()
            })
            .unwrap();
        assert_eq!(
            bounded.remote(vec![json!(2)], Default::default()).unwrap(),
            json!(4)
        );
        let mut submitted = function.spawn(vec![json!(2)], Default::default()).unwrap();
        let mut reconnected = function.job("job").unwrap();
        drop(app);
        drop(function);
        drop(bounded);
        assert_eq!(submitted.wait(None).unwrap(), json!(4));
        assert_eq!(
            reconnected
                .result(std::time::Duration::from_secs(1))
                .unwrap(),
            json!(4)
        );
    })
    .join()
    .unwrap();
}

#[test]
fn blocking_api_calls_native_registered_handler_and_preserves_resource_kind() {
    let app = runpod::blocking::apps::App::new("demo").unwrap();
    let mut api = NativeApi::new("api");
    api.route(
        RouteSpec {
            method: "GET".into(),
            path: "/x".into(),
            handler_name: "read".into(),
        },
        NativeFunction::new(|_| async { Ok(json!(42)) }),
    )
    .unwrap();
    let api = app.api(ResourceSpec::default(), api).unwrap();
    assert!(matches!(
        &app.resources().unwrap()[0],
        runpod::blocking::apps::ResourceHandle::Api(_)
    ));
    drop(app);
    api.initialize().unwrap();
    assert_eq!(
        api.call_handler("read", Default::default()).unwrap(),
        json!(42)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_api_http_verbs_keep_runtime_after_app_drop() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    for verb in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
        Mock::given(method(verb))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(verb)))
            .expect(1)
            .mount(&server)
            .await;
    }
    let uri = server.uri();
    std::thread::spawn(move || {
        let app = runpod::blocking::apps::App::with_config(
            "demo",
            AppConfig {
                context: Some(ExecutionContext::from_values([("RUNPOD_DEV_SESSION", "1")])),
                ..AppConfig::default()
            },
        )
        .unwrap();
        let mut target =
            runpod::apps::targets::Target::live(runpod::Client::new("key").unwrap(), "ep", "api")
                .unwrap();
        target.set_load_balancer_url(uri).unwrap();
        app.as_async().set_dev_target("api", target).unwrap();
        let mut service = NativeApi::new("api");
        service
            .route(
                RouteSpec {
                    method: "GET".into(),
                    path: "/x".into(),
                    handler_name: "read".into(),
                },
                NativeFunction::sync(|_| panic!("HTTP only")),
            )
            .unwrap();
        let api = app.api(ResourceSpec::default(), service).unwrap();
        drop(app);
        assert_eq!(api.get("/x", None).unwrap(), json!("GET"));
        assert_eq!(api.post("/x", None).unwrap(), json!("POST"));
        assert_eq!(api.put("/x", None).unwrap(), json!("PUT"));
        assert_eq!(api.delete("/x", None).unwrap(), json!("DELETE"));
        assert_eq!(api.patch("/x", None).unwrap(), json!("PATCH"));
    })
    .join()
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_remote_stream_and_cancel_preserve_job_protocol() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    let server = MockServer::start().await;
    Mock::given(path("/flash/run"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/flash/stream/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status":"COMPLETED","stream":[{"output":1},{"output":2}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/cancel/job"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"CANCELLED"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let uri = server.uri();
    std::thread::spawn(move || {
        let client = runpod::Client::with_config(runpod::ClientConfig {
            endpoint_base_url: uri,
            ..runpod::ClientConfig::new("key")
        })
        .unwrap();
        let app = runpod::blocking::apps::App::with_config(
            "demo",
            AppConfig {
                client: Some(client),
                context: Some(ExecutionContext::default()),
                ..AppConfig::default()
            },
        )
        .unwrap();
        let generator = app
            .queue(
                ResourceSpec::default(),
                definition("gen"),
                NativeFunction::streaming(|_| futures_util::stream::empty()),
            )
            .unwrap();
        let mut submitted = generator.spawn(vec![], Default::default()).unwrap();
        let output = generator.stream(vec![], Default::default());
        drop(app);
        drop(generator);
        assert_eq!(
            output.collect::<runpod::Result<Vec<_>>>().unwrap(),
            vec![json!(1), json!(2)]
        );
        submitted.cancel().unwrap();
        assert!(submitted.as_async().queue().unwrap().done());
    })
    .join()
    .unwrap();
}

#[test]
fn worker_local_recursion_from_sync_and_async_callers() {
    use std::sync::{Arc, Mutex};
    let current = Arc::new(Mutex::new(None::<runpod::blocking::apps::FunctionHandle>));
    let weak = Arc::downgrade(&current);
    let app = runpod::blocking::apps::App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::from_values([
                ("RUNPOD_POD_ID", "pod"),
                ("RUNPOD_DEV_APP", "demo"),
                ("RUNPOD_DEV_RESOURCE", "countdown"),
            ])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let countdown = app
        .queue(
            ResourceSpec::default(),
            definition("countdown"),
            NativeFunction::sync(move |call| {
                let n = call.args[0].as_u64().unwrap();
                if n == 0 {
                    return Ok(json!(0));
                }
                let handle = weak.upgrade().unwrap().lock().unwrap().clone().unwrap();
                let value = handle.remote(vec![json!(n - 1)], Default::default())?;
                Ok(json!(value.as_u64().unwrap() + 1))
            }),
        )
        .unwrap();
    *current.lock().unwrap() = Some(countdown.clone());
    assert_eq!(
        countdown
            .remote(vec![json!(3)], Default::default())
            .unwrap(),
        json!(3)
    );
    let caller = tokio::runtime::Runtime::new().unwrap();
    caller.block_on(async {
        let calls = (0..4).map(|_| {
            countdown
                .as_async()
                .remote(vec![json!(4)], Default::default())
        });
        for result in futures_util::future::join_all(calls).await {
            assert_eq!(result.unwrap(), json!(4));
        }
    });
}

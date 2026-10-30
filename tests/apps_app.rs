//! Native equivalents of `test_app.py` and context/dispatch tests.
use futures_util::{StreamExt, stream};
use runpod::{
    Client, ClientConfig, Value,
    apps::{
        App, AppConfig, AppRegistry, Context, ExecutionContext, NativeFunction, ResourceKind,
        ResourceSpec, Workers,
        targets::{FunctionDefinition, JobOptions},
    },
    json,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn definition(name: &str) -> FunctionDefinition {
    FunctionDefinition {
        name: name.into(),
        parameters: vec!["x".into()],
        source: None,
    }
}
fn echo() -> NativeFunction {
    NativeFunction::sync(|call| Ok(call.args.first().cloned().unwrap_or(Value::Null)))
}
fn app(context: ExecutionContext, client: Option<Client>) -> App {
    App::with_config(
        "demo",
        AppConfig {
            context: Some(context),
            client,
            ..AppConfig::default()
        },
    )
    .unwrap()
}
fn local() -> App {
    app(ExecutionContext::default(), None)
}
fn client(server: &MockServer) -> Client {
    Client::with_config(ClientConfig {
        endpoint_base_url: server.uri(),
        api_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap()
}

#[test]
fn context_detection_preserves_worker_precedence_and_empty_values() {
    let cases = [
        (vec![], Context::Local),
        (vec![("RUNPOD_DEV_SESSION", "1")], Context::Dev),
        (vec![("RUNPOD_ENDPOINT_ID", "ep")], Context::Worker),
        (
            vec![("RUNPOD_POD_ID", "pod"), ("RUNPOD_DEV_SESSION", "1")],
            Context::Worker,
        ),
        (
            vec![("RUNPOD_POD_ID", ""), ("RUNPOD_DEV_SESSION", "")],
            Context::Local,
        ),
    ];
    for (vars, expected) in cases {
        let context = ExecutionContext::from_values(vars);
        assert_eq!(context.kind, expected);
        assert_eq!(context.is_local(), expected != Context::Worker);
    }
}

#[test]
fn app_registration_is_owned_ordered_and_rejects_duplicate_names() {
    assert!(App::new("").is_err());
    let mut registry = AppRegistry::default();
    let app = local();
    registry.register(app.clone());
    let queue = app
        .queue(ResourceSpec::default(), definition("first"), echo())
        .unwrap();
    let task = app
        .task(ResourceSpec::default(), definition("second"), echo())
        .unwrap();
    assert_eq!(queue.spec().unwrap().kind, ResourceKind::Queue);
    assert_eq!(task.spec().unwrap().kind, ResourceKind::Task);
    assert_eq!(registry.apps().len(), 1);
    assert_eq!(
        app.definition()
            .unwrap()
            .resources
            .iter()
            .map(|r| r.spec.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        app.task(ResourceSpec::default(), definition("first"), echo())
            .unwrap_err()
            .to_string()
            .contains("duplicate resource")
    );
    let snapshot = app.definition().unwrap();
    registry.clear();
    assert!(registry.apps().is_empty());
    assert_eq!(snapshot.resources.len(), 2);
}

#[test]
fn invalid_registration_does_not_change_resources() {
    let app = local();
    let invalid = ResourceSpec {
        workers: Some(Workers::Range([9, 1])),
        ..ResourceSpec::default()
    };
    assert!(app.queue(invalid, definition("bad"), echo()).is_err());
    assert!(app.definition().unwrap().resources.is_empty());
}

#[tokio::test]
async fn local_sync_and_async_run_without_initialization_or_credentials() {
    let app = local();
    let queue = app
        .queue(
            ResourceSpec::default(),
            definition("double"),
            NativeFunction::sync(|call| Ok(json!(call.args[0].as_i64().unwrap() * 2))),
        )
        .unwrap();
    let initialized = Arc::new(AtomicUsize::new(0));
    let count = initialized.clone();
    queue
        .init(NativeFunction::sync(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        }))
        .unwrap();
    assert_eq!(
        queue
            .local(vec![json!(21)], Default::default())
            .await
            .unwrap(),
        json!(42)
    );
    let async_fn = app
        .task(
            ResourceSpec::default(),
            definition("increment"),
            NativeFunction::new(
                |call| async move { Ok(json!(call.args[0].as_i64().unwrap() + 1)) },
            ),
        )
        .unwrap();
    assert_eq!(
        async_fn
            .local(vec![json!(1)], Default::default())
            .await
            .unwrap(),
        json!(2)
    );
    assert_eq!(initialized.load(Ordering::SeqCst), 0);
    queue.initialize().await.unwrap();
    assert_eq!(initialized.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn worker_executes_own_body_and_collects_generators() {
    let app = app(
        ExecutionContext::from_values([
            ("RUNPOD_POD_ID", "pod"),
            ("FLASH_RESOURCE_NAME", "double"),
        ]),
        None,
    );
    let queue = app
        .queue(
            ResourceSpec::default(),
            definition("double"),
            NativeFunction::sync(|call| Ok(json!(call.args[0].as_i64().unwrap() * 10))),
        )
        .unwrap();
    assert_eq!(
        queue
            .remote(vec![json!(4)], Default::default())
            .await
            .unwrap(),
        json!(40)
    );
    let app = app_for_stream();
    let generator = app
        .queue(
            ResourceSpec::default(),
            definition("gen"),
            NativeFunction::streaming(|_| stream::iter([Ok(json!("x")), Ok(json!("y"))])),
        )
        .unwrap();
    assert_eq!(
        generator.remote(vec![], Default::default()).await.unwrap(),
        json!(["x", "y"])
    );
    let values = generator
        .stream(vec![], Default::default())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        values
            .into_iter()
            .collect::<runpod::Result<Vec<_>>>()
            .unwrap(),
        vec![json!("x"), json!("y")]
    );
}
fn app_for_stream() -> App {
    app(
        ExecutionContext::from_values([
            ("RUNPOD_ENDPOINT_ID", "ep"),
            ("RUNPOD_DEV_APP", "demo"),
            ("RUNPOD_DEV_RESOURCE", "gen"),
        ]),
        None,
    )
}

#[tokio::test]
async fn remote_aio_and_options_route_sibling_through_sentinel() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/flash/runsync"))
        .and(header("X-Flash-App", "demo"))
        .and(header("X-Flash-Environment", "production"))
        .and(body_json(
            json!({"input":{"x":2},"policy":{"executionTimeout":1,"ttl":2}}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"status":"COMPLETED","output":4})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let app = app(
        ExecutionContext::from_values([
            ("RUNPOD_ENDPOINT_ID", "ep"),
            ("FLASH_RESOURCE_NAME", "other"),
            ("FLASH_ENVIRONMENT", "production"),
        ]),
        Some(client(&server)),
    );
    let queue = app
        .queue(
            ResourceSpec::default(),
            definition("q"),
            NativeFunction::sync(|_| panic!("sibling must run remotely")),
        )
        .unwrap();
    let bound = queue
        .with_options(&JobOptions {
            execution_timeout: Some(1),
            ..JobOptions::default()
        })
        .unwrap()
        .with_options(&JobOptions {
            ttl: Some(2),
            ..JobOptions::default()
        })
        .unwrap();
    assert_eq!(
        bound
            .remote(vec![json!(2)], Default::default())
            .await
            .unwrap(),
        json!(4)
    );
    assert_eq!(queue.options(), &json!({}));
}

#[tokio::test]
async fn discovery_guard_and_missing_dev_target_prevent_invocation() {
    let context = ExecutionContext::from_values([
        ("RUNPOD_DISCOVERY_SCAN", "1"),
        ("RUNPOD_POD_ID", "pod"),
        ("FLASH_RESOURCE_NAME", "q"),
    ]);
    let app = app(context, None);
    let queue = app
        .queue(
            ResourceSpec::default(),
            definition("q"),
            NativeFunction::sync(|_| panic!("discovery must not invoke")),
        )
        .unwrap();
    assert!(
        queue
            .remote(vec![], Default::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("discovery")
    );
    let app = App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::from_values([("RUNPOD_DEV_SESSION", "1")])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let queue = app
        .queue(ResourceSpec::default(), definition("q"), echo())
        .unwrap();
    assert!(
        queue
            .remote(vec![], Default::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("not deployed")
    );
}

#[tokio::test]
async fn streaming_rejects_plain_functions_and_tasks_and_options_reject_tasks() {
    let app = local();
    let queue = app
        .queue(ResourceSpec::default(), definition("plain"), echo())
        .unwrap();
    assert!(
        queue
            .stream(vec![], Default::default())
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("not a generator")
    );
    let task = app
        .task(ResourceSpec::default(), definition("task"), echo())
        .unwrap();
    assert!(
        task.stream(vec![], Default::default())
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("tasks do not stream")
    );
    assert!(task.with_options(&JobOptions::default()).is_err());
    assert!(task.job("job").await.is_err());
}

#[test]
fn schedule_records_intent_and_manifest_still_rejects_it() {
    let app = local();
    let queue = app
        .queue(ResourceSpec::default(), definition("scheduled"), echo())
        .unwrap();
    assert!(queue.schedule("").is_err());
    queue.schedule("*/5 * * * *").unwrap();
    assert_eq!(
        queue.spec().unwrap().schedule.as_deref(),
        Some("*/5 * * * *")
    );
    assert!(
        runpod::apps::deploy::build_manifest(&app.definition().unwrap(), "3.12", &[])
            .unwrap_err()
            .to_string()
            .contains("schedules")
    );
}

#[tokio::test]
async fn spawn_with_options_returns_queue_job_and_reconnect_does_not_submit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/flash/run"))
        .and(body_json(
            json!({"input":{"x":1},"s3Config":{"bucketName":"bucket"}}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/status/job"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job","status":"COMPLETED","output":null})),
        )
        .expect(2)
        .mount(&server)
        .await;
    let app = app(ExecutionContext::default(), Some(client(&server)));
    let queue = app
        .queue(ResourceSpec::default(), definition("q"), echo())
        .unwrap();
    let mut submitted = queue
        .with_options(&JobOptions {
            s3_config: Some(json!({"bucketName":"bucket"})),
            ..JobOptions::default()
        })
        .unwrap()
        .spawn(vec![json!(1)], Default::default())
        .await
        .unwrap();
    assert_eq!(submitted.queue().unwrap().id(), "job");
    assert!(submitted.task().is_none());
    assert_eq!(submitted.wait(None).await.unwrap(), Value::Null);
    let before = server.received_requests().await.unwrap().len();
    let mut reconnected = queue.job("job").await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), before);
    assert_eq!(
        reconnected
            .result(std::time::Duration::from_secs(1))
            .await
            .unwrap(),
        Value::Null
    );
}

#[tokio::test]
async fn remote_stream_is_lazy_and_preserves_chunk_order() {
    let server = MockServer::start().await;
    Mock::given(path("/flash/run"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/flash/stream/job"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"status":"COMPLETED","stream":[{"output":"x"},{"output":"y"}]}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let app = app(ExecutionContext::default(), Some(client(&server)));
    let generator = app
        .queue(
            ResourceSpec::default(),
            definition("gen"),
            NativeFunction::streaming(|_| stream::empty()),
        )
        .unwrap();
    let mut output = generator.stream(vec![json!(2)], Default::default());
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(output.next().await.unwrap().unwrap(), json!("x"));
    assert_eq!(output.next().await.unwrap().unwrap(), json!("y"));
    assert!(output.next().await.is_none());
}

#[tokio::test]
async fn development_worker_resolves_siblings_by_stable_name_and_caches_target() {
    use runpod::apps::{dev_endpoint_name, serialization::FunctionSource};
    let server = MockServer::start().await;
    let wanted = dev_endpoint_name("demo", "sibling");
    assert_ne!(dev_endpoint_name("a-b", "c"), dev_endpoint_name("a", "b-c"));
    Mock::given(path("/v2/serverless"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"endpoints":[{"id":"unrelated","name":"other"},{"id":"ep123","name":wanted}]}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/ep123/runsync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"status":"COMPLETED","output":{"success":true,"json_result":42}}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    let context = ExecutionContext::from_values([
        ("RUNPOD_ENDPOINT_ID", "ep"),
        ("RUNPOD_DEV_APP", "demo"),
        ("RUNPOD_DEV_RESOURCE", "current"),
    ]);
    let app = app(context, Some(client(&server)));
    let mut function = definition("sibling");
    function.source = Some(FunctionSource::Text("def sibling(x): return x\n".into()));
    let sibling = app
        .queue(
            ResourceSpec::default(),
            function,
            NativeFunction::sync(|_| panic!("must route to sibling")),
        )
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            sibling
                .remote(vec![json!(42)], Default::default())
                .await
                .unwrap(),
            json!(42)
        );
    }
}

#[tokio::test]
async fn dev_session_uses_bound_target_and_does_not_need_other_credentials() {
    use runpod::apps::targets::{ResourceAddress, Target};
    let server = MockServer::start().await;
    Mock::given(path("/flash/runsync"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"status":"COMPLETED","output":7})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let app = app(
        ExecutionContext::from_values([("RUNPOD_DEV_SESSION", "1")]),
        None,
    );
    app.set_dev_target(
        "q",
        Target::sentinel(
            client(&server),
            ResourceAddress::new("demo", "default", "q"),
        )
        .unwrap(),
    )
    .unwrap();
    let queue = app
        .queue(ResourceSpec::default(), definition("q"), echo())
        .unwrap();
    assert_eq!(
        queue.remote(vec![], Default::default()).await.unwrap(),
        json!(7)
    );
}

#[test]
fn current_worker_identity_prefers_dev_app_over_flash_resource() {
    let context = ExecutionContext::from_values([
        ("RUNPOD_DEV_APP", "other"),
        ("RUNPOD_DEV_RESOURCE", "q"),
        ("FLASH_RESOURCE_NAME", "q"),
    ]);
    assert!(!context.is_current_worker("demo", "q"));
    assert!(context.is_current_worker("other", "q"));
    let context =
        ExecutionContext::from_values([("FLASH_RESOURCE_NAME", ""), ("RUNPOD_RESOURCE_NAME", "q")]);
    assert!(context.is_current_worker("demo", "q"));
}

#[tokio::test(flavor = "current_thread")]
async fn sync_native_body_keeps_the_executor_responsive_and_preserves_panic_cause() {
    let app = local();
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = std::sync::Mutex::new(wait);
    let handle = app
        .queue(
            ResourceSpec::default(),
            definition("wait"),
            NativeFunction::sync(move |_| {
                signal.notify_one();
                wait.lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|e| runpod::Error::InvalidInput(e.to_string()))?;
                Ok(json!("released"))
            }),
        )
        .unwrap();
    let running = tokio::spawn(async move { handle.local(vec![], Default::default()).await });
    started.notified().await;
    release.send(()).unwrap();
    assert_eq!(running.await.unwrap().unwrap(), json!("released"));
    let panic_fn = app
        .queue(
            ResourceSpec::default(),
            definition("panic"),
            NativeFunction::sync(|_| panic!("handler panic")),
        )
        .unwrap();
    assert!(
        matches!(panic_fn.local(vec![], Default::default()).await, Err(runpod::Error::BackgroundTask(cause)) if cause.is_panic())
    );
}

#[test]
fn typed_environment_and_symbol_metadata_reach_definition_without_running_hooks() {
    use runpod::apps::{EnvValue, Secret, deploy::SymbolReference};
    let app = local();
    let function = app
        .queue(ResourceSpec::default(), definition("q"), echo())
        .unwrap();
    function
        .set_environment(&[(
            "TOKEN".into(),
            EnvValue::Secret(Secret::new("TOKEN").unwrap()),
        )])
        .unwrap();
    function
        .set_symbol(SymbolReference {
            module: "main".into(),
            qualname: "q".into(),
        })
        .unwrap();
    let definition = app.definition().unwrap();
    assert_eq!(definition.resources[0].referenced_secrets, ["TOKEN"]);
    assert_eq!(
        definition.resources[0].symbol.as_ref().unwrap().qualname,
        "q"
    );
    let snapshot = app.resources().unwrap();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].spec().unwrap().name, "q");
}

#[tokio::test]
async fn task_spawn_retains_pod_job_until_wait_or_cancel() {
    use runpod::apps::{serialization::FunctionSource, tasks::TaskConfig};
    let server = MockServer::start().await;
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"deployCpuPod":{"id":"pod"}}})),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/submit"))
        .respond_with(ResponseTemplate::new(200))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/result"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"status":"DONE","response":{"success":true,"json_result":42}}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/v2/pods/pod"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(2)
        .mount(&server)
        .await;
    let app = App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::default()),
            client: Some(client(&server)),
            task: TaskConfig {
                proxy_base_url: Some(server.uri()),
                ..TaskConfig::default()
            },
            ..AppConfig::default()
        },
    )
    .unwrap();
    let mut function = definition("work");
    function.source = Some(FunctionSource::Text("def work(x): return x\n".into()));
    let task = app
        .task(
            ResourceSpec {
                cpu: Some("cpu5c-2-4".into()),
                ..ResourceSpec::default()
            },
            function,
            echo(),
        )
        .unwrap();
    let mut submitted = task
        .spawn(vec![json!(42)], Default::default())
        .await
        .unwrap();
    assert!(submitted.queue().is_none());
    assert_eq!(submitted.task().unwrap().pod_id().as_deref(), Some("pod"));
    assert_eq!(submitted.wait(None).await.unwrap(), json!(42));
    let mut canceled = task
        .spawn(vec![json!(42)], Default::default())
        .await
        .unwrap();
    canceled.cancel().await.unwrap();
    assert_eq!(canceled.task().unwrap().pod_id(), None);
}

#[test]
fn native_handles_are_send_sync_and_debug_does_not_disclose_job_options() {
    fn check<T: Send + Sync>() {}
    check::<App>();
    check::<runpod::apps::FunctionHandle>();
    check::<NativeFunction>();
    let app = local();
    let function = app
        .queue(ResourceSpec::default(), definition("q"), echo())
        .unwrap()
        .with_options(&JobOptions {
            webhook: Some("secret-webhook".into()),
            ..JobOptions::default()
        })
        .unwrap();
    assert!(!format!("{function:?}").contains("secret-webhook"));
}

use runpod::{
    Client, ClientConfig, Value,
    apps::{
        App, AppConfig, EnvValue, ExecutionContext, FunctionHandle, GlobalVolume, Mount, NativeApi,
        NativeFunction, ResourceKind, ResourceSpec, RouteSpec, Secret, Workers,
        dev::{DevConfig, DevSession},
        events::EventSink,
        serialization::FunctionSource,
        targets::FunctionDefinition,
    },
    json,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_string_contains, header, method, path},
};

#[derive(Default)]
struct BackendState {
    existing: Vec<Value>,
    operations: Vec<Value>,
    fail_save: BTreeSet<String>,
    fail_delete: BTreeSet<String>,
    delayed: Option<String>,
}
struct Backend {
    server: MockServer,
    state: Arc<Mutex<BackendState>>,
    create_started: Arc<tokio::sync::Notify>,
}
impl Backend {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let state = Arc::new(Mutex::new(BackendState::default()));
        let shared = state.clone();
        Mock::given(method("GET"))
            .and(path("/v2/serverless"))
            .respond_with(move |_: &Request| {
                let mut state = shared.lock().unwrap();
                state.operations.push(json!({"list":true}));
                ResponseTemplate::new(200).set_body_json(json!({"endpoints":state.existing}))
            })
            .mount(&server)
            .await;
        let shared = state.clone();
        let create_started = Arc::new(tokio::sync::Notify::new());
        let signal = create_started.clone();
        Mock::given(method("POST")).and(path("/graphql"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                assert!(body["query"].as_str().unwrap().contains("saveEndpoint"));
                let input = &body["variables"]["input"];
                let resource = input["template"]["env"].as_array().unwrap().iter()
                    .find(|e| e["key"]=="RUNPOD_DEV_RESOURCE").unwrap()["value"].as_str().unwrap();
                let mut state = shared.lock().unwrap();
                state.operations.push(json!({"save":input}));
                let response = if state.fail_save.contains(resource) {
                    ResponseTemplate::new(200).set_body_json(json!({"errors":[{"message":"update failed"}]}))
                } else {
                    ResponseTemplate::new(200).set_body_json(json!({"data":{"saveEndpoint":{"id":input.get("id").cloned().unwrap_or(json!(format!("ep-{resource}")))}}}))
                };
                if state.delayed.as_deref()==Some(resource) {
                    signal.notify_one();
                    response.set_delay(Duration::from_millis(150))
                } else { response }
            }).mount(&server).await;
        let shared = state.clone();
        Mock::given(method("DELETE"))
            .respond_with(move |request: &Request| {
                let id = request.url.path().rsplit('/').next().unwrap();
                let mut state = shared.lock().unwrap();
                state.operations.push(json!({"delete":id}));
                if state.fail_delete.contains(id) {
                    ResponseTemplate::new(409).set_body_json(json!({"error":"in use"}))
                } else {
                    ResponseTemplate::new(204)
                }
            })
            .mount(&server)
            .await;
        Self {
            server,
            state,
            create_started,
        }
    }
    fn client(&self) -> Client {
        Client::with_config(ClientConfig {
            api_base_url: self.server.uri(),
            endpoint_base_url: self.server.uri(),
            ..ClientConfig::new("fixture-key")
        })
        .unwrap()
    }
    fn session(&self, apps: Vec<App>, events: Option<EventSink>) -> DevSession {
        let mut config = DevConfig::from_client(&self.client());
        config.images.tag = "latest".into();
        config.image_override = None;
        config.runtime_overrides.clear();
        DevSession::with_config(apps, self.client(), config, events).unwrap()
    }
    fn operations(&self) -> Vec<Value> {
        self.state.lock().unwrap().operations.clone()
    }
    fn saves(&self) -> Vec<Value> {
        self.operations()
            .iter()
            .filter_map(|v| v.get("save").cloned())
            .collect()
    }
    fn deletes(&self) -> Vec<Value> {
        self.operations()
            .iter()
            .filter_map(|v| v.get("delete").cloned())
            .collect()
    }
}
fn app() -> App {
    App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::default()),
            ..AppConfig::default()
        },
    )
    .unwrap()
}
fn register(app: &App, name: &str, mut spec: ResourceSpec) -> FunctionHandle {
    spec.name = name.into();
    spec.cpu = Some("cpu3c-1-2".into());
    let function = FunctionDefinition {
        name: name.into(),
        parameters: vec!["x".into()],
        source: Some(FunctionSource::Text(format!(
            "def {name}(x):\n    return x\n"
        ))),
    };
    let body = NativeFunction::sync(|call| Ok(json!(call.args)));
    if spec.kind == ResourceKind::Task {
        app.task(spec, function, body).unwrap()
    } else {
        app.queue(spec, function, body).unwrap()
    }
}
fn queue(app: &App, name: &str) -> FunctionHandle {
    register(app, name, ResourceSpec::default())
}
type RecordedEvents = Arc<Mutex<Vec<(String, Vec<Value>)>>>;
fn events() -> (EventSink, RecordedEvents) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = events.clone();
    (
        EventSink::new(move |name, args| {
            recorded.lock().unwrap().push((name.into(), args.to_vec()));
            Ok(())
        }),
        events,
    )
}

#[tokio::test]
async fn test_start_provisions_and_registers_targets() {
    let backend = Backend::new().await;
    let app = App::with_config(
        "demo",
        AppConfig {
            client: Some(backend.client()),
            context: Some(ExecutionContext::default()),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let handle = queue(&app, "q");
    Mock::given(path("/ep-q/run"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","status":"IN_QUEUE"})),
        )
        .expect(1)
        .mount(&backend.server)
        .await;
    let mut session = backend.session(vec![app], None);
    session.start().await.unwrap();
    assert_eq!(session.endpoint_ids(), ["ep-q"]);
    assert_eq!(backend.saves().len(), 1);
    // The app starts in local context: the session binds development routing without globals.
    handle
        .spawn(vec![json!(42)], Default::default())
        .await
        .unwrap();
    let requests = backend.server.received_requests().await.unwrap();
    let submitted: Value = requests
        .iter()
        .find(|r| r.url.path() == "/ep-q/run")
        .unwrap()
        .body_json()
        .unwrap();
    assert_eq!(submitted["input"]["args"], json!([42]));
    session.stop().await.unwrap();
    assert!(handle.spawn(vec![], Default::default()).await.is_err());
}

#[tokio::test]
async fn test_start_adopts_and_reconciles_existing_endpoint() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    backend.state.lock().unwrap().existing =
        vec![json!({"name":runpod::apps::dev_endpoint_name("demo","q"),"id":"ep-old"})];
    let (sink, recorded) = events();
    let mut session = backend.session(vec![app], Some(sink));
    session.start().await.unwrap();
    assert_eq!(session.endpoint_ids(), ["ep-old"]);
    assert_eq!(backend.saves()[0]["id"], "ep-old");
    assert!(
        recorded
            .lock()
            .unwrap()
            .contains(&("adopted".into(), vec![json!("q"), json!("ep-old")]))
    );
    session.stop().await.unwrap();
    assert_eq!(backend.deletes(), vec![json!("ep-old")]);
}

#[tokio::test]
async fn test_stop_deletes_all_session_endpoints() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "first");
    queue(&app, "second");
    let mut session = backend.session(vec![app], None);
    session.start().await.unwrap();
    session.stop().await.unwrap();
    assert_eq!(
        backend.deletes(),
        vec![json!("ep-first"), json!("ep-second")]
    );
    assert!(session.endpoint_ids().is_empty());
    session.stop().await.unwrap();
    assert_eq!(backend.deletes().len(), 2);
}

#[tokio::test]
async fn test_tasks_not_provisioned() {
    let backend = Backend::new().await;
    let app = app();
    register(
        &app,
        "task",
        ResourceSpec {
            kind: ResourceKind::Task,
            ..ResourceSpec::default()
        },
    );
    let mut session = backend.session(vec![app], None);
    session.start().await.unwrap();
    assert!(backend.saves().is_empty());
    session.stop().await.unwrap();
}

#[tokio::test]
async fn test_stop_retains_failed_ids_and_retries_only_failures() {
    let backend = Backend::new().await;
    let app = app();
    for name in ["first", "deleted", "second"] {
        queue(&app, name);
    }
    let mut session = backend.session(vec![app], None);
    session.start().await.unwrap();
    backend.state.lock().unwrap().fail_delete = ["ep-first".into(), "ep-second".into()].into();
    let error = session.stop().await.unwrap_err().to_string();
    assert!(error.contains("first (ep-first):") && error.contains("second (ep-second):"));
    assert_eq!(session.endpoint_ids(), ["ep-first", "ep-second"]);
    backend.state.lock().unwrap().fail_delete.clear();
    session.stop().await.unwrap();
    assert!(session.endpoint_ids().is_empty());
    assert_eq!(
        backend.deletes(),
        json!([
            "ep-first",
            "ep-deleted",
            "ep-second",
            "ep-first",
            "ep-second"
        ])
        .as_array()
        .unwrap()
        .clone()
    );
}

#[tokio::test]
async fn test_context_entry_failure_cleans_already_created_endpoints() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "created");
    queue(&app, "fails");
    backend
        .state
        .lock()
        .unwrap()
        .fail_save
        .insert("fails".into());
    let mut session = backend.session(vec![app], None);
    let result: runpod::Result<()> = session
        .run(|_| Box::pin(async { panic!("startup must fail") }))
        .await;
    assert!(result.unwrap_err().to_string().contains("update failed"));
    assert_eq!(backend.deletes(), vec![json!("ep-created")]);
}

#[tokio::test]
async fn test_failed_adoption_remains_tracked_for_cleanup() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    backend.state.lock().unwrap().existing =
        vec![json!({"name":runpod::apps::dev_endpoint_name("demo","q"),"id":"ep-old"})];
    backend.state.lock().unwrap().fail_save.insert("q".into());
    let mut session = backend.session(vec![app], None);
    let result: runpod::Result<()> = session
        .run(|_| Box::pin(async { panic!("adoption must fail") }))
        .await;
    assert!(result.is_err());
    assert_eq!(backend.deletes(), vec![json!("ep-old")]);
}

#[tokio::test]
async fn test_refresh_bumps_generation_and_updates() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    let (sink, recorded) = events();
    let mut session = backend.session(vec![app.clone()], Some(sink));
    session.start().await.unwrap();
    session.refresh(vec![app]).await.unwrap();
    assert_eq!(session.generation(), 2);
    let saves = backend.saves();
    assert_eq!(saves.len(), 2);
    assert_eq!(saves[1]["id"], "ep-q");
    assert!(
        saves[1]["template"]["env"]
            .as_array()
            .unwrap()
            .contains(&json!({"key":"RUNPOD_DEV_GENERATION","value":"2"}))
    );
    let events = recorded.lock().unwrap().clone();
    assert!(
        !events
            .iter()
            .any(|(e, _)| e == "resource_added" || e == "resource_changed")
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn test_refresh_diff_events() {
    let backend = Backend::new().await;
    let original = app();
    queue(&original, "stays");
    queue(&original, "goes");
    let (sink, recorded) = events();
    let mut session = backend.session(vec![original], Some(sink));
    session.start().await.unwrap();
    let replacement = app();
    register(
        &replacement,
        "stays",
        ResourceSpec {
            workers: Some(Workers::Range([1, 4])),
            ..ResourceSpec::default()
        },
    );
    queue(&replacement, "fresh");
    session.refresh(vec![replacement]).await.unwrap();
    assert_eq!(backend.deletes(), vec![json!("ep-goes")]);
    let ops = backend.operations();
    assert!(
        ops.iter().position(|v| v.get("delete").is_some()).unwrap()
            < ops.iter().rposition(|v| v.get("save").is_some()).unwrap()
    );
    let recorded = recorded.lock().unwrap().clone();
    assert!(recorded.contains(&("resource_removed".into(), vec![json!("goes")])));
    assert!(recorded.contains(&(
        "resource_added".into(),
        vec![json!("fresh"), json!("queue"), json!("cpu3c-1-2")]
    )));
    assert!(recorded.contains(&(
        "resource_changed".into(),
        vec![json!("stays"), json!(["workersMax", "workersMin"])]
    )));
    session.stop().await.unwrap();
}

#[tokio::test]
async fn test_failed_removals_remain_tracked_for_stop() {
    let backend = Backend::new().await;
    let original = app();
    for name in ["basic", "other", "survivor"] {
        queue(&original, name);
    }
    let mut session = backend.session(vec![original], None);
    session.start().await.unwrap();
    backend
        .state
        .lock()
        .unwrap()
        .fail_delete
        .insert("ep-basic".into());
    let replacement = app();
    queue(&replacement, "survivor");
    queue(&replacement, "new");
    assert!(
        session
            .refresh(vec![replacement])
            .await
            .unwrap_err()
            .to_string()
            .contains("ep-basic")
    );
    assert_eq!(session.endpoint_ids(), ["ep-basic", "ep-survivor"]);
    assert_eq!(
        backend.saves().len(),
        3,
        "removal errors prevent updates and additions"
    );
    backend.state.lock().unwrap().fail_delete.clear();
    session.stop().await.unwrap();
    assert_eq!(
        backend.deletes(),
        json!(["ep-basic", "ep-other", "ep-basic", "ep-survivor"])
            .as_array()
            .unwrap()
            .clone()
    );
}

#[tokio::test]
async fn test_lifecycle_events_emitted() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    let (sink, recorded) = events();
    let (cleanup, cleanup_events) = events();
    let mut session = backend.session(vec![app], Some(sink));
    session.start().await.unwrap();
    session.stop_with_events(Some(&cleanup)).await.unwrap();
    assert_eq!(
        *recorded.lock().unwrap(),
        vec![
            ("session_starting".into(), vec![]),
            (
                "provisioning".into(),
                vec![json!("q"), json!("queue"), json!("cpu3c-1-2")]
            ),
            ("ready".into(), vec![json!("q"), json!("ep-q")]),
            ("session_started".into(), vec![])
        ]
    );
    assert_eq!(
        *cleanup_events.lock().unwrap(),
        vec![
            ("cleanup_started".into(), vec![json!(1)]),
            ("deleting".into(), vec![json!("q")]),
            ("deleted".into(), vec![json!("q")])
        ]
    );
}

#[tokio::test]
async fn test_missing_event_methods_ignored() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    let mut session = backend.session(
        vec![app],
        Some(EventSink::new(|_, _| {
            Err(runpod::Error::InvalidInput("observer failed".into()))
        })),
    );
    assert_eq!(
        session
            .run(|s| Box::pin(async move { Ok(s.endpoint_ids().len()) }))
            .await
            .unwrap(),
        1
    );
    assert!(session.endpoint_ids().is_empty());
}

#[tokio::test]
async fn canceled_create_is_settled_before_cleanup_even_after_cleanup_is_canceled() {
    for failure in [false, true] {
        let backend = Backend::new().await;
        let app = app();
        queue(&app, "q");
        backend.state.lock().unwrap().delayed = Some("q".into());
        if failure {
            backend.state.lock().unwrap().fail_save.insert("q".into());
        }
        let mut session = backend.session(vec![app], None);
        tokio::select! {
            result = session.start() => panic!("create should still be pending: {result:?}"),
            () = backend.create_started.notified() => {}
        }
        assert!(session.endpoint_ids().is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(10), session.stop())
                .await
                .is_err()
        );
        session.stop().await.unwrap();
        assert!(session.endpoint_ids().is_empty());
        assert_eq!(
            backend.saves().len(),
            1,
            "canceled submission must never be repeated"
        );
        assert_eq!(
            backend.deletes(),
            if failure { vec![] } else { vec![json!("ep-q")] }
        );
    }
}

#[tokio::test]
async fn secrets_preflight_includes_tasks_and_blocks_all_provisioning() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    let task = register(
        &app,
        "task",
        ResourceSpec {
            kind: ResourceKind::Task,
            ..ResourceSpec::default()
        },
    );
    task.set_environment(&[(
        "TOKEN".into(),
        EnvValue::Secret(Secret::new("missing").unwrap()),
    )])
    .unwrap();
    Mock::given(path("/graphql"))
        .and(body_string_contains("secrets"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"myself":{"secrets":[]}}})),
        )
        .with_priority(1)
        .expect(1)
        .mount(&backend.server)
        .await;
    let mut session = backend.session(vec![app], None);
    assert!(
        session
            .start()
            .await
            .unwrap_err()
            .to_string()
            .contains("secret not found: missing")
    );
    assert!(backend.operations().is_empty());
    session.stop().await.unwrap();
}

#[tokio::test]
async fn api_model_registry_and_volume_resolution_survive_refresh() {
    let backend = Backend::new().await;
    for (url, body) in [
        (
            "/v2/registries",
            json!({"registries":[{"id":"reg","name":"private"}]}),
        ),
        (
            "/v2/global-volumes",
            json!({"globalVolumes":[{"id":"vol","name":"models"}],"pagination":{"hasNextPage":false}}),
        ),
    ] {
        Mock::given(path(url))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(if url.ends_with("global-volumes") {
                1
            } else {
                2
            })
            .mount(&backend.server)
            .await;
    }
    let app = app();
    let mut body = NativeApi::new("api");
    body.route(
        RouteSpec {
            method: "GET".into(),
            path: "/x".into(),
            handler_name: "read".into(),
        },
        NativeFunction::sync(|_| Ok(json!(42))),
    )
    .unwrap();
    app.api(
        ResourceSpec {
            gpu: Some("ADA_24".into()),
            registry_auth: Some("private".into()),
            model: Some("org/model".into()),
            mounts: vec![Mount {
                path: "/runpod-volume".into(),
                volume: GlobalVolume::new("models").unwrap().into(),
            }],
            ..ResourceSpec::default()
        },
        body,
    )
    .unwrap();
    let mut session = backend.session(vec![app.clone()], None);
    session.start().await.unwrap();
    session.refresh(vec![app]).await.unwrap();
    let saved = backend.saves();
    for payload in saved {
        assert_eq!(payload["type"], "LB");
        assert_eq!(payload["template"]["containerRegistryAuthId"], "reg");
        assert_eq!(
            payload["volumes"],
            json!([{"volumeId":"vol","volumeType":"OBJECT_STORE_VOLUME"}])
        );
        assert!(
            payload["modelReferences"]
                .as_array()
                .is_some_and(|v| v.len() == 1)
        );
    }
    session.stop().await.unwrap();
}

#[tokio::test]
async fn body_error_still_cleans_up_and_cleanup_error_takes_precedence() {
    for fail_cleanup in [false, true] {
        let backend = Backend::new().await;
        let app = app();
        queue(&app, "q");
        if fail_cleanup {
            backend
                .state
                .lock()
                .unwrap()
                .fail_delete
                .insert("ep-q".into());
        }
        let mut session = backend.session(vec![app], None);
        let result: runpod::Result<()> = session
            .run(|_| Box::pin(async { Err(runpod::Error::InvalidInput("body failed".into())) }))
            .await;
        let error = result.unwrap_err().to_string();
        assert!(error.contains(if fail_cleanup {
            "dev cleanup incomplete"
        } else {
            "body failed"
        }));
        assert_eq!(backend.deletes(), vec![json!("ep-q")]);
        backend.state.lock().unwrap().fail_delete.clear();
        session.stop().await.unwrap();
    }
}

#[tokio::test]
async fn test_dev_events_reattached_after_refresh() {
    let backend = Backend::new().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("deployCpuPod"))
        .and(header("authorization", "Bearer fixture-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"errors":[{"message":"rejected by test"}]})),
        )
        .with_priority(1)
        .expect(2)
        .mount(&backend.server)
        .await;
    let (sink, recorded) = events();
    let original = app();
    let task = register(
        &original,
        "task",
        ResourceSpec {
            kind: ResourceKind::Task,
            ..ResourceSpec::default()
        },
    );
    let mut session = backend.session(vec![original], Some(sink));
    session.start().await.unwrap();
    assert!(
        task.remote(vec![json!(42)], Default::default())
            .await
            .is_err()
    );
    let replacement = app();
    let task = register(
        &replacement,
        "task",
        ResourceSpec {
            kind: ResourceKind::Task,
            ..ResourceSpec::default()
        },
    );
    session.refresh(vec![replacement]).await.unwrap();
    assert!(
        task.remote(vec![json!(43)], Default::default())
            .await
            .is_err()
    );
    assert_eq!(
        recorded
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| e == "dispatch")
            .count(),
        2
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn test_worker_self_call_stays_local() {
    let backend = Backend::new().await;
    let app = App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::from_values([
                ("RUNPOD_ENDPOINT_ID", "ep-self"),
                ("RUNPOD_DEV_APP", "demo"),
                ("RUNPOD_DEV_RESOURCE", "q"),
            ])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let handle = queue(&app, "q");
    let mut session = backend.session(vec![app], None);
    session.start().await.unwrap();
    assert_eq!(
        handle
            .remote(vec![json!(4)], Default::default())
            .await
            .unwrap(),
        json!([4])
    );
    session.stop().await.unwrap();
}

#[tokio::test]
async fn shutdown_signal_waits_for_pending_creation_and_cleanup_before_returning() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    backend.state.lock().unwrap().delayed = Some("q".into());
    let mut session = backend.session(vec![app], None);
    let result: Option<()> = session
        .run_until(
            |_| Box::pin(async { panic!("shutdown interrupts startup") }),
            backend.create_started.notified(),
        )
        .await
        .unwrap();
    assert_eq!(result, None);
    assert!(session.endpoint_ids().is_empty());
    assert_eq!(backend.deletes(), vec![json!("ep-q")]);
}

#[tokio::test]
async fn shutdown_during_body_stops_all_endpoints() {
    let backend = Backend::new().await;
    let app = app();
    queue(&app, "q");
    let stop = Arc::new(tokio::sync::Notify::new());
    let signal = stop.clone();
    let mut session = backend.session(vec![app], None);
    let result: Option<()> = session
        .run_until(
            move |_| {
                Box::pin(async move {
                    signal.notify_one();
                    std::future::pending().await
                })
            },
            stop.notified(),
        )
        .await
        .unwrap();
    assert_eq!(result, None);
    assert!(session.endpoint_ids().is_empty());
    assert_eq!(backend.deletes(), vec![json!("ep-q")]);
}

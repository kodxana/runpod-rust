//! Explicit route registration replaces Python class decorators and inspection.
use runpod::{
    Client, ClientConfig, Value,
    apps::{
        App, AppConfig, ExecutionContext, NativeApi, NativeFunction, ResourceSpec, RouteSpec,
        serialization::FunctionSource, targets::Target,
    },
    json,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn app(client: Option<Client>) -> App {
    App::with_config(
        "demo",
        AppConfig {
            client,
            context: Some(ExecutionContext::default()),
            ..AppConfig::default()
        },
    )
    .unwrap()
}
fn route(method: &str, path: &str, handler: &str) -> RouteSpec {
    RouteSpec {
        method: method.into(),
        path: path.into(),
        handler_name: handler.into(),
    }
}
fn body() -> NativeFunction {
    NativeFunction::new(|_| async { Ok(Value::Null) })
}
fn client(server: &MockServer) -> Client {
    Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        endpoint_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap()
}

#[test]
fn api_collects_sorted_routes_and_rejects_duplicates_and_empty_services() {
    let app = app(None);
    let mut service = NativeApi::new("Inference");
    service
        .route(route("GET", "/health", "health"), body())
        .unwrap();
    service
        .route(route("POST", "/generate", "generate"), body())
        .unwrap();
    service.init("setup", body()).unwrap();
    assert!(
        service
            .init("another", body())
            .unwrap_err()
            .to_string()
            .contains("multiple")
    );
    assert!(
        service
            .route(route("POST", "/generate", "duplicate"), body())
            .unwrap_err()
            .to_string()
            .contains("duplicate route")
    );
    let api = app.api(ResourceSpec::default(), service).unwrap();
    let spec = api.spec().unwrap();
    assert_eq!(spec.name, "Inference");
    assert_eq!(spec.kind, runpod::apps::ResourceKind::Api);
    assert_eq!(
        spec.routes
            .iter()
            .map(|r| r.handler_name.as_str())
            .collect::<Vec<_>>(),
        ["generate", "health"]
    );
    assert!(
        app.api(ResourceSpec::default(), NativeApi::new("Empty"))
            .unwrap_err()
            .to_string()
            .contains("no routes")
    );
    assert_eq!(app.definition().unwrap().resources.len(), 1);
}

#[test]
fn api_reserved_control_routes_are_rejected_for_every_verb() {
    let mut service = NativeApi::new("api");
    for method in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
        for path in ["/ping", "/execute", "/_runpod/sync"] {
            assert!(service.route(route(method, path, "f"), body()).is_err());
        }
    }
}

#[test]
fn queue_task_and_api_share_one_resource_namespace_and_registration_order() {
    let app = app(None);
    let mut service = NativeApi::new("service");
    service.route(route("GET", "/x", "read"), body()).unwrap();
    app.api(ResourceSpec::default(), service).unwrap();
    let function = runpod::apps::targets::FunctionDefinition {
        name: "second".into(),
        parameters: vec![],
        source: None,
    };
    app.task(ResourceSpec::default(), function.clone(), body())
        .unwrap();
    assert!(
        app.queue(
            ResourceSpec {
                name: "service".into(),
                ..ResourceSpec::default()
            },
            function,
            body()
        )
        .is_err()
    );
    assert_eq!(
        app.resources()
            .unwrap()
            .iter()
            .map(|r| r.spec().unwrap().name)
            .collect::<Vec<_>>(),
        ["service", "second"]
    );
}

#[tokio::test]
async fn api_initialization_is_explicit_and_native_methods_keep_owned_state() {
    let app = app(None);
    let state = Arc::new(AtomicUsize::new(0));
    let mut service = NativeApi::new("api");
    let initialized = state.clone();
    service
        .init(
            "setup",
            NativeFunction::sync(move |_| {
                initialized.store(42, Ordering::SeqCst);
                Ok(Value::Null)
            }),
        )
        .unwrap();
    let state_for_call = state.clone();
    service
        .route(
            route("GET", "/state", "state"),
            NativeFunction::new(move |_| {
                let value = state_for_call.load(Ordering::SeqCst);
                async move { Ok(json!(value)) }
            }),
        )
        .unwrap();
    let api = app.api(ResourceSpec::default(), service).unwrap();
    assert_eq!(state.load(Ordering::SeqCst), 0);
    api.initialize().await.unwrap();
    assert_eq!(
        api.call_handler("state", Default::default()).await.unwrap(),
        json!(42)
    );
}

#[tokio::test]
async fn api_http_verbs_use_bound_target_and_share_live_source_sync() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/_runpod/sync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    for verb in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
        Mock::given(method(verb))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"method":verb})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let app = App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::from_values([("RUNPOD_DEV_SESSION", "1")])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let mut target = Target::live(client(&server), "ep123", "api").unwrap();
    target.set_load_balancer_url(server.uri()).unwrap();
    app.set_dev_target("api", target).unwrap();
    let mut service = NativeApi::new("api");
    service.route(route("GET", "/x", "read"), body()).unwrap();
    service.set_source(FunctionSource::Text("# complete service source\n".into()));
    let api = app.api(ResourceSpec::default(), service).unwrap();
    assert_eq!(api.get("/x", None).await.unwrap()["method"], "GET");
    assert_eq!(
        api.post("/x", Some(&json!({"value":1}))).await.unwrap()["method"],
        "POST"
    );
    assert_eq!(api.put("/x", None).await.unwrap()["method"], "PUT");
    assert_eq!(api.delete("/x", None).await.unwrap()["method"], "DELETE");
    assert_eq!(api.patch("/x", None).await.unwrap()["method"], "PATCH");
}

#[tokio::test]
async fn api_discovery_guard_prevents_http_and_asgi_factory_metadata_is_explicit() {
    let server = MockServer::start().await;
    let app = App::with_config(
        "demo",
        AppConfig {
            client: Some(client(&server)),
            context: Some(ExecutionContext::from_values([(
                "RUNPOD_DISCOVERY_SCAN",
                "1",
            )])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let factory = NativeApi::asgi_factory("web", "Container.web");
    let api = app.api(ResourceSpec::default(), factory).unwrap();
    assert_eq!(
        api.spec().unwrap().asgi_factory.as_deref(),
        Some("Container.web")
    );
    assert!(
        api.get("/x", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("discovery")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn own_worker_api_still_uses_http_and_does_not_run_native_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(42)))
        .expect(1)
        .mount(&server)
        .await;
    let app = App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::from_values([
                ("RUNPOD_ENDPOINT_ID", "ep123"),
                ("RUNPOD_DEV_APP", "demo"),
                ("RUNPOD_DEV_RESOURCE", "api"),
            ])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let mut target = Target::live(client(&server), "ep123", "api").unwrap();
    target.set_load_balancer_url(server.uri()).unwrap();
    app.set_dev_target("api", target).unwrap();
    let mut service = NativeApi::new("api");
    service
        .route(
            route("GET", "/x", "read"),
            NativeFunction::sync(|_| panic!("HTTP methods never run native handlers locally")),
        )
        .unwrap();
    let api = app.api(ResourceSpec::default(), service).unwrap();
    assert_eq!(api.get("/x", None).await.unwrap(), json!(42));
}

#[tokio::test]
async fn failed_source_sync_prevents_route_call_and_next_call_can_retry() {
    let server = MockServer::start().await;
    Mock::given(path("/_runpod/sync"))
        .respond_with(ResponseTemplate::new(400).set_body_string("invalid source"))
        .up_to_n_times(1)
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/_runpod/sync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(42)))
        .expect(1)
        .mount(&server)
        .await;
    let app = App::with_config(
        "demo",
        AppConfig {
            context: Some(ExecutionContext::from_values([("RUNPOD_DEV_SESSION", "1")])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let mut target = Target::live(client(&server), "ep123", "api").unwrap();
    target.set_load_balancer_url(server.uri()).unwrap();
    app.set_dev_target("api", target).unwrap();
    let mut service = NativeApi::new("api");
    service.route(route("GET", "/x", "read"), body()).unwrap();
    service.set_source(FunctionSource::Text("# service\n".into()));
    let api = app.api(ResourceSpec::default(), service).unwrap();
    assert!(api.get("/x", None).await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(api.get("/x", None).await.unwrap(), json!(42));
}

#[test]
fn api_metadata_reaches_manifest_and_resource_snapshot_preserves_handle_kinds() {
    use runpod::apps::{
        EnvValue, ResourceHandle, Secret,
        deploy::{SymbolReference, build_manifest},
    };
    let app = app(None);
    let mut service = NativeApi::new("api");
    service
        .route(route("POST", "/old", "write"), body())
        .unwrap();
    service
        .route(route("POST", "/new", "write"), body())
        .unwrap();
    service.set_symbol(SymbolReference {
        module: "main".into(),
        qualname: "Inference".into(),
    });
    let api = app.api(ResourceSpec::default(), service).unwrap();
    api.set_environment(&[(
        "TOKEN".into(),
        EnvValue::Secret(Secret::new("TOKEN").unwrap()),
    )])
    .unwrap();
    let definition = app.definition().unwrap();
    assert_eq!(definition.resources[0].referenced_secrets, ["TOKEN"]);
    let manifest = build_manifest(&definition, "3.12", &[]).unwrap();
    assert_eq!(
        manifest["resources"][0]["routes"],
        json!([{"method":"POST","path":"/new","handler":"write"}])
    );
    assert!(matches!(
        &app.resources().unwrap()[0],
        ResourceHandle::Api(_)
    ));
    fn send_sync<T: Send + Sync>() {}
    send_sync::<runpod::apps::ApiHandle>();
    send_sync::<NativeApi>();
}

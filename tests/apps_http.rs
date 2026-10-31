use runpod::{
    Error, Value,
    apps::{App, NativeApi, NativeFunction, ResourceSpec, RouteSpec},
    json,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::{net::TcpListener, sync::oneshot};

fn route(method: &str, path: &str, name: &str) -> RouteSpec {
    RouteSpec {
        method: method.into(),
        path: path.into(),
        handler_name: name.into(),
    }
}

#[tokio::test]
async fn init_blocks_health_and_handlers_then_shutdown_drains_requests() {
    let app = App::new("lifecycle").unwrap();
    let mut service = NativeApi::new("api");
    let entering = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let ready = Arc::new(AtomicBool::new(false));
    let started = entering.clone();
    let proceed = release.clone();
    let state = ready.clone();
    service
        .init(
            "setup",
            NativeFunction::new(move |_| {
                let started = started.clone();
                let proceed = proceed.clone();
                let state = state.clone();
                async move {
                    started.notify_one();
                    proceed.notified().await;
                    state.store(true, Ordering::SeqCst);
                    Ok(Value::Null)
                }
            }),
        )
        .unwrap();
    let handling = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let called = handling.clone();
    let end = finish.clone();
    service
        .route(
            route("POST", "/wait", "wait"),
            NativeFunction::new(move |call| {
                let called = called.clone();
                let end = end.clone();
                let ready = ready.clone();
                async move {
                    assert!(ready.load(Ordering::SeqCst));
                    called.notify_one();
                    end.notified().await;
                    Ok(json!(call.args))
                }
            }),
        )
        .unwrap();
    let api = app.api(ResourceSpec::default(), service).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stopped.await;
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), entering.notified())
        .await
        .unwrap();
    let http = reqwest::Client::new();
    let ping = http.get(format!("{address}/ping")).send();
    tokio::pin!(ping);
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut ping)
            .await
            .is_err()
    );
    release.notify_one();
    assert_eq!(
        ping.await.unwrap().json::<Value>().await.unwrap(),
        json!({"status":"healthy"})
    );
    let request = tokio::spawn(
        http.post(format!("{address}/wait"))
            .json(&json!({"a":1}))
            .send(),
    );
    tokio::time::timeout(Duration::from_secs(3), handling.notified())
        .await
        .unwrap();
    stop.send(()).unwrap();
    finish.notify_one();
    assert_eq!(
        request
            .await
            .unwrap()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        json!([{"a":1}])
    );
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn startup_error_or_cancellation_never_accepts_requests() {
    for fail in [false, true] {
        let mut service = NativeApi::new("api");
        let entered = Arc::new(Notify::new());
        let signal = entered.clone();
        service
            .init(
                "setup",
                NativeFunction::new(move |_| {
                    let signal = signal.clone();
                    async move {
                        signal.notify_one();
                        if fail {
                            Err(Error::InvalidInput("init failed".into()))
                        } else {
                            std::future::pending().await
                        }
                    }
                }),
            )
            .unwrap();
        service
            .route(
                route("GET", "/run", "run"),
                NativeFunction::sync(|_| panic!("not ready")),
            )
            .unwrap();
        let api = App::new("startup")
            .unwrap()
            .api(ResourceSpec::default(), service)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            api.serve(listener, async {
                let _ = stopped.await;
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        if !fail {
            stop.send(()).unwrap();
        }
        let result = server.await.unwrap();
        if fail {
            assert!(
                matches!(result, Err(Error::InvalidInput(ref message)) if message == "init failed")
            );
        } else {
            result.unwrap();
        }
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }
}

#[tokio::test]
async fn invalid_path_patterns_and_python_factories_fail_before_init() {
    for path in ["/x/{id:unknown}", "/x/{id}/{id:int}"] {
        let mut service = NativeApi::new("api");
        service
            .init(
                "setup",
                NativeFunction::sync(|_| panic!("invalid routes must fail before init")),
            )
            .unwrap();
        service
            .route(
                route("GET", path, "run"),
                NativeFunction::sync(|_| unreachable!()),
            )
            .unwrap();
        let api = App::new("invalid")
            .unwrap()
            .api(ResourceSpec::default(), service)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = api
            .serve(listener, std::future::pending())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::InvalidInput(_)));
    }
    let api = App::new("factory")
        .unwrap()
        .api(
            ResourceSpec::default(),
            NativeApi::asgi_factory("web", "web"),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    assert!(
        api.serve(listener, std::future::pending())
            .await
            .unwrap_err()
            .to_string()
            .contains("ASGI factories execute in a Python worker")
    );
}

#[tokio::test]
async fn native_client_calls_the_hosted_server_and_handles_large_and_streamed_results() {
    use runpod::{
        Client, ClientConfig,
        apps::{AppConfig, ExecutionContext, targets::Target},
    };
    let app = App::with_config(
        "native",
        AppConfig {
            context: Some(ExecutionContext::from_values([("RUNPOD_DEV_SESSION", "1")])),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let mut service = NativeApi::new("api");
    service
        .route(
            route("POST", "/echo", "echo"),
            NativeFunction::new(|call| async move {
                Ok(call.args.into_iter().next().unwrap_or(Value::Null))
            }),
        )
        .unwrap();
    service
        .route(
            route("GET", "/stream", "stream"),
            NativeFunction::streaming(|_| {
                futures_util::stream::iter([Ok(json!(1)), Ok(json!({"value":null}))])
            }),
        )
        .unwrap();
    let api = app.api(ResourceSpec::default(), service).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    // A deployed target with an explicit load-balancer URL exercises ApiHandle's real HTTP
    // client without invoking live Python source synchronization or a control plane.
    let client = Client::with_config(ClientConfig {
        api_base_url: address.clone(),
        endpoint_base_url: address.clone(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    let mut target = Target::sentinel(
        client,
        runpod::apps::targets::ResourceAddress::new("native", "main", "api"),
    )
    .unwrap();
    target.set_load_balancer_url(&address).unwrap();
    app.set_dev_target("api", target).unwrap();
    let hosted = api.clone();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        hosted
            .serve(listener, async {
                let _ = stopped.await;
            })
            .await
    });
    let large = json!({"payload":"x".repeat(2 * 1024 * 1024 + 1)});
    assert_eq!(api.post("/echo", Some(&large)).await.unwrap(), large);
    assert_eq!(
        api.get("/stream", None).await.unwrap(),
        json!([1,{"value":null}])
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn documentation_and_custom_routes_keep_framework_precedence() {
    let app = App::new("docs").unwrap();
    let mut service = NativeApi::new("api");
    service
        .route(
            route("GET", "/docs", "a_shadowed"),
            NativeFunction::sync(|_| panic!("docs have precedence")),
        )
        .unwrap();
    service
        .route(
            route("POST", "/docs", "b_post"),
            NativeFunction::sync(|call| Ok(json!(call.args))),
        )
        .unwrap();
    let api = app
        .api(
            ResourceSpec {
                name: "Title <&>".into(),
                ..ResourceSpec::default()
            },
            service,
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        api.serve(listener, async {
            let _ = stopped.await;
        })
        .await
    });
    let http = reqwest::Client::new();
    for path in ["/docs", "/redoc"] {
        let response = http.get(format!("{address}{path}")).send().await.unwrap();
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains("text/html")
        );
        let html = response.text().await.unwrap();
        assert!(html.contains("Title &lt;&amp;&gt;"));
        assert!(html.contains("/openapi.json"));
    }
    assert_eq!(
        http.post(format!("{address}/docs"))
            .json(&json!({"x":1}))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        json!([{"x":1}])
    );
    assert_eq!(
        http.head(format!("{address}/openapi.json"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        ""
    );
    for path in ["/execute", "/_runpod/sync"] {
        assert_eq!(
            http.post(format!("{address}{path}"))
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

fn service(routes: &Value, calls: &Arc<Mutex<Vec<Value>>>) -> runpod::apps::ApiHandle {
    let app = App::new("fixture").unwrap();
    let mut api = NativeApi::new("DeclaredName");
    let ready = Arc::new(AtomicBool::new(false));
    let initialized = ready.clone();
    api.init(
        "setup",
        NativeFunction::sync(move |_| {
            initialized.store(true, Ordering::SeqCst);
            Ok(Value::Null)
        }),
    )
    .unwrap();
    for route in routes.as_array().unwrap() {
        let spec = RouteSpec {
            method: route["method"].as_str().unwrap().into(),
            path: route["path"].as_str().unwrap().into(),
            handler_name: route["handler"].as_str().unwrap().into(),
        };
        let declaration = route.clone();
        let calls = calls.clone();
        let ready = ready.clone();
        api.route(
            spec,
            NativeFunction::sync(move |args| {
                assert!(
                    args.kwargs.is_empty(),
                    "the source wrapper forwards no keyword arguments"
                );
                let ready = ready.load(Ordering::SeqCst);
                calls
                    .lock()
                    .unwrap()
                    .push(json!({"handler":declaration["handler"],"args":args.args,"ready":ready}));
                if declaration["error"] == true {
                    return Err(Error::InvalidInput("handler failed".into()));
                }
                Ok(declaration
                    .get("output")
                    .cloned()
                    .unwrap_or_else(|| json!({"args":args.args,"ready":ready})))
            }),
        )
        .unwrap();
    }
    app.api(
        ResourceSpec {
            name: "service".into(),
            ..ResourceSpec::default()
        },
        api,
    )
    .unwrap()
}

#[tokio::test]
async fn runtime_http_fixtures() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/apps_http.json")).unwrap();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let api = service(&case["routes"], &calls);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            api.serve(listener, async {
                let _ = stopped.await;
            })
            .await
        });
        for (request, expected) in case["requests"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["responses"].as_array().unwrap())
        {
            let path = request["path"].as_str().unwrap();
            let mut sent = http.request(
                request["method"].as_str().unwrap().parse().unwrap(),
                format!("{address}{path}"),
            );
            if let Some(body) = request.get("json") {
                sent = sent.json(body);
            }
            if let Some(raw) = request.get("raw") {
                sent = sent.body(raw.as_str().unwrap().to_owned()).header(
                    "content-type",
                    request["content_type"]
                        .as_str()
                        .unwrap_or("application/octet-stream"),
                );
            }
            let response = sent.send().await.unwrap();
            assert_eq!(
                u64::from(response.status().as_u16()),
                expected["status"].as_u64().unwrap(),
                "{name}: {request}"
            );
            assert_eq!(
                response.headers().get("allow").map(|h| h.to_str().unwrap()),
                expected["allow"].as_str(),
                "{name}: {request}"
            );
            let location = response
                .headers()
                .get("location")
                .map(|h| h.to_str().unwrap().replace(&address, "http://testserver"));
            assert_eq!(
                location.as_deref(),
                expected["location"].as_str(),
                "{name}: {request}"
            );
            let text = response.text().await.unwrap();
            let body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
            assert_eq!(body, expected["body"], "{name}: {request}");
        }
        if name == "paths and route order" {
            let schema: Value = http
                .get(format!("{address}/openapi.json"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let expected: Value =
                serde_json::from_str(include_str!("fixtures/apps_http_openapi.json")).unwrap();
            assert_eq!(schema, expected);
        }
        assert_eq!(
            json!(*calls.lock().unwrap()),
            case["calls"],
            "{name}: calls"
        );
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }
}

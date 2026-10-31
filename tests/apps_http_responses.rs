use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::{StreamExt, stream};
use runpod::{
    Error, Value,
    apps::{
        App, FactoryHealth, FunctionArgs, NativeApi, NativeHttpApp, NativeHttpFunction,
        ResourceSpec, RouteSpec,
    },
    json,
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Notify, oneshot},
};

fn unhex(text: &str) -> Vec<u8> {
    let (pairs, remainder) = text.as_bytes().as_chunks::<2>();
    assert_eq!(remainder, b"");
    pairs
        .iter()
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
        .collect()
}

fn response_for(case: &Value) -> Response {
    let mut builder =
        Response::builder()
            .status(
                u16::try_from(case["status"].as_u64().unwrap_or(
                    if case.get("location").is_some() {
                        307
                    } else {
                        200
                    },
                ))
                .unwrap(),
            )
            .header("x-test", "passthrough");
    let body = if let Some(chunks) = case["chunks"].as_array() {
        let chunks: Vec<_> = chunks
            .iter()
            .map(|part| Ok::<_, std::io::Error>(unhex(part.as_str().unwrap())))
            .collect();
        Body::from_stream(stream::iter(chunks))
    } else if let Some(location) = case["location"].as_str() {
        builder = builder.header(header::LOCATION, location);
        Body::empty()
    } else if let Some(value) = case.get("json") {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(value).unwrap())
    } else if let Some(hex) = case["hex"].as_str() {
        Body::from(unhex(hex))
    } else {
        Body::from(case["text"].as_str().unwrap().to_owned())
    };
    if let Some(media) = case["media_type"].as_str() {
        builder = builder.header(
            header::CONTENT_TYPE,
            if media.starts_with("text/") {
                format!("{media}; charset=utf-8")
            } else {
                media.to_owned()
            },
        );
    }
    let mut response = builder.body(body).unwrap();
    for cookie in case["cookies"].as_array().into_iter().flatten() {
        response.headers_mut().append(
            header::SET_COOKIE,
            cookie.as_str().unwrap().parse().unwrap(),
        );
    }
    response
}

async fn assert_response(response: reqwest::Response, expected: &Value) {
    assert_eq!(
        u64::from(response.status().as_u16()),
        expected["status"].as_u64().unwrap()
    );
    for name in [
        "content-type",
        "content-length",
        "location",
        "set-cookie",
        "x-test",
        "allow",
    ] {
        let values: Vec<_> = response
            .headers()
            .get_all(name)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(
            json!(values),
            expected["headers"].get(name).cloned().unwrap_or(json!([])),
            "header {name}"
        );
    }
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        unhex(expected["hex"].as_str().unwrap())
    );
}

async fn host(
    service: NativeApi,
) -> (
    String,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<runpod::Result<()>>,
) {
    let api = App::new("http-responses")
        .unwrap()
        .api(ResourceSpec::default(), service)
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
    (address, stop, server)
}

#[tokio::test]
async fn response_passthrough_matches_python_runtime() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/apps_http_responses.json")).unwrap();
    let mut service = NativeApi::new("responses");
    for case in fixture["responses"].as_array().unwrap() {
        let case = case.clone();
        service
            .http_route(
                RouteSpec {
                    method: "GET".into(),
                    path: format!("/{}", case["name"].as_str().unwrap()),
                    handler_name: case["name"].as_str().unwrap().into(),
                },
                NativeHttpFunction::sync(move |_| Ok(response_for(&case))),
            )
            .unwrap();
    }
    let (address, stop, server) = host(service).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for case in fixture["responses"].as_array().unwrap() {
        let response = client
            .get(format!("{address}/{}", case["name"].as_str().unwrap()))
            .send()
            .await
            .unwrap();
        assert_response(response, &case["response"]).await;
    }
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

fn method_error(allow: &'static str) -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, allow)],
        Json(json!({"detail":"Method Not Allowed"})),
    )
        .into_response()
}

#[tokio::test]
async fn factories_keep_health_and_lifecycle_contracts() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/apps_http_responses.json")).unwrap();
    for case in fixture["factories"].as_array().unwrap() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let kind = case["kind"].as_str().unwrap().to_owned();
        let service = NativeApi::http_factory("web", move || {
            let events = recorded.clone();
            let kind = kind.clone();
            async move {
                events.lock().unwrap().extend(["factory", "startup"]);
                let policy = match kind.as_str() {
                    "missing" => FactoryHealth::Add,
                    "raw" => FactoryHealth::Wrap,
                    _ => FactoryHealth::Preserve,
                };
                let router = Router::new().fallback(move |request: Request| {
                    let kind = kind.clone();
                    async move {
                        if kind == "raw" {
                            return Response::builder()
                                .status(209)
                                .body(Body::from("raw"))
                                .unwrap();
                        }
                        let path = percent_encoding::percent_decode_str(request.uri().path())
                            .decode_utf8_lossy();
                        if path == "/ping" && kind != "missing" {
                            if request.method().as_str().eq_ignore_ascii_case(&kind) {
                                return Response::builder()
                                    .status(203)
                                    .body(Body::from("custom"))
                                    .unwrap();
                            }
                            return method_error(if kind == "get" { "GET" } else { "POST" });
                        }
                        if path == "/echo" && request.method() == Method::POST {
                            let value = request.headers().get("x-test").cloned().unwrap();
                            let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                            return Response::builder()
                                .status(201)
                                .header("x-test", value)
                                .body(Body::from(bytes))
                                .unwrap();
                        }
                        (StatusCode::NOT_FOUND, Json(json!({"detail":"Not Found"}))).into_response()
                    }
                });
                Ok(NativeHttpApp::new(router, policy).on_shutdown(async move {
                    events.lock().unwrap().push("shutdown");
                    Ok(())
                }))
            }
        });
        assert!(
            events.lock().unwrap().is_empty(),
            "registration must be lazy"
        );
        let (address, stop, server) = host(service).await;
        let client = reqwest::Client::new();
        for (request, expected) in case["requests"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["responses"].as_array().unwrap())
        {
            let response = client
                .request(
                    request["method"].as_str().unwrap().parse().unwrap(),
                    format!("{address}{}", request["path"].as_str().unwrap()),
                )
                .header("x-test", "custom")
                .body(request["raw"].as_str().unwrap_or("").to_owned())
                .send()
                .await
                .unwrap();
            assert_response(response, expected).await;
        }
        assert_eq!(json!(*events.lock().unwrap()), case["during"]);
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
        assert_eq!(json!(*events.lock().unwrap()), case["after"]);
    }
}

#[tokio::test]
async fn streaming_flushes_before_completion_and_shutdown_drains_body() {
    let release = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let finish = release.clone();
    let state = dropped.clone();
    let mut service = NativeApi::new("stream");
    service
        .http_route(
            RouteSpec {
                method: "POST".into(),
                path: "/stream".into(),
                handler_name: "stream".into(),
            },
            NativeHttpFunction::new(move |args| {
                let finish = finish.clone();
                let state = state.clone();
                async move {
                    assert_eq!(args.args, vec![json!({"input":1})]);
                    let body = async_stream::stream! {
                        yield Ok::<_, std::io::Error>("first\n");
                        finish.notified().await;
                        yield Ok("last\n");
                        state.store(true,Ordering::SeqCst);
                    };
                    Ok(Response::new(Body::from_stream(body)))
                }
            }),
        )
        .unwrap();
    let (address, stop, mut server) = host(service).await;
    let response = reqwest::Client::new()
        .post(format!("{address}/stream"))
        .json(&json!({"input":1}))
        .send()
        .await
        .unwrap();
    let mut chunks = response.bytes_stream();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), chunks.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "first\n"
    );
    stop.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut server)
            .await
            .is_err()
    );
    release.notify_one();
    assert_eq!(chunks.next().await.unwrap().unwrap(), "last\n");
    assert!(chunks.next().await.is_none());
    server.await.unwrap().unwrap();
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn http_handlers_keep_registration_and_error_semantics() {
    let mut service = NativeApi::new("routes");
    let route = RouteSpec {
        method: "GET".into(),
        path: "/result".into(),
        handler_name: "result".into(),
    };
    service
        .route(
            route.clone(),
            runpod::apps::NativeFunction::sync(|_| Ok(json!("before"))),
        )
        .unwrap();
    service
        .http_route(
            route.clone(),
            NativeHttpFunction::new(|_| async {
                Err(Error::InvalidInput("private detail".into()))
            }),
        )
        .unwrap();
    let mut duplicate = route.clone();
    duplicate.handler_name = "other".into();
    assert!(
        service
            .http_route(
                duplicate,
                NativeHttpFunction::sync(|_| Ok(Response::new(Body::empty())))
            )
            .is_err()
    );
    let api = App::new("direct")
        .unwrap()
        .api(ResourceSpec::default(), service.clone())
        .unwrap();
    assert!(
        api.call_handler("result", FunctionArgs::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("HTTP response")
    );
    assert!(
        api.call_http_handler("result", FunctionArgs::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("private detail")
    );
    let (address, stop, server) = host(service).await;
    let response = reqwest::get(format!("{address}/result")).await.unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(response.text().await.unwrap(), "Internal Server Error");
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    // Raw bodies, duplicate headers, and status are retained by direct native calls too.
    let mut service = NativeApi::new("direct");
    service
        .http_route(
            route,
            NativeHttpFunction::sync(|_| {
                let mut response = Response::builder()
                    .status(418)
                    .body(Body::from(vec![0, 255]))
                    .unwrap();
                response
                    .headers_mut()
                    .append("x-test", HeaderValue::from_static("one"));
                response
                    .headers_mut()
                    .append("x-test", HeaderValue::from_static("two"));
                Ok(response)
            }),
        )
        .unwrap();
    let api = App::new("direct")
        .unwrap()
        .api(ResourceSpec::default(), service)
        .unwrap();
    let response = api
        .call_http_handler("result", FunctionArgs::default())
        .await
        .unwrap();
    assert_eq!(response.status(), 418);
    assert_eq!(response.headers().get_all("x-test").iter().count(), 2);
    assert_eq!(
        to_bytes(response.into_body(), 100).await.unwrap().as_ref(),
        [0, 255]
    );
}

#[tokio::test]
async fn factory_startup_failure_and_cancellation_release_resources() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for fail in [false, true] {
        let entered = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let signal = entered.clone();
        let released = dropped.clone();
        let mut service = NativeApi::http_factory("failure", move || {
            let signal = signal.clone();
            let guard = Dropped(released.clone());
            async move {
                let _guard = guard;
                signal.notify_one();
                if fail {
                    Err(Error::InvalidInput("factory startup failed".into()))
                } else {
                    std::future::pending().await
                }
            }
        });
        assert!(
            service
                .init(
                    "setup",
                    runpod::apps::NativeFunction::sync(|_| Ok(Value::Null))
                )
                .is_err()
        );
        assert!(
            service
                .http_route(
                    RouteSpec {
                        method: "GET".into(),
                        path: "/ignored".into(),
                        handler_name: "ignored".into()
                    },
                    NativeHttpFunction::sync(|_| unreachable!())
                )
                .is_err()
        );
        let api = App::new("failure")
            .unwrap()
            .api(ResourceSpec::default(), service.clone())
            .unwrap();
        assert_eq!(api.spec().unwrap().routes, []);
        assert!(api.spec().unwrap().asgi_factory.is_none());
        assert!(api.initialize().await.is_err());
        assert!(
            api.call_http_handler("ignored", FunctionArgs::default())
                .await
                .is_err()
        );
        let (address, stop, server) = host(service).await;
        tokio::time::timeout(Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        if fail {
            assert!(
                server
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("factory startup failed")
            );
        } else {
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(40),
                    reqwest::get(format!("{address}/ping"))
                )
                .await
                .is_err()
            );
            stop.send(()).unwrap();
            server.await.unwrap().unwrap();
        }
        assert!(dropped.load(Ordering::SeqCst));
        assert!(
            TcpListener::bind(address.strip_prefix("http://").unwrap())
                .await
                .is_ok(),
            "listener released"
        );
    }
}

#[tokio::test]
async fn factory_waits_for_startup_and_drains_real_router_before_cleanup() {
    let entering = Arc::new(Notify::new());
    let start = Arc::new(Notify::new());
    let handling = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let complete = Arc::new(AtomicBool::new(false));
    let signal = entering.clone();
    let ready = start.clone();
    let called = handling.clone();
    let end = finish.clone();
    let done = complete.clone();
    let service = NativeApi::http_factory("router", move || {
        let signal = signal.clone();
        let ready = ready.clone();
        let called = called.clone();
        let end = end.clone();
        let done = done.clone();
        let cleanup = done.clone();
        async move {
            signal.notify_one();
            ready.notified().await;
            let router = Router::new().route(
                "/echo/{id}",
                axum::routing::post(
                    move |axum::extract::Path(id): axum::extract::Path<u32>,
                          body: axum::body::Bytes| {
                        let called = called.clone();
                        let end = end.clone();
                        let done = done.clone();
                        async move {
                            called.notify_one();
                            end.notified().await;
                            done.store(true, Ordering::SeqCst);
                            (StatusCode::CREATED, [("x-id", id.to_string())], body)
                        }
                    },
                ),
            );
            Ok(
                NativeHttpApp::new(router, FactoryHealth::Add).on_shutdown(async move {
                    assert!(cleanup.load(Ordering::SeqCst));
                    Err(Error::InvalidInput("cleanup failed".into()))
                }),
            )
        }
    });
    let (address, stop, mut server) = host(service).await;
    tokio::time::timeout(Duration::from_secs(3), entering.notified())
        .await
        .unwrap();
    let ping = reqwest::get(format!("{address}/ping"));
    tokio::pin!(ping);
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut ping)
            .await
            .is_err()
    );
    start.notify_one();
    assert_eq!(ping.await.unwrap().status(), 200);
    let request = tokio::spawn(
        reqwest::Client::new()
            .post(format!("{address}/echo/42"))
            .body(vec![0, 255])
            .send(),
    );
    tokio::time::timeout(Duration::from_secs(3), handling.notified())
        .await
        .unwrap();
    stop.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut server)
            .await
            .is_err()
    );
    finish.notify_one();
    let response = request.await.unwrap().unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(response.headers()["x-id"], "42");
    assert_eq!(response.bytes().await.unwrap().as_ref(), [0, 255]);
    assert!(
        server
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cleanup failed")
    );
}

#[tokio::test]
async fn response_stream_error_is_a_transport_failure_after_headers() {
    let fail = Arc::new(Notify::new());
    let signal = fail.clone();
    let mut service = NativeApi::new("failure");
    service
        .http_route(
            RouteSpec {
                method: "GET".into(),
                path: "/stream".into(),
                handler_name: "stream".into(),
            },
            NativeHttpFunction::new(move |_| {
                let signal = signal.clone();
                async move {
                    let body = async_stream::stream! {
                        yield Ok("first");
                        signal.notified().await;
                        yield Err(std::io::Error::other("stream failed"));
                    };
                    Ok(Response::new(Body::from_stream(body)))
                }
            }),
        )
        .unwrap();
    let (address, stop, server) = host(service).await;
    let response = reqwest::get(format!("{address}/stream")).await.unwrap();
    assert_eq!(response.status(), 200);
    let mut chunks = response.bytes_stream();
    assert_eq!(chunks.next().await.unwrap().unwrap(), "first");
    fail.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), chunks.next())
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

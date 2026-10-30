//! Lifecycle teardown contracts from test_manage.py against real local HTTP services.
use runpod::{
    Client, ClientConfig, Value,
    apps::{
        AppsApiClient,
        events::EventSink,
        manage::{self, EnvironmentDisposition},
    },
    json,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path},
};

#[derive(Default)]
struct Backend {
    app: Value,
    apps: Vec<Value>,
    environments: Vec<Value>,
    fail: BTreeSet<String>,
    false_confirmation: bool,
}
impl Backend {
    fn regular() -> Self {
        Self {
            app: json!({"id":"app-1","name":"app","flashEnvironments":[{"id":"env-1","name":"default"}]}),
            environments: vec![
                json!({"id":"env-1","name":"default","endpoints":[{"id":"ep-0","name":"svc-0"},{"id":"ep-1","name":"svc-1"}]}),
            ],
            ..Self::default()
        }
    }
}
async fn backend(config: Backend) -> (MockServer, AppsApiClient) {
    let server = MockServer::start().await;
    let config = Arc::new(config);
    let graphql = config.clone();
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(move |request: &Request| {
            let payload: Value = request.body_json().unwrap();
            let query = payload["query"].as_str().unwrap();
            let variables = &payload["variables"];
            let (key, value, failure_key) = if query.contains("deleteFlashEnvironment") {
                (
                    "deleteFlashEnvironment",
                    json!(!graphql.false_confirmation),
                    format!(
                        "environment:{}",
                        variables["flashEnvironmentId"].as_str().unwrap()
                    ),
                )
            } else if query.contains("deleteFlashApp") {
                (
                    "deleteFlashApp",
                    json!(!graphql.false_confirmation),
                    "app".into(),
                )
            } else if query.contains("flashEnvironmentByName") {
                (
                    "flashEnvironmentByName",
                    graphql
                        .environments
                        .iter()
                        .find(|env| env["name"] == variables["input"]["name"])
                        .cloned()
                        .unwrap_or(Value::Null),
                    "lookup".into(),
                )
            } else if query.contains("flashAppByName") {
                ("flashAppByName", graphql.app.clone(), "app_lookup".into())
            } else {
                ("myself", json!({"flashApps":graphql.apps}), "list".into())
            };
            if graphql.fail.contains(&failure_key) {
                ResponseTemplate::new(200).set_body_json(json!({"errors":[{"message":"in use"}]}))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"data":{key:value}}))
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(move |request: &Request| {
            let id = request.url.path().rsplit('/').next().unwrap();
            if config.fail.contains(id) {
                ResponseTemplate::new(409).set_body_json(json!({"error":"in use"}))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({}))
            }
        })
        .mount(&server)
        .await;
    let api = AppsApiClient::new(
        Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap(),
    )
    .unwrap();
    (server, api)
}
async fn mutations(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter_map(|request| {
            if request.method == "DELETE" {
                return Some(request.url.path().to_owned());
            }
            let payload: Value = request.body_json().unwrap();
            let query = payload["query"].as_str().unwrap();
            if query.contains("deleteFlashEnvironment") {
                Some(format!(
                    "env:{}",
                    payload["variables"]["flashEnvironmentId"].as_str().unwrap()
                ))
            } else if query.contains("deleteFlashApp") {
                Some("app".into())
            } else {
                None
            }
        })
        .collect()
}

#[tokio::test]
async fn test_list_sorted_by_name() {
    let (_server, api) = backend(Backend {
        apps: vec![
            json!({"name":"zeta"}),
            json!({"name":"alpha","id":1}),
            json!({"id":2}),
            json!({"name":"alpha","id":3}),
        ],
        ..Backend::default()
    })
    .await;
    let apps = manage::list_apps(&api).await.unwrap();
    assert_eq!(
        apps.iter().map(|v| v["id"].clone()).collect::<Vec<_>>(),
        vec![json!(2), json!(1), json!(3), Value::Null]
    );
}
#[tokio::test]
async fn test_get_app_not_found() {
    let (_server, api) = backend(Backend::default()).await;
    assert!(
        matches!(manage::get_app("ghost", &api).await, Err(runpod::Error::App(runpod::apps::AppError::AppNotFound(name))) if name=="ghost")
    );
}
#[tokio::test]
async fn test_get_environment_not_found() {
    let (_server, api) = backend(Backend::regular()).await;
    assert!(
        manage::get_environment("app", "ghost", &api)
            .await
            .unwrap_err()
            .to_string()
            .contains("no environment 'ghost'")
    );
}
#[tokio::test]
async fn test_deletes_endpoints_then_environment() {
    let (server, api) = backend(Backend::regular()).await;
    let result =
        manage::undeploy_environment("app", "default", EnvironmentDisposition::Delete, &api, None)
            .await
            .unwrap();
    assert_eq!(result.endpoints_deleted, 2);
    assert!(result.environment_deleted);
    assert!(result.failures.is_empty());
    assert_eq!(
        mutations(&server).await,
        ["/v2/serverless/ep-0", "/v2/serverless/ep-1", "env:env-1"]
    );
}
#[tokio::test]
async fn test_keep_env_flag() {
    let (server, api) = backend(Backend::regular()).await;
    let result =
        manage::undeploy_environment("app", "default", EnvironmentDisposition::Keep, &api, None)
            .await
            .unwrap();
    assert_eq!(result.endpoints_deleted, 2);
    assert!(!result.environment_deleted);
    assert_eq!(
        mutations(&server).await,
        ["/v2/serverless/ep-0", "/v2/serverless/ep-1"]
    );
}
#[tokio::test]
async fn test_endpoint_failure_keeps_environment() {
    let (server, api) = backend(Backend {
        fail: ["ep-0".into()].into(),
        ..Backend::regular()
    })
    .await;
    let result =
        manage::undeploy_environment("app", "default", EnvironmentDisposition::Delete, &api, None)
            .await
            .unwrap();
    assert_eq!(result.endpoints_deleted, 1);
    assert!(!result.environment_deleted);
    assert_eq!(result.failures.len(), 1);
    assert!(result.failures[0].starts_with("endpoint svc-0:"));
    assert_eq!(
        mutations(&server).await,
        ["/v2/serverless/ep-0", "/v2/serverless/ep-1"]
    );
}
#[tokio::test]
async fn test_events_emitted() {
    let (_server, api) = backend(Backend::regular()).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let sink = EventSink::new(move |name, args| {
        captured
            .lock()
            .unwrap()
            .push((name.to_owned(), args.to_vec()));
        Ok(())
    });
    manage::undeploy_environment(
        "app",
        "default",
        EnvironmentDisposition::Delete,
        &api,
        Some(&sink),
    )
    .await
    .unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            ("cleanup_started".into(), vec![json!(2)]),
            ("deleting".into(), vec![json!("svc-0")]),
            ("deleted".into(), vec![json!("svc-0")]),
            ("deleting".into(), vec![json!("svc-1")]),
            ("deleted".into(), vec![json!("svc-1")])
        ]
    );
}
#[tokio::test]
async fn test_undeploys_all_envs_then_app() {
    let mut config = Backend::regular();
    config.app["flashEnvironments"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"env-2","name":"staging"}));
    config
        .environments
        .push(json!({"id":"env-2","name":"staging","endpoints":[]}));
    let (server, api) = backend(config).await;
    let result = manage::delete_app("app", &api, None).await.unwrap();
    assert_eq!(result.endpoints_deleted, 2);
    assert!(result.app_deleted);
    assert!(!result.environment_deleted);
    assert!(result.failures.is_empty());
    assert_eq!(
        mutations(&server).await,
        [
            "/v2/serverless/ep-0",
            "/v2/serverless/ep-1",
            "env:env-1",
            "env:env-2",
            "app"
        ]
    );
}
#[tokio::test]
async fn test_failure_keeps_app() {
    let (server, api) = backend(Backend {
        fail: ["ep-0".into()].into(),
        ..Backend::regular()
    })
    .await;
    let result = manage::delete_app("app", &api, None).await.unwrap();
    assert!(!result.app_deleted);
    assert_eq!(result.failures.len(), 1);
    assert_eq!(
        mutations(&server).await,
        ["/v2/serverless/ep-0", "/v2/serverless/ep-1"]
    );
}

#[tokio::test]
async fn environment_and_app_delete_failures_are_qualified_and_collected() {
    for failure in ["environment:env-1", "app"] {
        let (server, api) = backend(Backend {
            fail: [failure.into()].into(),
            ..Backend::regular()
        })
        .await;
        let result = manage::delete_app("app", &api, None).await.unwrap();
        assert_eq!(result.endpoints_deleted, 2);
        assert!(!result.app_deleted);
        assert_eq!(result.failures.len(), 1);
        assert!(result.failures[0].starts_with(if failure == "app" {
            "app app:"
        } else {
            "environment default:"
        }));
        assert_eq!(
            mutations(&server).await.last().unwrap(),
            if failure == "app" { "app" } else { "env:env-1" }
        );
    }
}

#[tokio::test]
async fn false_confirmations_preserve_upstream_completed_call_flags() {
    let (_server, api) = backend(Backend {
        false_confirmation: true,
        ..Backend::regular()
    })
    .await;
    let env =
        manage::undeploy_environment("app", "default", EnvironmentDisposition::Delete, &api, None)
            .await
            .unwrap();
    assert!(env.environment_deleted);
    let app = manage::delete_app("app", &api, None).await.unwrap();
    assert!(app.app_deleted);
    assert!(!app.environment_deleted);
}

#[tokio::test]
async fn empty_environment_and_app_are_deleted_without_endpoint_calls() {
    let mut config = Backend::regular();
    config.environments[0]["endpoints"] = Value::Null;
    config.app["flashEnvironments"] = Value::Null;
    let (server, api) = backend(config).await;
    let result =
        manage::undeploy_environment("app", "default", EnvironmentDisposition::Delete, &api, None)
            .await
            .unwrap();
    assert_eq!(result.endpoints_deleted, 0);
    assert!(result.environment_deleted);
    assert!(
        manage::delete_app("app", &api, None)
            .await
            .unwrap()
            .app_deleted
    );
    assert_eq!(mutations(&server).await, ["env:env-1", "app"]);
}

#[tokio::test]
async fn endpoint_name_fallback_failed_event_and_observer_failure_do_not_stop_cleanup() {
    let mut config = Backend::regular();
    config.environments[0]["endpoints"] =
        json!([{"id":"ep-0","name":""},{"name":"missing-id"},{"id":"ep-1"}]);
    let (server, api) = backend(config).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let sink = EventSink::new(move |name, args| {
        captured
            .lock()
            .unwrap()
            .push((name.to_owned(), args.to_vec()));
        Err(runpod::Error::InvalidInput("observer failed".into()))
    });
    let result = manage::undeploy_environment(
        "app",
        "default",
        EnvironmentDisposition::Delete,
        &api,
        Some(&sink),
    )
    .await
    .unwrap();
    assert_eq!(result.endpoints_deleted, 2);
    assert_eq!(result.failures.len(), 1);
    assert!(!result.environment_deleted);
    let events = events.lock().unwrap().clone();
    assert!(events.contains(&("deleting".into(), vec![json!("ep-0")])));
    assert!(events.contains(&("delete_failed".into(), vec![json!("missing-id")])));
    assert!(events.contains(&("deleted".into(), vec![json!("ep-1")])));
    assert_eq!(
        mutations(&server).await,
        ["/v2/serverless/ep-0", "/v2/serverless/ep-1"]
    );
}

#[tokio::test]
async fn child_failure_does_not_prevent_other_environments_but_lookup_failure_propagates() {
    let mut config = Backend {
        fail: ["ep-0".into()].into(),
        ..Backend::regular()
    };
    config.app["flashEnvironments"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"staging"}));
    config
        .environments
        .push(json!({"id":"env-2","name":"staging","endpoints":[]}));
    let (server, api) = backend(config).await;
    let result = manage::delete_app("app", &api, None).await.unwrap();
    assert!(!result.app_deleted);
    assert_eq!(result.failures.len(), 1);
    assert_eq!(mutations(&server).await.last().unwrap(), "env:env-2");
    let (server, api) = backend(Backend {
        fail: ["lookup".into()].into(),
        ..Backend::regular()
    })
    .await;
    assert!(manage::delete_app("app", &api, None).await.is_err());
    assert!(mutations(&server).await.is_empty());
}

#[tokio::test]
async fn cancellation_stops_later_deletions_without_rollback() {
    let (server, api) = backend(Backend::regular()).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    Mock::given(method("DELETE"))
        .and(path("/v2/serverless/ep-0"))
        .respond_with(move |_: &Request| {
            signal.notify_one();
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(200))
                .set_body_json(json!({}))
        })
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    let deletion = tokio::spawn(async move { manage::delete_app("app", &api, None).await });
    started.notified().await;
    deletion.abort();
    assert!(deletion.await.unwrap_err().is_cancelled());
    assert_eq!(mutations(&server).await, ["/v2/serverless/ep-0"]);
}

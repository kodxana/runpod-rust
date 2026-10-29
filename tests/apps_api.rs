//! Wire contracts captured by executing the pinned Python Apps API.
use runpod::{
    Client, ClientConfig, Value,
    apps::{AppsApiClient, api::Product, volume::ComputeType},
    json,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path, query_param},
};

fn client(server: &MockServer) -> AppsApiClient {
    AppsApiClient::new(
        Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("test-key")
        })
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn python_apps_api_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/apps_api.json")).unwrap();
    for case in cases {
        let server = MockServer::start().await;
        for request in case["requests"].as_array().unwrap() {
            let anonymous = request["anonymous"].as_bool().unwrap();
            let mut mock = Mock::given(method(request["method"].as_str().unwrap()))
                .and(path(request["path"].as_str().unwrap()))
                .and(move |r: &wiremock::Request| {
                    if anonymous {
                        !r.headers.contains_key("authorization")
                    } else {
                        r.headers
                            .get("authorization")
                            .is_some_and(|h| h == "Bearer test-key")
                    }
                });
            if !request["body"].is_null() {
                mock = mock.and(body_json(request["body"].clone()));
            }
            for (key, value) in request["params"].as_object().unwrap() {
                mock = mock.and(query_param(
                    key,
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                ));
            }
            mock.respond_with(ResponseTemplate::new(200).set_body_json(&request["response"]))
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
        }
        let c = client(&server);
        let k = &case["kwargs"];
        let s = |key: &str| k[key].as_str().unwrap_or("");
        let product = if k["pods"] == true {
            Product::Pod
        } else {
            Product::Serverless
        };
        let result = match case["operation"].as_str().unwrap() {
            "save_endpoint" => c.save_endpoint(&k["endpoint_input"]).await,
            "delete_endpoint" => c.delete_endpoint(s("endpoint_id")).await.map(|v| json!(v)),
            "list_my_endpoints" => c.list_my_endpoints().await.map(|v| json!(v)),
            "endpoint_workers" => c.endpoint_workers(s("endpoint_id")).await,
            "deploy_task_pod" => {
                c.deploy_task_pod(
                    &k["pod_input"],
                    if k["is_cpu"] == true {
                        ComputeType::Cpu
                    } else {
                        ComputeType::Gpu
                    },
                )
                .await
            }
            "terminate_pod" => c.terminate_pod(s("pod_id")).await.map(|()| Value::Null),
            "get_app_by_name" => c.get_app_by_name(s("name")).await.map(|v| json!(v)),
            "create_app" => c.create_app(s("name")).await,
            "create_environment" => c.create_environment(s("app_id"), s("name")).await,
            "gpu_stock_status" => c
                .gpu_stock_status(
                    s("gpu_id"),
                    s("data_center_id"),
                    k["gpu_count"].as_u64().unwrap_or(1),
                    product,
                )
                .await
                .map(|v| json!(v)),
            "cpu_stock_status" => c
                .cpu_stock_status(s("instance_id"), s("data_center_id"), product)
                .await
                .map(|v| json!(v)),
            "network_volume_datacenters" => c.network_volume_datacenters().await.map(|v| json!(v)),
            "list_global_volumes" => c.list_global_volumes().await.map(|v| json!(v)),
            "create_global_volume" => c.create_global_volume(s("name")).await,
            "list_network_volumes" => c.list_network_volumes().await.map(|v| json!(v)),
            "create_network_volume" => {
                c.create_network_volume(s("name"), k["size"].as_u64().unwrap(), s("data_center_id"))
                    .await
            }
            "list_registry_auths" => c.list_registry_auths().await.map(|v| json!(v)),
            "create_registry_auth" => {
                c.create_registry_auth(s("name"), s("username"), s("password"))
                    .await
            }
            "delete_registry_auth" => c.delete_registry_auth(s("auth_id")).await.map(|v| json!(v)),
            "list_secrets" => c.list_secrets().await.map(|v| json!(v)),
            "create_secret" => {
                c.create_secret(s("name"), s("value"), s("description"))
                    .await
            }
            "delete_secret" => c.delete_secret(s("secret_id")).await.map(|v| json!(v)),
            "list_apps" => c.list_apps().await.map(|v| json!(v)),
            "get_environment_by_name" => c
                .get_environment_by_name(s("app_name"), s("env_name"))
                .await
                .map(|v| json!(v)),
            "delete_app" => c.delete_app(s("app_id")).await.map(|v| json!(v)),
            "delete_environment" => c
                .delete_environment(s("environment_id"))
                .await
                .map(|v| json!(v)),
            "create_auth_request" => c.create_auth_request().await,
            "get_auth_request_status" => c.get_auth_request_status(s("request_id")).await,
            "prepare_artifact_upload" => {
                c.prepare_artifact_upload(s("app_id"), k["tarball_size"].as_u64().unwrap())
                    .await
            }
            "finalize_artifact_upload" => {
                c.finalize_artifact_upload(s("app_id"), s("object_key"), &k["manifest"])
                    .await
            }
            "deploy_build" => c.deploy_build(s("environment_id"), s("build_id")).await,
            other => panic!("unmapped operation {other}"),
        };
        if let Some(error) = case.get("error") {
            assert_eq!(
                result.unwrap_err().to_string(),
                error.as_str().unwrap(),
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                result.unwrap_or_else(|e| panic!("{}: {e}", case["name"])),
                case["output"],
                "{}",
                case["name"]
            );
        }
        server.verify().await;
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            case["requests"].as_array().unwrap().len(),
            "unexpected extra request: {}",
            case["name"]
        );
    }
}

#[tokio::test]
async fn test_rest_save_failure_never_retries_or_falls_back() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/serverless"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({"detail":"unavailable"})))
        .expect(1)
        .mount(&server)
        .await;
    let result = client(&server).save_endpoint(&json!({"name":"chat","template":{"imageName":"image"},"scalerType":"QUEUE_DELAY","scalerValue":4})).await;
    assert_eq!(result.unwrap_err().status_code(), Some(503));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn test_graphql_errors_propagate_immediately() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"errors":[{"message":"denied"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client(&server).list_apps().await.unwrap_err().to_string(),
        "denied"
    );
}

#[tokio::test]
async fn test_create_auth_request_is_anonymous() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(|r: &wiremock::Request| !r.headers.contains_key("authorization"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"createFlashAuthRequest":{"id":"auth"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = AppsApiClient::anonymous(server.uri()).unwrap();
    assert_eq!(c.create_auth_request().await.unwrap(), json!({"id":"auth"}));
}

#[tokio::test]
async fn test_upload_retries_then_succeeds_and_reports_progress() {
    use std::sync::{Arc, Mutex};
    let server = MockServer::start().await;
    let file = tempfile::NamedTempFile::new().unwrap();
    let bytes = vec![42; 1024 * 1024 + 17];
    std::fs::write(file.path(), &bytes).unwrap();
    Mock::given(method("PUT"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let updates = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&updates);
    client(&server)
        .upload_tarball(
            &format!("{}/upload?signature=dummy", server.uri()),
            file.path(),
            move |sent, total| record.lock().unwrap().push((sent, total)),
        )
        .await
        .unwrap();
    for r in server.received_requests().await.unwrap() {
        assert!(!r.headers.contains_key("authorization"));
        assert_eq!(r.body, bytes);
        assert_eq!(r.headers["content-type"], "application/gzip");
        assert_eq!(r.headers["content-length"], bytes.len().to_string());
    }
    let updates = updates.lock().unwrap();
    assert_eq!(
        updates
            .iter()
            .filter(|(sent, _)| *sent == bytes.len() as u64)
            .count(),
        2
    );
    assert!(
        updates
            .iter()
            .all(|(_, total)| *total == bytes.len() as u64)
    );
}

async fn interrupted_responses(
    failures: usize,
    succeed: bool,
) -> (AppsApiClient, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        for index in 0..failures + usize::from(succeed) {
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let count = connection.read(&mut buf).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let response = if index < failures {
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{".to_owned()
            } else {
                let body = r#"{"data":{"myself":{"flashApps":[{"id":"ok"}]}}}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            connection.write_all(response.as_bytes()).await.unwrap();
            connection.shutdown().await.unwrap();
        }
    });
    let client = AppsApiClient::new(
        Client::with_config(ClientConfig {
            api_base_url: url,
            ..ClientConfig::new("key")
        })
        .unwrap(),
    )
    .unwrap();
    (client, handle)
}

#[tokio::test]
async fn test_retries_transport_errors() {
    let (c, server) = interrupted_responses(3, true).await;
    assert_eq!(c.list_apps().await.unwrap(), vec![json!({"id":"ok"})]);
    server.await.unwrap();
}

#[tokio::test]
async fn test_exhausted_retries_raise() {
    let (c, server) = interrupted_responses(4, false).await;
    assert!(matches!(c.list_apps().await, Err(runpod::Error::Http(_))));
    server.await.unwrap();
}

#[tokio::test]
async fn test_mutation_transport_failure_is_not_retried() {
    let (c, server) = interrupted_responses(1, false).await;
    let start = std::time::Instant::now();
    assert!(matches!(
        c.create_app("app").await,
        Err(runpod::Error::Http(_))
    ));
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    server.await.unwrap();
}

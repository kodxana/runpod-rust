use runpod::{
    Value,
    apps::{
        ResourceSpec,
        images::RuntimeImages,
        shim::runtime_launcher,
        tasks::{TaskConfig, cuda_versions_at_least, pod_input, unwrap_task_response},
    },
    json,
};

#[test]
fn python_task_payload_and_runtime_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/tasks.json")).unwrap();
    for case in cases {
        let input = &case["input"];
        let images = RuntimeImages {
            python_version: "3.12".into(),
            tag: "test".into(),
        };
        let result = match case["operation"].as_str().unwrap() {
            "image" => RuntimeImages {
                python_version: input["version"].as_str().unwrap().into(),
                tag: "test".into(),
            }
            .runtime_image(
                serde_json::from_value(input["kind"].clone()).unwrap(),
                if input["gpu"].as_bool().unwrap() {
                    runpod::apps::volume::ComputeType::Gpu
                } else {
                    runpod::apps::volume::ComputeType::Cpu
                },
            )
            .map(Value::String),
            "launcher" => Ok(json!(runtime_launcher(
                serde_json::from_value(input.clone()).unwrap()
            ))),
            "pod" => pod_input(
                &serde_json::from_value::<ResourceSpec>(input.clone()).unwrap(),
                "test-token",
                "calculate",
                "deadbeef",
                &TaskConfig {
                    images,
                    runtime_package_spec: Some("runtime==2".into()),
                    sdk_package_spec: Some("sdk==3".into()),
                    ..TaskConfig::default()
                },
            ),
            "cuda" => cuda_versions_at_least(input.as_str().unwrap()).map(|v| json!(v)),
            "unwrap" => unwrap_task_response(input),
            "custom_image" => RuntimeImages {
                python_version: input.as_str().unwrap().into(),
                ..images
            }
            .image_for_spec(
                &serde_json::from_value(json!({"kind":"task","name":"custom","image":"custom:1"}))
                    .unwrap(),
            )
            .map(Value::String),
            other => panic!("unknown operation {other}"),
        };
        if let Some(expected) = case.get("error") {
            let error = result.unwrap_err().to_string();
            if case["operation"] == "cuda" {
                assert!(!error.is_empty());
            } else {
                assert_eq!(error, expected.as_str().unwrap(), "{case}");
            }
        } else {
            assert_eq!(result.unwrap(), case["output"], "{case}");
        }
    }
}

use runpod::{
    Client, ClientConfig,
    apps::{
        AppsApiClient,
        tasks::{TaskExecution, TaskJob},
    },
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn execution(server: &MockServer) -> TaskExecution {
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("api-secret")
    })
    .unwrap();
    TaskExecution::with_config(serde_json::from_value(json!({"kind":"task","name":"calculate","gpu":"4090","datacenter":["EU-RO-1","US-KS-2"]})).unwrap(),
        AppsApiClient::new(client).unwrap(), TaskConfig {proxy_base_url:Some(server.uri()), ..TaskConfig::default()}, None).unwrap()
}
async fn deployed(server: &MockServer) -> TaskExecution {
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"podFindAndDeployOnDemand":{"id":"pod"}}})),
        )
        .expect(1)
        .mount(server)
        .await;
    let task = execution(server);
    task.start().await.unwrap();
    task
}
async fn deleted(server: &MockServer, times: u64) {
    Mock::given(method("DELETE"))
        .and(path("/v2/pods/pod"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn task_job_reads_caches_and_cleans_up_with_separate_runner_authentication() {
    let server = MockServer::start().await;
    let task = deployed(&server).await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    task.wait_ready(Duration::from_secs(5)).await.unwrap();
    Mock::given(path("/submit"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    task.submit(&json!({"args":[]})).await.unwrap();
    Mock::given(path("/result"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"status":"DONE","response":{"success":true,"json_result":42}}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    deleted(&server, 1).await;
    let mut job = TaskJob::new(task.clone());
    assert_eq!(job.pod_id().as_deref(), Some("pod"));
    assert_eq!(job.wait(None).await.unwrap(), json!(42));
    assert_eq!(job.wait(Some(Duration::ZERO)).await.unwrap(), json!(42));
    assert_eq!(job.pod_id(), None);
    let requests = server.received_requests().await.unwrap();
    let pod: Value = requests[0].body_json().unwrap();
    let token = pod["variables"]["input"]["env"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "RUNPOD_TASK_TOKEN")
        .unwrap()["value"]
        .as_str()
        .unwrap();
    assert_eq!(token.len(), 43);
    for request in &requests {
        match request.url.path() {
            "/ping" => assert!(!request.headers.contains_key("authorization")),
            "/submit" | "/result" => {
                assert_eq!(request.headers["authorization"], format!("Bearer {token}"))
            }
            _ => assert_eq!(request.headers["authorization"], "Bearer api-secret"),
        }
    }
    assert!(!format!("{task:?}").contains(token));
}

#[tokio::test]
async fn task_timeout_validation_and_acknowledgement_prevent_silent_runtime_mismatch() {
    let server = MockServer::start().await;
    let task = deployed(&server).await;
    for timeout in [json!(-1), json!(true), json!("5"), json!([])] {
        assert_eq!(
            task.submit(&json!({"timeout":timeout}))
                .await
                .unwrap_err()
                .to_string(),
            "task timeout must be a finite non-negative number"
        );
    }
    Mock::given(path("/timeout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        task.set_timeout(Duration::from_secs(5))
            .await
            .unwrap_err()
            .to_string()
            .contains("did not acknowledge")
    );
    deleted(&server, 1).await;
    task.terminate().await.unwrap();
}

#[tokio::test]
async fn capacity_fallback_tries_datacenters_and_preserves_the_initial_error() {
    for succeed in [true, false] {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        Mock::given(path("/graphql"))
            .respond_with(move |_: &wiremock::Request| {
                let n = count.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(if succeed && n == 2 {
                    json!({"data":{"podFindAndDeployOnDemand":{"id":"pod"}}})
                } else {
                    json!({"errors":[{"message":format!("insufficient capacity {n}")}]})
                })
            })
            .expect(3)
            .mount(&server)
            .await;
        let task = execution(&server);
        if succeed {
            task.start().await.unwrap();
            deleted(&server, 1).await;
        } else {
            assert!(
                task.start()
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("insufficient capacity 0")
            );
        }
        task.terminate().await.unwrap();
        let requests = server.received_requests().await.unwrap();
        for (n, expected) in [
            json!(["EU-RO-1", "US-KS-2"]),
            json!(["EU-RO-1"]),
            json!(["US-KS-2"]),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                requests[n].body_json::<Value>().unwrap()["variables"]["input"]["dataCenterIds"],
                expected
            );
        }
    }
}

#[tokio::test]
async fn failed_delete_retains_the_pod_id_for_manual_retry_and_not_found_is_success() {
    let server = MockServer::start().await;
    let task = deployed(&server).await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error":"forbidden"})))
        .with_priority(1)
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({})))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(task.terminate().await.unwrap_err().status_code(), Some(403));
    assert_eq!(task.pod_id().as_deref(), Some("pod"));
    task.terminate().await.unwrap();
    task.terminate().await.unwrap();
    assert_eq!(task.pod_id(), None);
}

#[tokio::test]
async fn canceling_creation_still_records_the_late_pod_for_cleanup() {
    let server = MockServer::start().await;
    let arrived = Arc::new(tokio::sync::Notify::new());
    let notify = arrived.clone();
    Mock::given(path("/graphql"))
        .respond_with(move |_: &wiremock::Request| {
            notify.notify_one();
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(100))
                .set_body_json(json!({"data":{"podFindAndDeployOnDemand":{"id":"pod"}}}))
        })
        .expect(1)
        .mount(&server)
        .await;
    deleted(&server, 1).await;
    let task = execution(&server);
    let owned = task.clone();
    let start = tokio::spawn(async move { owned.start().await });
    tokio::time::timeout(Duration::from_secs(5), arrived.notified())
        .await
        .unwrap();
    start.abort();
    assert!(start.await.unwrap_err().is_cancelled());
    task.terminate().await.unwrap();
    assert_eq!(task.pod_id(), None);
}

use runpod::apps::{
    events::EventSink,
    logs::LogsClient,
    serialization::{FunctionSource, deserialize_result},
    targets::{FunctionDefinition, PodTarget},
};
fn target(server: &MockServer) -> PodTarget {
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("api-secret")
    })
    .unwrap();
    let mut target = PodTarget::with_config(
        client.clone(),
        serde_json::from_value(json!({"kind":"task","name":"calculate","cpu":"cpu5c-2-4"}))
            .unwrap(),
        FunctionDefinition {
            name: "calculate".into(),
            parameters: vec!["x".into()],
            source: Some(FunctionSource::Text(
                "def calculate(x): return x + 1".into(),
            )),
        },
        TaskConfig {
            proxy_base_url: Some(server.uri()),
            ..TaskConfig::default()
        },
        None,
    )
    .unwrap();
    target.set_logs_client(LogsClient::with_hapi_base(client, server.uri()).unwrap());
    target
}

#[tokio::test]
async fn pod_target_payload_and_completion_events_follow_cleanup() {
    let server = MockServer::start().await;
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"deployCpuPod":{"id":"pod"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/submit"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/result"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"status":"DONE","response":{"success":true,"json_result":2}}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/pods/pod/logs"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    Mock::given(path("/v1/pod/pod/logs"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"container":["2099-01-01T00:00:00Z result log"]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    deleted(&server, 1).await;
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = events.clone();
    let mut target = target(&server);
    target.set_events(Some(EventSink::new(move |name, _| {
        record.lock().unwrap().push(name.to_owned());
        Ok(())
    })));
    let payload = target
        .build_payload(&[json!(1)], &serde_json::Map::new())
        .await
        .unwrap();
    assert_eq!(payload["function_name"], "calculate");
    assert_eq!(payload["serialization_format"], "cloudpickle");
    assert_eq!(
        deserialize_result::<Value>(payload["args"][0].as_str().unwrap()).unwrap(),
        json!(1)
    );
    assert!(payload.get("input").is_none());
    assert_eq!(target.invoke(&payload, None).await.unwrap(), json!(2));
    assert_eq!(
        *events.lock().unwrap(),
        [
            "dispatch",
            "task_status",
            "task_status",
            "worker_ready",
            "worker_log",
            "request_completed"
        ]
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .last()
            .unwrap()
            .method,
        "DELETE"
    );
}

#[tokio::test]
async fn canceled_target_creation_and_wait_continue_cleanup_in_the_background() {
    for phase in ["creation", "wait"] {
        let server = MockServer::start().await;
        let arrived = Arc::new(tokio::sync::Notify::new());
        let notify = arrived.clone();
        Mock::given(path("/graphql"))
            .respond_with(move |_: &wiremock::Request| {
                if phase == "creation" {
                    notify.notify_one();
                }
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(50))
                    .set_body_json(json!({"data":{"deployCpuPod":{"id":"pod"}}}))
            })
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/ping"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(path("/submit"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let notify = arrived.clone();
        Mock::given(path("/result"))
            .respond_with(move |_: &wiremock::Request| {
                notify.notify_one();
                ResponseTemplate::new(200).set_delay(Duration::from_secs(60))
            })
            .mount(&server)
            .await;
        let deleted = Arc::new(tokio::sync::Notify::new());
        let notify = deleted.clone();
        Mock::given(method("DELETE"))
            .respond_with(move |_: &wiremock::Request| {
                notify.notify_one();
                ResponseTemplate::new(200).set_body_json(json!({}))
            })
            .expect(1)
            .mount(&server)
            .await;
        let target = target(&server);
        let running = tokio::spawn(async move {
            if phase == "creation" {
                target.invoke(&json!({}), None).await
            } else {
                target.submit(&json!({})).await.unwrap().wait(None).await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), arrived.notified())
            .await
            .unwrap();
        running.abort();
        assert!(running.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(5), deleted.notified())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn runner_404_retries_are_bounded_and_other_errors_are_not_replayed() {
    async fn check(route: &str, status: u16, persistent: bool) {
        let server = MockServer::start().await;
        let task = deployed(&server).await;
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let route_owned = route.to_owned();
        Mock::given(path(route))
            .respond_with(move |_: &wiremock::Request| {
                let attempt = calls.fetch_add(1, Ordering::SeqCst);
                if persistent || attempt < 2 {
                    ResponseTemplate::new(status).set_body_json(json!({"error":"retry?"}))
                } else {
                    ResponseTemplate::new(200).set_body_json(if route_owned == "/result" {
                        json!({"status":"DONE","response":{"success":true}})
                    } else {
                        json!({"timeout":5})
                    })
                }
            })
            .expect(if status == 404 {
                if persistent { 6 } else { 3 }
            } else {
                1
            })
            .mount(&server)
            .await;
        let result = if route == "/result" {
            task.poll_result().await.map(|r| {
                assert_eq!(r, Some(json!({"success":true})));
            })
        } else {
            task.submit(&json!({"timeout":5})).await
        };
        if persistent || status != 404 {
            assert_eq!(result.unwrap_err().status_code(), Some(status));
        } else {
            result.unwrap();
        }
        deleted(&server, 1).await;
        task.terminate().await.unwrap();
    }
    tokio::join!(
        check("/result", 404, false),
        check("/submit", 404, false),
        check("/result", 404, true),
        check("/submit", 404, true),
        check("/result", 503, true),
        check("/submit", 503, true)
    );
}

#[tokio::test]
async fn job_errors_and_zero_timeout_always_clean_up_and_preserve_original_failure() {
    for case in ["ack", "zero", "remote", "poll", "both"] {
        let server = MockServer::start().await;
        let task = deployed(&server).await;
        let timeout = match case {
            "ack" => Some(Duration::from_secs(1)),
            "zero" => Some(Duration::ZERO),
            _ => None,
        };
        if timeout.is_some() {
            Mock::given(path("/timeout"))
                .respond_with(ResponseTemplate::new(200).set_body_json(if case == "zero" {
                    json!({"timeout":0})
                } else {
                    json!({})
                }))
                .expect(1)
                .mount(&server)
                .await;
        } else {
            Mock::given(path("/result")).respond_with(if case=="poll" {ResponseTemplate::new(401)} else {ResponseTemplate::new(200).set_body_json(json!({"status":"DONE","response":{"success":false,"error":"original failure"}}))}).expect(1).mount(&server).await;
        }
        Mock::given(method("DELETE"))
            .respond_with(
                ResponseTemplate::new(if case == "both" { 403 } else { 200 })
                    .set_body_json(json!({})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut job = TaskJob::new(task);
        let error = job.wait(timeout).await.unwrap_err();
        match case {
            "ack" => assert!(error.to_string().contains("did not acknowledge")),
            "zero" => assert!(error.to_string().contains("did not finish in 0s")),
            "poll" => assert_eq!(error.status_code(), Some(401)),
            _ => assert_eq!(error.to_string(), "task execution failed: original failure"),
        }
        assert_eq!(job.pod_id().is_some(), case == "both");
    }
}

#[tokio::test]
async fn running_task_then_cancel_caches_null_and_unlimited_execute_submits_null_timeout() {
    let server = MockServer::start().await;
    let task = deployed(&server).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    Mock::given(path("/result"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(if count.fetch_add(1, Ordering::SeqCst) == 0 {
                json!({"status":"RUNNING"})
            } else {
                json!({"status":"DONE","response":{"success":true,"json_result":null}})
            })
        })
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/submit"))
        .and(wiremock::matchers::body_json(json!({"timeout":null})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let response = task.execute(&json!({}), None).await.unwrap();
    assert_eq!(unwrap_task_response(&response).unwrap(), Value::Null);
    deleted(&server, 1).await;
    let mut job = TaskJob::new(task);
    job.cancel().await.unwrap();
    assert_eq!(job.wait(None).await.unwrap(), Value::Null);
}

#[tokio::test]
async fn delete_retries_transient_responses_and_coalesces_concurrent_terminators() {
    let server = MockServer::start().await;
    let task = deployed(&server).await;
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    Mock::given(method("DELETE"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(match calls.fetch_add(1, Ordering::SeqCst) {
                0 => 429,
                1 => 503,
                _ => 200,
            })
            .set_body_json(json!({}))
        })
        .expect(3)
        .mount(&server)
        .await;
    let (one, two) = tokio::join!(task.terminate(), task.terminate());
    one.unwrap();
    two.unwrap();
    assert_eq!(task.pod_id(), None);
}

#[tokio::test]
async fn cleanup_deadline_leaves_owned_cleanup_running() {
    use runpod::apps::tasks::finish_cleanup;
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(finish_cleanup(async move {
        started.send(()).unwrap();
        wait.await.unwrap();
        finished.send(()).unwrap();
        Ok(())
    }));
    ready.await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        task.await.unwrap().unwrap_err().to_string(),
        "task pod cleanup is still pending"
    );
    release.send(()).unwrap();
    done.await.unwrap();
    tokio::time::resume();
}

#[tokio::test]
async fn setup_failures_clean_up_and_non_capacity_creation_is_never_retried() {
    for phase in ["start", "ready", "submit"] {
        let server = MockServer::start().await;
        Mock::given(path("/graphql"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(if phase == "start" {
                    json!({"errors":[{"message":"invalid image"}]})
                } else {
                    json!({"data":{"deployCpuPod":{"id":"pod"}}})
                }),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("api-secret")
        })
        .unwrap();
        let target = PodTarget::with_config(
            client,
            serde_json::from_value(json!({"kind":"task","name":"calculate","cpu":"cpu5c-2-4"}))
                .unwrap(),
            FunctionDefinition {
                name: "f".into(),
                parameters: vec![],
                source: None,
            },
            TaskConfig {
                proxy_base_url: Some(server.uri()),
                ready_timeout: if phase == "ready" {
                    Duration::ZERO
                } else {
                    Duration::from_secs(600)
                },
                ..TaskConfig::default()
            },
            None,
        )
        .unwrap();
        if phase != "start" {
            deleted(&server, 1).await;
        }
        if phase == "submit" {
            Mock::given(path("/ping"))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(path("/submit"))
                .respond_with(ResponseTemplate::new(502))
                .expect(1)
                .mount(&server)
                .await;
        }
        let error = target.submit(&json!({})).await.unwrap_err();
        match phase {
            "start" => assert!(error.to_string().contains("invalid image")),
            "ready" => assert!(error.to_string().contains("did not become ready")),
            _ => assert_eq!(error.status_code(), Some(502)),
        }
    }
}

#[tokio::test]
async fn task_start_resolves_registry_and_attaches_existing_network_storage() {
    let server = MockServer::start().await;
    Mock::given(path("/v2/catalog/cpus/cpu3c"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"dataCenters":[{"id":"EU-RO-1","availability":"HIGH"}]})),
        )
        .expect(runpod::apps::DataCenter::all().len() as u64)
        .mount(&server)
        .await;
    Mock::given(path("/v2/registries"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"registries":[{"id":"auth","name":"private"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/network-volumes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"networkVolumes":[{"id":"volume","name":"shared","dataCenter":"EU-RO-1"}],"pagination":{"hasNextPage":false}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"deployCpuPod":{"id":"pod"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    let mut spec:ResourceSpec=serde_json::from_value(json!({"kind":"task","name":"calc","cpu":"cpu3c-2-4","image":"private:1","registry_auth":"private"})).unwrap();
    spec.mounts = vec![runpod::apps::Mount {
        path: "/data".into(),
        volume: runpod::apps::NetworkVolume::new("shared").unwrap().into(),
    }];
    let task = TaskExecution::new(spec, AppsApiClient::new(client).unwrap()).unwrap();
    task.start().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let request = requests
        .iter()
        .find(|r| r.url.path() == "/graphql")
        .unwrap()
        .body_json::<Value>()
        .unwrap();
    let input = &request["variables"]["input"];
    assert_eq!(input["containerRegistryAuthId"], "auth");
    assert_eq!(input["dataCenterIds"], json!(["EU-RO-1"]));
    assert_eq!(
        input["volumeMounts"],
        json!([{"volumeId":"volume","volumeType":"NETWORK_VOLUME","mountPath":"/data"}])
    );
    assert!(
        input["env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["key"] == "RUNPOD_MOUNTS")
    );
    deleted(&server, 1).await;
    task.terminate().await.unwrap();
}

#[tokio::test]
async fn shared_volume_placement_accounts_for_siblings_and_ignores_unrelated_resources() {
    use runpod::apps::{Mount, NetworkVolume, ResourceKind, Selection};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/network-volumes"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"networkVolumes":[],"pagination":{"hasNextPage":false}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v2/catalog/datacenters")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"dataCenters":[{"id":"EU-RO-1","networkVolumeTypes":["STANDARD"]},{"id":"US-IL-1","networkVolumeTypes":["STANDARD"]}]}))).expect(1).mount(&server).await;
    Mock::given(path("/v2/catalog/cpus/cpu3c")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"dataCenters":[{"id":"EU-RO-1","availability":"LOW"},{"id":"US-IL-1","availability":"HIGH"}]}))).expect((2*runpod::apps::DataCenter::all().len()) as u64).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/v2/network-volumes"))
        .and(wiremock::matchers::body_json(
            json!({"name":"shared","size":50,"dataCenter":"EU-RO-1"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"volume"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"deployCpuPod":{"id":"pod"}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut spec = ResourceSpec::new(ResourceKind::Task, "task");
    spec.cpu = Some("cpu3c-2-4".into());
    spec.datacenter = Some(Selection::Many(vec!["US-IL-1".into(), "EU-RO-1".into()]));
    spec.mounts = vec![Mount {
        path: "/data".into(),
        volume: NetworkVolume::new("shared").unwrap().into(),
    }];
    let mut sibling = spec.clone();
    sibling.kind = ResourceKind::Queue;
    sibling.name = "sibling".into();
    sibling.mounts[0].path = "/runpod-volume".into();
    sibling.datacenter = Some("EU-RO-1".into());
    let mut unrelated = sibling.clone();
    unrelated.name = "unrelated".into();
    unrelated.datacenter = Some("US-IL-1".into());
    unrelated.mounts[0].volume = NetworkVolume::new("other").unwrap().into();
    let siblings = vec![spec.clone(), sibling, unrelated];
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    let execution = TaskExecution::with_config(
        spec,
        AppsApiClient::new(client).unwrap(),
        TaskConfig::default(),
        Some(siblings),
    )
    .unwrap();
    execution.start().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    let input = requests
        .iter()
        .find(|r| r.url.path() == "/graphql")
        .unwrap()
        .body_json::<Value>()
        .unwrap();
    assert_eq!(
        input["variables"]["input"]["dataCenterIds"],
        json!(["EU-RO-1"])
    );
    deleted(&server, 1).await;
    execution.terminate().await.unwrap();
}

#[test]
fn runtime_image_channel_environment_is_captured_without_global_mutation() {
    const EXPECTED: &str = "RUNPOD_RUST_TEST_EXPECTED_CHANNEL";
    if let Ok(expected) = std::env::var(EXPECTED) {
        assert_eq!(runpod::apps::images::runtime_tag(), expected);
        assert_eq!(
            RuntimeImages::default()
                .runtime_image(
                    runpod::apps::ResourceKind::Task,
                    runpod::apps::volume::ComputeType::Cpu
                )
                .unwrap(),
            format!("runpod/task:py3.12-{expected}")
        );
        return;
    }
    for tag in [None, Some("dev"), Some("")] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "runtime_image_channel_environment_is_captured_without_global_mutation",
            ])
            .env(EXPECTED, tag.unwrap_or("latest"))
            .env_remove("RUNPOD_RUNTIME_TAG");
        if let Some(tag) = tag {
            child.env("RUNPOD_RUNTIME_TAG", tag);
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

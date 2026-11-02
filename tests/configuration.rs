use runpod::{
    Client, ClientConfig, Value,
    configuration::{
        self, ContainerOptions, EndpointOptions, EndpointType, Flashboot, ScaleOptions,
    },
    json,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}
fn container(value: &Value) -> ContainerOptions {
    ContainerOptions {
        image: value["image"].as_str().map(str::to_owned),
        template: value["template"].as_str().map(str::to_owned),
        args: value["args"].as_str().map(str::to_owned),
        disk: value["disk"].as_u64(),
        global_networking: value["global_networking"].as_bool(),
        cmd: value["cmd"].as_str().map(str::to_owned),
        entrypoint: value["entrypoint"].as_str().map(str::to_owned),
        ports: strings(&value["ports"]),
        registry: value["registry"].as_str().map(str::to_owned),
        env: strings(&value["env"]),
    }
}
fn scale(value: &Value) -> ScaleOptions {
    ScaleOptions {
        min_workers: value["min_workers"].as_u64(),
        max_workers: value["max_workers"].as_u64(),
        idle_timeout_seconds: value["idle_timeout_seconds"].as_u64(),
        queue_delay_seconds: value["queue_delay_seconds"].as_f64(),
        requests_per_worker: value["requests_per_worker"].as_u64(),
    }
}
fn options(value: &Value) -> EndpointOptions {
    let mut container = container(value);
    container.ports = strings(&value["port"]);
    EndpointOptions {
        container,
        name: value["name"].as_str().map(str::to_owned),
        flashboot: value["flashboot"].as_str().map(|s| match s {
            "off" => Flashboot::Off,
            "priority" => Flashboot::Priority,
            _ => Flashboot::Standard,
        }),
        request_timeout_seconds: value["request_timeout_seconds"].as_u64(),
        gpu: strings(&value["gpu"]),
        gpu_count: value["gpu_count"].as_u64(),
        cuda: strings(&value["cuda"]),
        cpu: strings(&value["cpu"]),
        vcpu_count: value["vcpu_count"].as_u64(),
        scale: scale(value),
        data_centers: strings(&value["data_center"]),
        regions: strings(&value["region"]),
        network_volumes: strings(&value["network_volume"]),
        global_volumes: strings(&value["global_volume"]),
        unset_env: strings(&value["unset_env"]),
    }
}
async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/endpoint_config.json")).unwrap();
    assert!(cases.iter().any(|c| c["name"] == name));
    for case in cases.iter().filter(|c| c["name"] == name) {
        let server = MockServer::start().await;
        for request in case["requests"].as_array().unwrap() {
            Mock::given(method("GET"))
                .and(path(request["path"].as_str().unwrap()))
                .and(header("authorization", "Bearer fixture-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&request["response"]))
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("fixture-key")
        })
        .unwrap();
        let args = case["args"].as_array().unwrap();
        let current = args.get(1).filter(|v| !v.is_null());
        let result = match case["operation"].as_str().unwrap() {
            "parse_env" => configuration::parse_env(strings(&args[0]).iter().map(String::as_str))
                .map(Value::Object),
            "container_fields" => {
                client
                    .container_fields(
                        &container(&args[0]),
                        current,
                        &strings(args.get(2).unwrap_or(&Value::Null)),
                    )
                    .await
            }
            "config_body" => match current {
                Some(current) => {
                    client
                        .update_endpoint_config(&options(&args[0]), current)
                        .await
                }
                None => {
                    client
                        .create_endpoint_config(
                            &options(&args[0]),
                            if args[0]["endpoint_type"] == "LOAD_BALANCER" {
                                EndpointType::LoadBalancer
                            } else {
                                EndpointType::Queue
                            },
                        )
                        .await
                }
            },
            "scale_patch" => configuration::scale_patch(&scale(&args[0]), &args[1]),
            "mutable" => configuration::mutable_endpoint(&args[0]),
            "validate" => {
                configuration::validate_endpoint_document(args[0].as_str().unwrap(), &args[1])
            }
            "patch" => configuration::endpoint_patch(&args[0], &args[1]),
            other => panic!("unknown operation {other}"),
        };
        if let Some(error) = case.get("error") {
            let actual = result.unwrap_err().to_string();
            if error.as_str().unwrap().starts_with("invalid JSON:") {
                assert!(actual.starts_with("invalid JSON:"));
            } else {
                assert_eq!(actual, error.as_str().unwrap(), "{case}");
            }
        } else {
            assert_eq!(result.unwrap(), case["result"], "{case}");
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), case["requests"].as_array().unwrap().len());
        for (actual, expected) in requests.iter().zip(case["requests"].as_array().unwrap()) {
            assert_eq!(actual.url.path(), expected["path"].as_str().unwrap());
            let params: serde_json::Map<String, Value> = actual
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), json!(v)))
                .collect();
            assert_eq!(Value::Object(params), expected["params"]);
        }
    }
}
macro_rules! source_test { ($($name:ident),* $(,)?) => {$( #[tokio::test] async fn $name() { verify(stringify!($name)).await; } )*}; }
source_test!(
    test_env_values_preserve_equals_and_allow_empty_values,
    test_env_error_does_not_echo_the_value,
    test_container_fields_preserve_omitted_values,
    test_container_fields_map_options_and_parse_commands,
    test_container_fields_reject_malformed_shell_quoting,
    test_container_fields_resolve_registry_id_or_name,
    test_container_fields_clear_registry_without_lookup,
    test_container_fields_reject_unknown_registry,
    test_container_fields_merge_then_unset_environment,
    test_container_fields_allow_empty_current_environment,
    test_mutable_document_holds_update_fields_only,
    test_partial_gpu_edit_preserves_omitted_exclusions,
    test_edit_clears_cuda_floor_with_empty_string,
    test_edit_validation,
    test_edit_rejects_switching_cpu_to_gpu,
    command_quoting_edges,
    valid_edit_documents,
    patch_edges,
    configuration_create,
    configuration_create_errors,
    configuration_update,
    scale_changes
);

#[test]
fn configuration_enums_use_cli_spellings() {
    for (text, value) in [
        ("queue", EndpointType::Queue),
        ("load-balancer", EndpointType::LoadBalancer),
    ] {
        assert_eq!(text.parse::<EndpointType>().unwrap(), value);
        assert_eq!(value.to_string(), text);
    }
    for (text, value) in [
        ("off", Flashboot::Off),
        ("standard", Flashboot::Standard),
        ("priority", Flashboot::Priority),
    ] {
        assert_eq!(text.parse::<Flashboot>().unwrap(), value);
        assert_eq!(value.to_string(), text);
    }
    assert!("unknown".parse::<EndpointType>().is_err());
    assert!("PRIORITY".parse::<Flashboot>().is_ok());
    assert!("on".parse::<Flashboot>().is_err());
}

#[tokio::test]
async fn blocking_configuration_resolves_registry_and_preserves_merges() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/registries"))
        .and(header("authorization", "Bearer fixture-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "registries":[{"id":"reg1", "name":"private"}], "pagination":{"hasNextPage":false}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let url = server.uri();
    let worker = std::thread::spawn(move || {
        let client = runpod::blocking::Client::with_config(ClientConfig {
            api_base_url: url,
            ..ClientConfig::new("fixture-key")
        })
        .unwrap();
        let fields = client
            .container_fields(
                &ContainerOptions {
                    registry: Some("private".into()),
                    env: vec!["SET=new".into()],
                    ..ContainerOptions::default()
                },
                Some(&json!({"env":{"KEEP":"old", "DROP":"old"}})),
                &["DROP".into()],
            )
            .unwrap();
        assert_eq!(
            fields,
            json!({"registry":"reg1", "env":{"KEEP":"old", "SET":"new"}})
        );
        let options = EndpointOptions {
            name: Some("test".into()),
            ..EndpointOptions::default()
        };
        let created = client
            .create_endpoint_config(&options, EndpointType::Queue)
            .unwrap();
        assert_eq!(
            created,
            json!({"name":"test", "type":"QUEUE", "scaling":{"type":"QUEUE_DELAY", "queueDelay":4}})
        );
        assert_eq!(
            client
                .update_endpoint_config(&options, &json!({"type":"QUEUE"}))
                .unwrap(),
            json!({"name":"test"})
        );
    });
    tokio::task::spawn_blocking(move || worker.join().unwrap())
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn numeric_limits_fail_without_requests_or_panics() {
    let server = MockServer::start().await;
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("fixture-key")
    })
    .unwrap();
    let options = EndpointOptions {
        request_timeout_seconds: Some(u64::MAX),
        ..EndpointOptions::default()
    };
    assert!(matches!(
        client
            .create_endpoint_config(&options, EndpointType::Queue)
            .await,
        Err(runpod::Error::InvalidInput(_))
    ));
    let options = EndpointOptions {
        request_timeout_seconds: Some(u64::MAX / 1000),
        ..EndpointOptions::default()
    };
    assert_eq!(
        client
            .create_endpoint_config(&options, EndpointType::Queue)
            .await
            .unwrap()["timeout"],
        json!(u64::MAX / 1000 * 1000)
    );
    for delay in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            configuration::scale_patch(
                &ScaleOptions {
                    queue_delay_seconds: Some(delay),
                    ..ScaleOptions::default()
                },
                &json!({})
            ),
            Err(runpod::Error::InvalidInput(_))
        ));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn malformed_current_documents_return_typed_errors() {
    let client = Client::new("fixture-key").unwrap();
    for endpoint in [
        json!([]),
        json!({"cpu":[{}]}),
        json!({"mounts":{"network":[{}]}}),
        json!({"gpu":"invalid"}),
    ] {
        assert!(
            matches!(
                configuration::mutable_endpoint(&endpoint),
                Err(runpod::Error::InvalidResponse(_))
            ),
            "{endpoint}"
        );
    }
    assert!(matches!(
        client
            .update_endpoint_config(&EndpointOptions::default(), &json!([]))
            .await,
        Err(runpod::Error::InvalidResponse(_))
    ));
    assert!(matches!(
        configuration::scale_patch(
            &ScaleOptions {
                max_workers: Some(1),
                ..ScaleOptions::default()
            },
            &json!({"workers":true})
        ),
        Err(runpod::Error::InvalidResponse(_))
    ));
    assert!(matches!(
        configuration::endpoint_patch(&json!({}), &json!({"gpu":true})),
        Err(runpod::Error::InvalidResponse(_))
    ));
}

use runpod::{
    Value,
    apps::{
        ResourceSpec,
        deploy::{AppDefinition, DeploymentConfig, build_manifest, deployed_endpoint_input},
    },
    json,
};

#[test]
fn python_deployment_manifest_and_endpoint_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/deploy.json")).unwrap();
    for case in cases {
        let input = &case["input"];
        let output = match case["operation"].as_str().unwrap() {
            "manifest" => {
                let app: AppDefinition = serde_json::from_value(input["app"].clone()).unwrap();
                build_manifest(
                    &app,
                    input["python_version"].as_str().unwrap(),
                    &serde_json::from_value::<Vec<String>>(input["excluded_packages"].clone())
                        .unwrap(),
                )
            }
            "endpoint" => {
                let app = AppDefinition::new(
                    input["app_name"].as_str().unwrap(),
                    input["app_environment"].as_str().unwrap(),
                )
                .unwrap();
                let spec: ResourceSpec = serde_json::from_value(input["spec"].clone()).unwrap();
                let mut config = DeploymentConfig::new(input["api_key"].as_str().unwrap());
                config.python_version = input["python_version"].as_str().unwrap().into();
                config.runtime_overrides =
                    serde_json::from_value(input["runtime_overrides"].clone()).unwrap();
                let members: Option<runpod::apps::gpu::PoolMembers> =
                    serde_json::from_value(input["pool_members"].clone()).unwrap();
                deployed_endpoint_input(
                    &app,
                    &spec,
                    input["environment_id"].as_str().unwrap(),
                    input["build_id"].as_str().unwrap(),
                    input["environment_name"].as_str(),
                    &config,
                    members.as_ref(),
                )
            }
            other => panic!("unknown operation {other}"),
        };
        match case.get("error") {
            Some(error) => assert_eq!(
                output.unwrap_err().to_string(),
                error.as_str().unwrap(),
                "{case}"
            ),
            None => assert_eq!(output.unwrap(), case["output"], "{case}"),
        }
    }
}

use runpod::{
    Client, ClientConfig,
    apps::{
        AppsApiClient, ResourceKind,
        deploy::{ResourceDefinition, reconcile_endpoints},
    },
};
use std::sync::{Arc, Mutex};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn client(server: &MockServer) -> (AppsApiClient, DeploymentConfig) {
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("api-key")
    })
    .unwrap();
    let mut config = DeploymentConfig::from_client(&client);
    config.runtime_overrides.clear();
    (AppsApiClient::new(client).unwrap(), config)
}
fn resource(kind: ResourceKind, name: &str) -> ResourceDefinition {
    ResourceDefinition {
        spec: ResourceSpec::new(kind, name),
        symbol: None,
    }
}

#[tokio::test]
async fn reconciliation_saves_in_name_order_updates_existing_and_then_deletes_removed_resources() {
    let server = MockServer::start().await;
    let operations = Arc::new(Mutex::new(Vec::<Value>::new()));
    let record = operations.clone();
    Mock::given(path("/graphql"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let input = body["variables"]["input"].clone();
            let name = input["name"].as_str().unwrap();
            let id = input
                .get("id")
                .cloned()
                .unwrap_or_else(|| json!(format!("new-{name}")));
            record.lock().unwrap().push(input);
            ResponseTemplate::new(200).set_body_json(json!({"data":{"saveEndpoint":{"id":id}}}))
        })
        .expect(2)
        .mount(&server)
        .await;
    for id in ["removed-id", "old-task-id"] {
        let record = operations.clone();
        Mock::given(method("DELETE"))
            .and(path(format!("/v2/serverless/{id}")))
            .respond_with(move |_: &wiremock::Request| {
                record.lock().unwrap().push(json!({"delete":id}));
                ResponseTemplate::new(204)
            })
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/v2/registries"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"registries":[{"name":"private","id":"registry-id"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (api, config) = client(&server);
    let mut app = AppDefinition::new("demo", "staging").unwrap();
    let mut zeta = resource(ResourceKind::Queue, "zeta");
    zeta.spec.cpu = Some("cpu3c-2-4".into());
    zeta.spec.registry_auth = Some("private".into());
    let mut alpha = resource(ResourceKind::Api, "alpha");
    alpha.spec.model = Some("org/model:revision".into());
    app.resources = vec![zeta, resource(ResourceKind::Task, "task"), alpha];
    let endpoints=reconcile_endpoints(&api,&app,&json!({"id":"environment-id","name":"production","endpoints":[{"name":"removed","id":"stale-removed-id"},{"name":"zeta","id":"stale-zeta"},{"name":"task","id":"old-task-id"},{"name":"zeta","id":"old-zeta"},{"name":"removed","id":"removed-id"}]}),"build-id",&config).await.unwrap();
    assert_eq!(
        json!(endpoints),
        json!({"alpha":"new-alpha","zeta":"old-zeta"})
    );
    let operations = operations.lock().unwrap();
    assert_eq!(
        operations
            .iter()
            .map(|op| op.get("name").unwrap_or(&op["delete"]).clone())
            .collect::<Vec<_>>(),
        vec![
            json!("alpha"),
            json!("zeta"),
            json!("removed-id"),
            json!("old-task-id")
        ]
    );
    assert_eq!(
        operations[0]["modelReferences"],
        json!(["org/model:revision"])
    );
    assert_eq!(
        operations[1]["template"]["containerRegistryAuthId"],
        "registry-id"
    );
    assert_eq!(operations[1]["id"], "old-zeta");
    for payload in &operations[..2] {
        assert_eq!(payload["flashEnvironmentId"], "environment-id");
        assert_eq!(payload["networkVolumeIds"], json!([]));
        assert_eq!(payload["volumes"], json!([]));
        let env = payload["template"]["env"].as_array().unwrap();
        assert!(
            env.iter()
                .any(|e| e["key"] == "FLASH_ENVIRONMENT" && e["value"] == "production")
        );
        assert!(
            env.iter()
                .any(|e| e["key"] == "RUNPOD_API_KEY" && e["value"] == "api-key")
        );
    }
    assert!(!format!("{config:?}").contains("api-key"));
}

#[tokio::test]
async fn validation_and_save_failures_prevent_obsolete_endpoint_deletion() {
    for phase in ["validate", "save", "missing-id"] {
        let server = MockServer::start().await;
        let (api, config) = client(&server);
        let mut app = AppDefinition::new("demo", "default").unwrap();
        app.resources = vec![
            resource(ResourceKind::Queue, "alpha"),
            resource(ResourceKind::Queue, "zeta"),
        ];
        if phase == "validate" {
            app.resources[1].spec.gpu_count = 0;
        } else {
            Mock::given(path("/graphql"))
                .respond_with(move |request: &wiremock::Request| {
                    let request: Value = request.body_json().unwrap();
                    ResponseTemplate::new(200).set_body_json(
                        if request["variables"]["input"]["name"] == "alpha" {
                            json!({"data":{"saveEndpoint":{"id":"new-alpha"}}})
                        } else if phase == "save" {
                            json!({"errors":[{"message":"save rejected"}]})
                        } else {
                            json!({"data":{"saveEndpoint":{}}})
                        },
                    )
                })
                .expect(2)
                .mount(&server)
                .await;
        }
        assert!(
            reconcile_endpoints(
                &api,
                &app,
                &json!({"id":"env","name":"default","endpoints":[{"name":"obsolete","id":"old"}]}),
                "build",
                &config
            )
            .await
            .is_err()
        );
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|request| request.method == "POST"));
        if phase == "validate" {
            assert!(requests.is_empty());
        }
    }
}

#[tokio::test]
async fn exact_gpu_pins_use_live_membership_and_task_only_apps_remove_old_endpoints() {
    let server = MockServer::start().await;
    Mock::given(path("/v2/catalog/gpus")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"gpus":[{"id":"NVIDIA H100 PCIe","pool":"ADA_80_PRO"},{"id":"Future H100","pool":"ADA_80_PRO"}]}))).expect(1).mount(&server).await;
    Mock::given(path("/graphql"))
        .respond_with(|request: &wiremock::Request| {
            let input: Value = request.body_json().unwrap();
            assert_eq!(
                input["variables"]["input"]["gpuIds"],
                "ADA_80_PRO,-Future H100"
            );
            ResponseTemplate::new(200).set_body_json(json!({"data":{"saveEndpoint":{"id":"new"}}}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let (api, config) = client(&server);
    let mut app = AppDefinition::new("demo", "default").unwrap();
    let mut queue = resource(ResourceKind::Queue, "queue");
    queue.spec.gpu = Some("NVIDIA H100 PCIe".into());
    app.resources = vec![queue];
    reconcile_endpoints(
        &api,
        &app,
        &json!({"id":"env","name":"default"}),
        "build",
        &config,
    )
    .await
    .unwrap();
    app.resources = vec![resource(ResourceKind::Task, "task")];
    Mock::given(method("DELETE"))
        .and(path("/v2/serverless/new"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        reconcile_endpoints(
            &api,
            &app,
            &json!({"id":"env","name":"default","endpoints":[{"name":"queue","id":"new"}]}),
            "build",
            &config
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn reconciliation_reuses_volume_resolution_and_pins_both_endpoints() {
    use runpod::apps::{Mount, NetworkVolume};
    let server = MockServer::start().await;
    Mock::given(path("/v2/network-volumes")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"networkVolumes":[{"id":"volume","name":"shared","dataCenter":"EU-RO-1"}],"pagination":{"hasNextPage":false}}))).expect(1).mount(&server).await;
    Mock::given(path("/v2/catalog/cpus/cpu3c"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"dataCenters":[{"id":"EU-RO-1","availability":"HIGH"}]})),
        )
        .expect(runpod::apps::DataCenter::all().len() as u64)
        .mount(&server)
        .await;
    Mock::given(path("/graphql"))
        .respond_with(|request: &wiremock::Request| {
            let input: Value = request.body_json().unwrap();
            let input = &input["variables"]["input"];
            assert_eq!(input["locations"], "EU-RO-1");
            assert_eq!(
                input["networkVolumeIds"],
                json!([{"networkVolumeId":"volume"}])
            );
            assert!(
                input["template"]["env"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["key"] == "RUNPOD_MOUNTS")
            );
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"saveEndpoint":{"id":input["name"]}}}))
        })
        .expect(2)
        .mount(&server)
        .await;
    let (api, config) = client(&server);
    let mut app = AppDefinition::new("demo", "default").unwrap();
    for name in ["alpha", "zeta"] {
        let mut definition = resource(ResourceKind::Queue, name);
        definition.spec.cpu = Some("cpu3c-2-4".into());
        definition.spec.mounts = vec![Mount {
            path: "/runpod-volume".into(),
            volume: NetworkVolume::new("shared").unwrap().into(),
        }];
        app.resources.push(definition);
    }
    let endpoints = reconcile_endpoints(
        &api,
        &app,
        &json!({"id":"env","name":"default"}),
        "build",
        &config,
    )
    .await
    .unwrap();
    assert_eq!(endpoints.len(), 2);
}

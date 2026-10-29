//! Differential wire contracts captured from the pinned Python implementation.
use runpod::{
    Client, ClientConfig, Value,
    control::{EndpointOptions, PodOptions, TemplateOptions},
    json,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path, query_param},
};

#[tokio::test]
async fn python_control_plane_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/control.json")).unwrap();
    for case in cases {
        let server = MockServer::start().await;
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap();
        for request in case["requests"].as_array().unwrap() {
            let mut mock = Mock::given(method(request["method"].as_str().unwrap()))
                .and(path(request["path"].as_str().unwrap()));
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
            mock.respond_with(
                ResponseTemplate::new(request["status"].as_u64().unwrap() as u16)
                    .set_body_json(&request["response"]),
            )
            .expect(1)
            .mount(&server)
            .await;
        }
        let k = &case["kwargs"];
        let s = |key: &str| k[key].as_str().unwrap_or("");
        let result = match case["operation"].as_str().unwrap() {
            "get_user" => client.get_user().await,
            "update_user_settings" => client.update_user_settings(s("pubkey")).await,
            "get_gpus" => client.get_gpus().await.map(|v| json!(v)),
            "get_gpu" => {
                client
                    .get_gpu(s("gpu_id"), k["gpu_quantity"].as_u64().unwrap_or(1))
                    .await
            }
            "get_pods" => client.get_pods().await.map(|v| json!(v)),
            "get_pod" => client
                .get_pod(s("pod_id"))
                .await
                .map(|v| v.unwrap_or(Value::Null)),
            "create_pod" => {
                client
                    .create_pod(&serde_json::from_value::<PodOptions>(k.clone()).unwrap())
                    .await
            }
            "stop_pod" => client.stop_pod(s("pod_id")).await,
            "resume_pod" => {
                client
                    .resume_pod(s("pod_id"), k["gpu_count"].as_u64())
                    .await
            }
            "terminate_pod" => client
                .terminate_pod(s("pod_id"))
                .await
                .map(|()| Value::Null),
            "get_endpoints" => client.get_endpoints().await.map(|v| json!(v)),
            "create_template" => {
                client
                    .create_template(&serde_json::from_value::<TemplateOptions>(k.clone()).unwrap())
                    .await
            }
            "create_endpoint" => {
                client
                    .create_endpoint(&serde_json::from_value::<EndpointOptions>(k.clone()).unwrap())
                    .await
            }
            "update_endpoint_template" => {
                client
                    .update_endpoint_template(s("endpoint_id"), s("template_id"))
                    .await
            }
            "create_container_registry_auth" => {
                client
                    .create_container_registry_auth(s("name"), s("username"), s("password"))
                    .await
            }
            "update_container_registry_auth" => {
                client
                    .update_container_registry_auth(
                        s("registry_auth_id"),
                        s("username"),
                        s("password"),
                    )
                    .await
            }
            "delete_container_registry_auth" => client
                .delete_container_registry_auth(s("registry_auth_id"))
                .await
                .map(|v| json!(v)),
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
            assert_eq!(result.unwrap(), case["output"], "{}", case["name"]);
        }
        server.verify().await;
    }
}

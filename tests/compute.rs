use runpod::{Client, ClientConfig, Value, compute, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/compute.json")).unwrap()
}
fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}
fn verify(operation: &str, errors: bool) {
    verify_cases(operation, errors, |_| true);
}
fn verify_cases(operation: &str, errors: bool, include: impl Fn(&Value) -> bool) {
    for case in cases()
        .iter()
        .filter(|c| c["operation"] == operation && c.get("error").is_some() == errors && include(c))
    {
        let a = case["args"].as_array().unwrap();
        let result = match operation {
            "match_gpus" => compute::match_gpus(a[0].as_str().unwrap(), a[1].as_array().unwrap())
                .map(|v| json!(v)),
            "resolve_gpus" => {
                compute::resolve_gpus(a[0].as_str().unwrap(), a[1].as_array().unwrap())
                    .map(|v| json!(v))
            }
            "parse_gpu_spec" => compute::parse_gpu_spec(a[0].as_str().unwrap()).map(|v| json!(v)),
            "parse_cuda" => compute::parse_cuda(strings(&a[0])).map(|v| json!(v)),
            "endpoint_gpu_selection" => compute::endpoint_gpu_selection(
                a[0].as_array().unwrap().iter(),
                a[1].as_array().unwrap(),
            )
            .map(|v| json!(v)),
            "resolve_regions" => {
                compute::resolve_regions(strings(&a[0]), a[1].as_array().unwrap()).map(|v| json!(v))
            }
            "match_cpus" => compute::match_cpus(a[0].as_str().unwrap(), a[1].as_array().unwrap())
                .map(|v| json!(v)),
            "resolve_cpu" => compute::resolve_cpu(a[0].as_str().unwrap(), a[1].as_array().unwrap())
                .map(|v| json!(v)),
            "availability_rank" => Ok(json!(compute::availability_rank(a[0].as_str()))),
            "best_availability" => Ok(json!(compute::best_availability(
                a[0].as_array().unwrap().iter().map(Value::as_str)
            ))),
            other => panic!("unsupported operation {other}"),
        };
        if errors {
            assert_eq!(
                result.unwrap_err().to_string(),
                case["error"].as_str().unwrap(),
                "{case}"
            );
        } else {
            assert_eq!(result.unwrap(), case["output"], "{case}");
        }
    }
}
#[test]
fn test_queries() {
    verify("match_gpus", false);
}
#[test]
fn test_resolve_raises_with_known_names() {
    verify("resolve_gpus", true);
    verify("resolve_gpus", false);
}
#[test]
fn test_gpu_spec() {
    verify("parse_gpu_spec", false);
}
#[test]
fn test_invalid_gpu_spec() {
    verify("parse_gpu_spec", true);
}
#[test]
fn test_cuda() {
    verify("parse_cuda", false);
}
#[test]
fn test_invalid_cuda() {
    verify("parse_cuda", true);
}
#[test]
fn test_excludes_unselected_pool_members() {
    verify_cases("endpoint_gpu_selection", false, |c| {
        !c["output"][1].as_array().unwrap().is_empty()
    });
}
#[test]
fn test_whole_pool() {
    verify_cases("endpoint_gpu_selection", false, |c| {
        c["output"][1].as_array().unwrap().is_empty()
    });
}
#[test]
fn test_not_on_serverless() {
    verify("endpoint_gpu_selection", true);
}
#[test]
fn test_resolution() {
    verify("resolve_regions", false);
}
#[test]
fn test_unknown_region_lists_options() {
    verify("resolve_regions", true);
}
#[test]
fn test_match() {
    verify("match_cpus", false);
}
#[test]
fn test_resolve() {
    verify("resolve_cpu", false);
    verify("resolve_cpu", true);
}
#[test]
fn test_best() {
    verify("availability_rank", false);
    verify("best_availability", false);
}

async fn requests(operation: &str, has_product: Option<bool>) {
    for case in cases().into_iter().filter(|c| {
        c["operation"] == operation
            && has_product.is_none_or(|has| {
                c["kwargs"]["product"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
                    == has
            })
    }) {
        let server = MockServer::start().await;
        let request = &case["request"];
        Mock::given(method("GET"))
            .and(path(request["path"].as_str().unwrap()))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"gpus":case["output"],"cpus":case["output"],"dataCenters":case["output"]}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap();
        let k = &case["kwargs"];
        let actual = match operation {
            "list_gpus" => client
                .list_catalog_gpus(&compute::GpuCatalogOptions {
                    product: k["product"].as_str().map(str::to_owned),
                    count: k["count"].as_u64(),
                    cloud: k["cloud"].as_str().map(str::to_owned),
                    cuda_versions: k
                        .get("cuda_versions")
                        .map(strings)
                        .unwrap_or_default()
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    min_cuda_version: k["min_cuda_version"].as_str().map(str::to_owned),
                })
                .await
                .unwrap(),
            "list_cpus" => client
                .list_catalog_cpus(k["product"].as_str(), k["vcpu_count"].as_u64())
                .await
                .unwrap(),
            "list_datacenters" => client
                .list_catalog_datacenters(
                    &case["args"][0]
                        .as_array()
                        .map(|_| strings(&case["args"][0]))
                        .unwrap_or_default(),
                )
                .await
                .unwrap(),
            other => panic!("unsupported operation {other}"),
        };
        assert_eq!(json!(actual), case["output"]);
        let received = server.received_requests().await.unwrap();
        let actual: std::collections::BTreeMap<_, _> = received[0]
            .url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let expected: std::collections::BTreeMap<_, _> = request["params"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str().map_or_else(|| v.to_string(), str::to_owned),
                )
            })
            .collect();
        assert_eq!(actual, expected, "{case}");
    }
}
#[tokio::test]
async fn test_list_gpus_with_availability() {
    requests("list_gpus", Some(true)).await;
}
#[tokio::test]
async fn test_list_gpus_without_availability() {
    requests("list_gpus", Some(false)).await;
}
#[tokio::test]
async fn test_list_cpus_and_datacenters() {
    requests("list_cpus", None).await;
    requests("list_datacenters", None).await;
}

#[test]
fn malformed_records_return_typed_errors() {
    for row in [
        json!({}),
        json!({"id":3}),
        json!({"id":"gpu","name":[]}),
        json!({"id":"gpu","pool":true}),
    ] {
        assert!(matches!(
            compute::match_gpus("gpu", &[row]),
            Err(runpod::Error::InvalidResponse(_))
        ));
    }
    assert!(matches!(
        compute::match_cpus("cpu", &[json!({"id":"cpu","group":3})]),
        Err(runpod::Error::InvalidResponse(_))
    ));
    assert!(matches!(
        compute::resolve_regions(["eu"], &[json!({"id":"dc","region":3})]),
        Err(runpod::Error::InvalidResponse(_))
    ));
    assert!(matches!(
        compute::resolve_gpus("unknown", &[]),
        Err(runpod::Error::Compute(_))
    ));
}

#[test]
fn numeric_boundaries_are_checked() {
    assert_eq!(
        compute::parse_gpu_spec("h100:18446744073709551615")
            .unwrap()
            .1,
        Some(u64::MAX)
    );
    for value in ["h100:18446744073709551616", "h100:-1", "h100:+2", "h100:٢"] {
        assert!(
            matches!(
                compute::parse_gpu_spec(value),
                Err(runpod::Error::Compute(_))
            ),
            "{value}"
        );
    }
    for value in ["18446744073709551616.4", "12.٤", "12.+4"] {
        assert!(
            matches!(compute::parse_cuda([value]), Err(runpod::Error::Compute(_))),
            "{value}"
        );
    }
}

#[tokio::test]
async fn catalog_errors_preserve_api_causes_and_reject_bad_shapes() {
    for body in [json!({}), json!({"gpus":null}), json!({"gpus":{}})] {
        let server = MockServer::start().await;
        Mock::given(path("/v2/catalog/gpus"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap();
        assert!(matches!(
            client
                .list_catalog_gpus(&compute::GpuCatalogOptions::default())
                .await,
            Err(runpod::Error::InvalidResponse(_))
        ));
    }
    let server = MockServer::start().await;
    Mock::given(path("/v2/catalog/gpus"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error":"denied"})))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("key")
    })
    .unwrap();
    assert_eq!(
        client
            .list_catalog_gpus(&compute::GpuCatalogOptions::default())
            .await
            .unwrap_err()
            .status_code(),
        Some(403)
    );
}

#[tokio::test]
async fn blocking_catalog_preserves_unknown_fields_and_authentication() {
    use wiremock::matchers::header;
    let server = MockServer::start().await;
    for (endpoint, key) in [
        ("gpus", "gpus"),
        ("cpus", "cpus"),
        ("datacenters", "dataCenters"),
    ] {
        Mock::given(path(format!("/v2/catalog/{endpoint}")))
            .and(header("Authorization", "Bearer key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({key:[{"id":"record","future":{"name":null}}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let url = server.uri();
    let worker = std::thread::spawn(move || {
        let client = runpod::blocking::Client::with_config(ClientConfig {
            api_base_url: url,
            ..ClientConfig::new("key")
        })
        .unwrap();
        for rows in [
            client
                .list_catalog_gpus(&compute::GpuCatalogOptions::default())
                .unwrap(),
            client.list_catalog_cpus(None, None).unwrap(),
            client.list_catalog_datacenters(&[]).unwrap(),
        ] {
            assert_eq!(json!(rows), json!([{"id":"record","future":{"name":null}}]));
        }
    });
    tokio::task::spawn_blocking(move || worker.join().unwrap())
        .await
        .unwrap();
}

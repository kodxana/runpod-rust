//! Differential coverage of candidates, storage constraints, and maximin placement.
use runpod::{
    Client, ClientConfig, Value,
    apps::{
        AppsApiClient, DataCenter, ResourceSpec,
        placement::{PlacementOptions, StockMap, candidates, hardware_keys, solve_placement},
    },
    json,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{path, query_param},
};

#[tokio::test]
async fn python_placement_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/placement.json")).unwrap();
    for case in cases {
        let server = MockServer::start().await;
        let specs: Vec<ResourceSpec> = serde_json::from_value(case["specs"].clone()).unwrap();
        let keys = specs
            .iter()
            .flat_map(|s| hardware_keys(s).unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        let stocks = case["stocks"].as_array().unwrap();
        let mut expected_requests = 0;
        for key in &keys {
            use runpod::apps::{api::Product, placement::StockKey};
            let (kind, hw, count, product) = match key {
                StockKey::Gpu {
                    id: Some(id),
                    count,
                    product,
                } => ("gpu", id, *count, *product),
                StockKey::Gpu { id: None, .. } => continue,
                StockKey::Cpu {
                    instance_id,
                    product,
                } => ("cpu", instance_id, 0, *product),
            };
            let stock_kind = format!(
                "{kind}{}",
                if product == Product::Pod { "-pod" } else { "" }
            );
            let dcs: Vec<Value> = stocks.iter().filter(|v|v[0]==stock_kind && v[1]==*hw && v[2]==count)
                .map(|v|json!({"id":v[3],"availability":match v[4].as_u64().unwrap() {3=>"HIGH",2=>"MEDIUM",1=>"LOW",_=>"NONE"}})).collect();
            let (catalog, id, count_param, count) = if kind == "cpu" {
                (
                    "cpus",
                    hw.split('-').next().unwrap().to_owned(),
                    "vcpuCount",
                    hw.split('-').nth(1).unwrap().to_owned(),
                )
            } else {
                (
                    "gpus",
                    percent_encoding::utf8_percent_encode(hw, percent_encoding::NON_ALPHANUMERIC)
                        .to_string(),
                    "count",
                    count.to_string(),
                )
            };
            Mock::given(path(format!("/v2/catalog/{catalog}/{id}")))
                .and(query_param(
                    "product",
                    if product == Product::Pod {
                        "POD"
                    } else {
                        "SERVERLESS"
                    },
                ))
                .and(query_param(count_param, count))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"dataCenters":dcs})))
                .expect(DataCenter::all().len() as u64)
                .mount(&server)
                .await;
            expected_requests += DataCenter::all().len();
        }
        let api = AppsApiClient::new(
            Client::with_config(ClientConfig {
                api_base_url: server.uri(),
                ..ClientConfig::new("key")
            })
            .unwrap(),
        )
        .unwrap();
        let mut stock = StockMap::new(api);
        let keys: Vec<_> = keys.into_iter().collect();
        stock.fetch(&keys).await;
        stock.fetch(&keys).await;
        for (index, spec) in specs.iter().enumerate() {
            assert_eq!(
                json!(candidates(spec, &stock).unwrap()),
                case["candidates"][index],
                "{}",
                case["name"]
            );
        }
        let options: PlacementOptions = serde_json::from_value(case["options"].clone()).unwrap();
        let result = solve_placement(&specs, &stock, &options);
        if let Some(error) = case.get("error") {
            assert_eq!(
                result.unwrap_err().to_string(),
                error.as_str().unwrap(),
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                result.unwrap(),
                case["output"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            expected_requests
        );
    }
}

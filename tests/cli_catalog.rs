use runpod::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

async fn verify(name: &str) {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/catalog.json")).unwrap();
    assert!(
        cases.iter().any(|case| case["name"] == name),
        "missing fixture group {name}"
    );
    for case in cases.iter().filter(|c| c["name"] == name) {
        let server = MockServer::start().await;
        for endpoint in ["gpus", "cpus", "datacenters"] {
            let route = format!("/v2/catalog/{endpoint}");
            let count = case["requests"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["path"] == route)
                .count();
            Mock::given(path(&route))
                .and(method("GET"))
                .and(header("authorization", "Bearer fixture-key"))
                .respond_with(if case["denied"] == true {
                    ResponseTemplate::new(401)
                        .set_body_json(json!({"error":"Unauthorized request"}))
                } else {
                    ResponseTemplate::new(200).set_body_json(&case["records"])
                })
                .expect(u64::try_from(count).unwrap())
                .mount(&server)
                .await;
        }
        let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"));
        process
            .arg("catalog")
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri());
        let output = process.output().await.unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            output.status.code(),
            Some(i32::try_from(case["code"].as_i64().unwrap()).unwrap()),
            "{case}\n{stdout}\n{stderr}"
        );
        if let Some(error) = case.get("error") {
            assert!(
                stderr.contains(error.as_str().unwrap()),
                "{}\n{stderr}",
                case["args"]
            );
            assert_eq!(stdout, "");
        } else if let Some(expected) = case.get("json") {
            assert_eq!(
                serde_json::from_str::<Value>(&stdout).unwrap(),
                *expected,
                "{}",
                case["args"]
            );
            assert_eq!(stderr, "");
            assert!(stdout.is_ascii(), "JSON should escape Unicode like Python");
        } else {
            assert_eq!(stderr, "");
            verify_table(&stdout, &case["table"]);
        }
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), case["requests"].as_array().unwrap().len());
        for (actual, expected) in received.iter().zip(case["requests"].as_array().unwrap()) {
            assert_eq!(actual.url.path(), expected["path"].as_str().unwrap());
            let actual: std::collections::BTreeMap<_, _> = actual
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let expected: std::collections::BTreeMap<_, _> = expected["params"]
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
            assert_eq!(actual, expected);
        }
    }
}
fn verify_table(stdout: &str, table: &Value) {
    if table["cells"].as_array().unwrap().is_empty() {
        assert_eq!(stdout.trim(), table["empty"].as_str().unwrap());
        return;
    }
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        table["cells"].as_array().unwrap().len() + 1,
        "{stdout}"
    );
    for value in table["headers"].as_array().unwrap() {
        assert!(lines[0].contains(value.as_str().unwrap()), "{stdout}");
    }
    for (line, cells) in lines.iter().skip(1).zip(table["cells"].as_array().unwrap()) {
        let mut remaining = *line;
        for cell in cells.as_array().unwrap() {
            let cell = cell.as_str().unwrap();
            let offset = remaining
                .find(cell)
                .unwrap_or_else(|| panic!("missing {cell:?} in {line:?}"));
            remaining = &remaining[offset + cell.len()..];
        }
    }
}
macro_rules! source_test { ($($name:ident),* $(,)?) => {$( #[tokio::test] async fn $name() { verify(stringify!($name)).await; } )*}; }
source_test!(
    test_max_count_matches_cloud,
    test_cpu_community_rejected_before_fetch,
    test_strongest_cuda_floor_is_sent,
    test_unavailable_gpu_omits_stock_fields,
    test_pod_table_sorted_by_stock_then_price,
    test_region_narrows_stock,
    test_cuda_and_cloud_filters,
    test_available_and_vram_filters,
    test_json_totals,
    test_serverless_shows_pools_only,
    test_unknown_gpu,
    test_empty,
    test_conflicting_flags,
    test_api_error_is_clean,
    test_table,
    test_query_region_and_available,
    test_unknown,
    test_per_vcpu_price_precision,
    test_inapplicable_flags_fail_before_fetch,
    test_all,
    test_query_is_a_region,
    additional_cli_edges
);

#[tokio::test]
async fn native_alias_help_and_parser_errors() {
    for binary in [env!("CARGO_BIN_EXE_rp"), env!("CARGO_BIN_EXE_runpod")] {
        for args in [vec!["--help"], vec!["catalog", "--help"]] {
            let result = tokio::process::Command::new(binary)
                .args(args)
                .env("RUNPOD_API_BASE_URL", "invalid URL")
                .output()
                .await
                .unwrap();
            assert!(result.status.success());
            assert_eq!(result.stderr, b"");
            assert!(
                String::from_utf8(result.stdout)
                    .unwrap()
                    .contains("catalog")
            );
        }
        for args in [
            vec!["catalog", "--count", "0"],
            vec!["catalog", "--vcpu", "1"],
            vec!["catalog", "--cloud", "invalid"],
            vec!["catalog", "--count", "4294967296"],
            vec!["catalog", "--count"],
            vec!["catalog", "--max-price", "bad"],
            vec!["catalog", "--cpu", "--count", "1"],
            vec!["unknown-command"],
            vec!["catalog", "--unknown"],
            vec!["catalog", "one", "two"],
        ] {
            let result = tokio::process::Command::new(binary)
                .args(&args)
                .env("RUNPOD_API_BASE_URL", "invalid URL")
                .output()
                .await
                .unwrap();
            assert_eq!(result.status.code(), Some(2), "{args:?}");
            assert_eq!(result.stdout, b"");
            assert_ne!(result.stderr, b"");
            assert!(
                !String::from_utf8(result.stderr)
                    .unwrap()
                    .contains("invalid URL")
            );
        }
        let result = tokio::process::Command::new(binary)
            .args(["catalog", "--cuda", "wrong"])
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", "invalid URL")
            .output()
            .await
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(result.stdout, b"");
        assert!(String::from_utf8(result.stderr).unwrap().contains("cuda"));
    }
    let server = MockServer::start().await;
    Mock::given(path("/v2/catalog/datacenters"))
        .and(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"dataCenters": []})))
        .expect(1)
        .mount(&server)
        .await;
    let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_runpod"))
        .args(["catalog", "--datacenters", "--json"])
        .env("RUNPOD_API_KEY", "fixture-key")
        .env("RUNPOD_API_BASE_URL", server.uri())
        .output()
        .await
        .unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"[]\n");
    assert_eq!(result.stderr, b"");
}

#[tokio::test]
async fn malformed_catalog_is_a_clean_error() {
    for data in [
        json!({}),
        json!({"gpus": ["wrong"]}),
        json!({"gpus": [{"name":"bad", "price": "wrong"}]}),
        json!({"gpus": [{"name":"bad", "price": {"secure": "wrong"}}]}),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/v2/catalog/gpus"))
            .and(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(data))
            .expect(1)
            .mount(&server)
            .await;
        let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(["catalog", "--json"])
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .output()
            .await
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(result.stdout, b"");
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.starts_with("Error: invalid response: catalog"),
            "{error}"
        );
        assert!(!error.contains("panicked"));
    }
}

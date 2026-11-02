use runpod::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

async fn verify(name: &str) {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/volume_cli.json")).unwrap();
    assert!(cases.iter().any(|case| case["name"] == name));
    for case in cases.iter().filter(|case| {
        case["name"] == name && case.get("selected").is_none() && case.get("answer").is_none()
    }) {
        let server = MockServer::start().await;
        for request in case["requests"].as_array().unwrap() {
            let mut mock = Mock::given(method(request["method"].as_str().unwrap()))
                .and(path(request["path"].as_str().unwrap()))
                .and(header("authorization", "Bearer fixture-key"));
            if let Some(cursor) = request["params"]["cursor"].as_str() {
                mock = mock.and(query_param("cursor", cursor));
            }
            mock.respond_with(
                ResponseTemplate::new(u16::try_from(request["status"].as_u64().unwrap()).unwrap())
                    .set_body_json(&request["response"]),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        }
        let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .arg("volume")
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a.as_str().unwrap()),
            )
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8(result.stdout).unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(
            result.status.code(),
            Some(i32::try_from(case["code"].as_i64().unwrap()).unwrap()),
            "{case}\n{stdout}\n{stderr}"
        );
        if let Some(error) = case.get("error") {
            // Parser usage layout is native; value/range errors still must reject before requests.
            if case["args"].as_array().unwrap().iter().any(|a| a == "5000") {
                assert!(stderr.contains("4096"), "{stderr}");
            } else {
                assert!(stderr.contains(error.as_str().unwrap()), "{case}\n{stderr}");
            }
            assert_eq!(stdout, "");
        } else {
            assert_eq!(stderr, "");
            if let Some(expected) = case.get("json") {
                assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), *expected);
            } else {
                verify_text(&stdout, case);
            }
        }
        let actual = server.received_requests().await.unwrap();
        assert_eq!(actual.len(), case["requests"].as_array().unwrap().len());
        for (actual, expected) in actual.iter().zip(case["requests"].as_array().unwrap()) {
            assert_eq!(actual.method.as_str(), expected["method"].as_str().unwrap());
            assert_eq!(actual.url.path(), expected["path"].as_str().unwrap());
            let query: serde_json::Map<String, Value> = actual
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), json!(v)))
                .collect();
            assert_eq!(Value::Object(query), expected["params"]);
            let body = if actual.body.is_empty() {
                Value::Null
            } else {
                actual.body_json().unwrap()
            };
            assert_eq!(body, expected["body"]);
        }
    }
}
fn verify_text(text: &str, case: &Value) {
    for message in case["success"].as_array().unwrap() {
        assert!(text.contains(message.as_str().unwrap()), "{text}");
    }
    if let Some(fields) = case.get("fields") {
        for field in fields.as_array().unwrap() {
            assert!(
                text.lines()
                    .any(|line| line.contains(field[0].as_str().unwrap())
                        && line.contains(field[1].as_str().unwrap())),
                "{text}"
            );
        }
    }
    if let Some(table) = case.get("table") {
        if table["cells"].as_array().unwrap().is_empty() {
            assert_eq!(text.trim(), table["empty"].as_str().unwrap());
        } else {
            for header in table["headers"].as_array().unwrap() {
                assert!(
                    text.lines()
                        .next()
                        .unwrap()
                        .contains(header.as_str().unwrap()),
                    "{text}"
                );
            }
            assert_eq!(
                text.lines().count(),
                table["cells"].as_array().unwrap().len() + 1
            );
            for (line, cells) in text.lines().skip(1).zip(table["cells"].as_array().unwrap()) {
                let mut rest = line;
                for cell in cells.as_array().unwrap() {
                    let cell = cell.as_str().unwrap();
                    let offset = rest
                        .find(cell)
                        .unwrap_or_else(|| panic!("{cell:?} missing in {line}"));
                    rest = &rest[offset + cell.len()..];
                }
            }
        }
    }
}
macro_rules! source_test { ($($name:ident),* $(,)?) => {$( #[tokio::test] async fn $name() { verify(stringify!($name)).await; } )*}; }
source_test!(
    test_list_table_and_json,
    test_list_empty,
    test_get_fields_and_json,
    test_missing_id_requires_terminal,
    test_api_errors_are_clean,
    test_create_exact_data_center,
    test_create_single_region_match_uses_default_tier,
    test_create_rejects_invalid_placement_and_size,
    test_create_requires_network_size,
    test_create_global,
    test_invalid_updates_and_global_options,
    test_rename_without_fetching,
    test_resize_requests,
    test_expand_human_output,
    test_resize_rejects_shrink_no_growth_and_overflow,
    test_expand_rejects_malformed_size_without_fetching,
    test_delete_yes,
    test_delete_requires_yes_without_terminal,
    additional_volume_edges
);

use runpod::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[tokio::test]
async fn snapshots_match_pinned_python() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/cli_logs.json")).unwrap();
    for case in cases {
        let server = MockServer::start().await;
        let args: Vec<&str> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let endpoint = args[0] == "endpoint";
        let segment = |id: &str| {
            percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC)
                .to_string()
        };
        if endpoint {
            let workers: Vec<Value> = case["workers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| json!({"id":id}))
                .collect();
            Mock::given(method("GET"))
                .and(path(format!("/v2/serverless/{}/workers", segment(args[2]))))
                .and(header("authorization", "Bearer fixture-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"workers":workers})))
                .expect(1)
                .mount(&server)
                .await;
        }
        for call in case["calls"].as_array().unwrap() {
            let worker = call["worker"].as_str().unwrap();
            let route = if endpoint {
                format!(
                    "/v2/serverless/{}/workers/{}/logs",
                    segment(args[2]),
                    segment(worker)
                )
            } else {
                format!("/v2/pods/{}/logs", segment(worker))
            };
            let body = case["events"][worker]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>();
            Mock::given(method("GET"))
                .and(path(route))
                .and(header("accept", "text/event-stream"))
                .and(header("authorization", "Bearer fixture-key"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
                .expect(1)
                .mount(&server)
                .await;
        }
        let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(&args)
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .env_remove("RUNPOD_HAPI_URL")
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}: {}",
            case["name"],
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout)
            .unwrap()
            .replace("\r\n", "\n");
        let expected = case["stdout"].as_str().unwrap();
        if case["as_json"] == true {
            let parse = |s: &str| {
                s.lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(parse(&text), parse(expected), "{}", case["name"]);
        } else {
            assert_eq!(text, expected, "{}", case["name"]);
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            case["calls"].as_array().unwrap().len() + usize::from(endpoint)
        );
        for request in requests.iter().filter(|r| r.url.path().ends_with("/logs")) {
            let call = &case["calls"][0];
            let mut expected = vec![("tail".to_owned(), call["tail"].to_string())];
            for (name, key) in [("source", "log_type"), ("since", "since")] {
                if let Some(value) = call[key].as_str() {
                    expected.push((name.into(), value.into()));
                }
            }
            let mut actual: Vec<_> = request.url.query_pairs().into_owned().collect();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn test_logs_reject_tail_above_api_limit() {
    for group in ["pod", "endpoint"] {
        for args in [["--tail", "5001"], ["--tail", "-1"], ["--source", "all"]] {
            let result = std::process::Command::new(env!("CARGO_BIN_EXE_rp"))
                .args([group, "logs", "id"])
                .args(args)
                .env_remove("RUNPOD_API_KEY")
                .env_remove("HOME")
                .env_remove("USERPROFILE")
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(2));
        }
    }
}

#[tokio::test]
async fn stream_auth_errors_and_disappeared_workers() {
    for endpoint in [false, true] {
        for status in [401, 403, 404] {
            let server = MockServer::start().await;
            Mock::given(path("/v2/serverless/id/workers"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"workers":[{"id":"id"}]})),
                )
                .mount(&server)
                .await;
            let route = if endpoint {
                "/v2/serverless/id/workers/id/logs"
            } else {
                "/v2/pods/id/logs"
            };
            Mock::given(path(route))
                .respond_with(
                    ResponseTemplate::new(status).set_delay(std::time::Duration::from_millis(20)),
                )
                .expect(1)
                .mount(&server)
                .await;
            let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
                .args([if endpoint { "endpoint" } else { "pod" }, "logs", "id"])
                .env("RUNPOD_API_KEY", "fixture-key")
                .env("RUNPOD_API_BASE_URL", server.uri())
                .env_remove("RUNPOD_HAPI_URL")
                .output()
                .await
                .unwrap();
            assert_eq!(
                result.status.code(),
                Some(i32::from(!(endpoint && status == 404)))
            );
            assert!(result.stdout.is_empty());
            if !result.status.success() {
                assert!(String::from_utf8_lossy(&result.stderr).contains(&status.to_string()));
            }
        }
    }
}

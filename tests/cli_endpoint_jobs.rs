use runpod::Value;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/endpoint_jobs.json")).unwrap();
    let mut checked = 0;
    for case in cases.iter().filter(|case| case["name"] == name) {
        checked += 1;
        let server = MockServer::start().await;
        let expected = case["requests"].as_array().unwrap();
        assert!(expected.len() <= 1);
        if let Some(reply) = expected.first() {
            let template =
                ResponseTemplate::new(u16::try_from(reply["status"].as_u64().unwrap()).unwrap());
            let template = if let Some(raw) = reply["raw"].as_str() {
                template.set_body_string(raw)
            } else {
                template.set_body_json(&reply["response"])
            };
            Mock::given(any())
                .respond_with(template)
                .mount(&server)
                .await;
        }
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"));
        command
            .arg("endpoint")
            .args(args)
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", format!("{}/control", server.uri()))
            .env("RUNPOD_ENDPOINT_BASE_URL", format!("{}/v2", server.uri()))
            .stdin(std::process::Stdio::null());
        if name == "job_confirmation_precedes_credentials" {
            command
                .env_remove("RUNPOD_API_KEY")
                .env_remove("HOME")
                .env_remove("USERPROFILE");
        }
        let result = command.output().await.unwrap();
        let stdout = String::from_utf8(result.stdout).unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(
            result.status.code().map(i64::from),
            case["code"].as_i64(),
            "{case}\n{stdout}\n{stderr}"
        );
        if case["code"] != 0 {
            let expected = case["stderr"]
                .as_str()
                .unwrap()
                .split("Error: ")
                .last()
                .unwrap()
                .trim();
            assert!(stderr.contains(expected), "{case}\n{stderr}");
        } else if let Ok(expected) = serde_json::from_str::<Value>(case["stdout"].as_str().unwrap())
        {
            assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), expected);
        }
        for message in case["success"].as_array().unwrap() {
            assert!(
                stdout.contains(message.as_str().unwrap()),
                "{case}\n{stdout}"
            );
        }
        for fields in case["fields"].as_array().unwrap() {
            for field in fields.as_array().unwrap() {
                let label = field[0].as_str().unwrap();
                let line = stdout
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix(label)
                            .filter(|rest| rest.starts_with(' '))
                            .map(str::trim)
                    })
                    .unwrap_or_else(|| panic!("missing field {label}: {stdout}"));
                let expected = &field[1];
                let expected = if expected.is_null() || expected == "" {
                    "-".to_owned()
                } else {
                    expected.as_str().unwrap().to_owned()
                };
                if label == "output" && expected != "-" {
                    match serde_json::from_str::<Value>(&expected) {
                        Ok(value) => {
                            assert_eq!(serde_json::from_str::<Value>(line).unwrap(), value);
                        }
                        Err(_) => assert_eq!(line, expected),
                    }
                } else {
                    assert_eq!(line, expected, "{case}");
                }
            }
        }
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), expected.len(), "{case}");
        for (actual, expected) in received.iter().zip(expected) {
            assert_eq!(actual.method.as_str(), expected["method"].as_str().unwrap());
            assert_eq!(
                actual.url.path(),
                format!("/v2/{}", expected["path"].as_str().unwrap())
            );
            assert_eq!(actual.headers["authorization"], "Bearer fixture-key");
            assert_eq!(actual.headers["content-type"], "application/json");
            assert!(actual.body.is_empty(), "null payload means no body");
            assert!(actual.url.query().is_none());
        }
    }
    assert!(checked > 0, "no cases for {name}");
}

macro_rules! source_tests { ($($name:ident),* $(,)?) => {$( #[tokio::test] async fn $name() { verify(stringify!($name)).await; } )*}; }
source_tests!(
    test_job_get,
    test_job_cancel_and_retry,
    test_purge,
    job_output_values,
    encoded_job_ids,
    job_response_fallbacks,
    job_http_errors,
    job_confirmation_precedes_credentials,
);

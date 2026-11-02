use runpod::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[test]
fn input_errors_precede_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    for (args, message) in [
        (
            vec!["--input", missing.to_str().unwrap(), "--async", "--stream"],
            "--input",
        ),
        (
            vec!["--async", "--stream"],
            "use either --async or --stream",
        ),
        (vec!["--data", "{bad"], "request body is not valid JSON"),
        (
            vec!["--input", "-", "--data", "{}"],
            "use either --input or --data",
        ),
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(["endpoint", "run", "ep1"])
            .args(args)
            .env_remove("RUNPOD_API_KEY")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(result.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains(message), "{stderr}");
    }
}

#[tokio::test]
async fn malformed_endpoint_record_does_not_submit_a_job() {
    for value in [Value::Null, json!([]), json!("invalid")] {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(value))
            .mount(&server)
            .await;
        let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(["endpoint", "run", "ep1", "--data", "{}", "--async"])
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .env("RUNPOD_ENDPOINT_BASE_URL", format!("{}/data", server.uri()))
            .output()
            .await
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains("invalid response")
        );
        let calls = server.received_requests().await.unwrap();
        assert_eq!(
            calls.len(),
            1,
            "invalid endpoint records must fail before submission"
        );
        assert_eq!(calls[0].method.as_str(), "GET");
    }
}

#[tokio::test]
async fn run_timeout_starts_after_submission_and_bounds_polling() {
    use wiremock::matchers::{method, path};
    for streaming in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/serverless/ep1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_millis(200))
                    .set_body_json(json!({"id":"ep1","type":"QUEUE"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/data/ep1/run"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_millis(200))
                    .set_body_json(json!({"id":"job1"})),
            )
            .mount(&server)
            .await;
        let operation = if streaming { "stream" } else { "status" };
        Mock::given(method("GET"))
            .and(path(format!("/data/ep1/{operation}/job1")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(2))
                    .set_body_json(json!({"id":"job1","status":"IN_QUEUE"})),
            )
            .mount(&server)
            .await;
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"));
        command
            .args(["endpoint", "run", "ep1", "--data", "{}", "--timeout", "0.1"])
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .env("RUNPOD_ENDPOINT_BASE_URL", format!("{}/data", server.uri()));
        if streaming {
            command.arg("--stream");
        }
        let result = command.output().await.unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(result.status.code(), Some(1), "{stderr}");
        let resume = if streaming { "stream" } else { "wait" };
        assert!(
            stderr.contains(&format!(
                "after 0.1s. resume with: rp endpoint job {resume} ep1 job1"
            )),
            "{stderr}"
        );
        let calls = server.received_requests().await.unwrap();
        assert_eq!(
            calls.len(),
            3,
            "lookup and submission must not consume the wait deadline"
        );
        assert_eq!(calls.iter().filter(|call| call.method == "POST").count(), 1);
    }
}

fn normalized(text: &str) -> Vec<Value> {
    if let Ok(values) = serde_json::Deserializer::from_str(text)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
    {
        return values;
    }
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|_| line.into()))
        .collect()
}
fn expected_path(call: &Value) -> String {
    match call["kind"].as_str().unwrap() {
        "data" => format!("/data/{}", call["path"].as_str().unwrap()),
        "control" => call["path"].as_str().unwrap().into(),
        "lb" => format!(
            "/lb{}",
            reqwest::Url::parse(call["url"].as_str().unwrap())
                .unwrap()
                .path()
        ),
        other => panic!("unexpected transport {other}"),
    }
}
async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/endpoint_run.json")).unwrap();
    let mut count = 0;
    for case in cases.iter().filter(|case| case["name"] == name) {
        count += 1;
        let server = MockServer::start().await;
        let replies = case["requests"].as_array().unwrap().clone();
        let base = server.uri();
        let index = Arc::new(AtomicUsize::new(0));
        Mock::given(any())
            .respond_with(move |_: &Request| {
                let Some(reply) = replies.get(index.fetch_add(1, Ordering::SeqCst)) else {
                    return ResponseTemplate::new(500);
                };
                let template = ResponseTemplate::new(
                    u16::try_from(reply["status"].as_u64().unwrap()).unwrap(),
                );
                if let Some(raw) = reply["raw"].as_str() {
                    return template.set_body_string(raw);
                }
                let mut value = reply["response"].clone();
                if reply["kind"] == "control" && value["type"] == "LOAD_BALANCER" {
                    let prefix = value["requestUrls"]["base"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(|url| {
                            reqwest::Url::parse(url)
                                .unwrap()
                                .path()
                                .trim_end_matches('/')
                                .to_owned()
                        })
                        .unwrap_or_default();
                    value["requestUrls"] = json!({"base":format!("{base}/lb{prefix}")});
                }
                template.set_body_json(value)
            })
            .mount(&server)
            .await;
        let directory = tempfile::tempdir().unwrap();
        let document = directory.path().join("request.json");
        if let Some(text) = case["document"].as_str() {
            std::fs::write(&document, text).unwrap();
        }
        let args: Vec<std::ffi::OsString> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| {
                if arg == "{document}" {
                    document.as_os_str().to_owned()
                } else {
                    arg.as_str().unwrap().into()
                }
            })
            .collect();
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .arg("endpoint")
            .args(args)
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .env("RUNPOD_ENDPOINT_BASE_URL", format!("{}/data", server.uri()))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = case["input"].as_str() {
            use tokio::io::AsyncWriteExt;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .await
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let result = child.wait_with_output().await.unwrap();
        let stdout = String::from_utf8(result.stdout).unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(
            result.status.code().map(i64::from),
            case["code"].as_i64(),
            "{case}\n{stdout}\n{stderr}"
        );
        if case["success"].as_array().unwrap().is_empty() {
            assert_eq!(
                normalized(&stdout),
                normalized(case["stdout"].as_str().unwrap()),
                "{case}"
            );
        } else {
            for message in case["success"].as_array().unwrap() {
                assert!(stdout.contains(message.as_str().unwrap()), "{stdout}");
            }
            assert!(stdout.contains("rp endpoint job wait"), "{stdout}");
        }
        if case["code"] != 0 {
            let source = case["stderr"].as_str().unwrap();
            if source.contains("Invalid value for '--timeout'") {
                assert!(stderr.contains("--timeout"), "{stderr}");
            } else if source.contains("request body is not valid JSON") {
                assert!(
                    stderr.contains("request body is not valid JSON"),
                    "{stderr}"
                );
            } else {
                let mut expected = source.split("Error: ").last().unwrap().trim().to_owned();
                for call in case["requests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|call| call["kind"] == "lb")
                {
                    let url = call["url"].as_str().unwrap();
                    expected =
                        expected.replace(url, &format!("{}{}", server.uri(), expected_path(call)));
                }
                assert!(stderr.contains(&expected), "{case}\n{stderr}");
            }
        } else {
            assert_eq!(stderr, case["stderr"].as_str().unwrap(), "{case}");
        }
        let received = server.received_requests().await.unwrap();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(received.len(), expected.len(), "{case}");
        for (actual, expected) in received.iter().zip(expected) {
            assert_eq!(actual.method.as_str(), expected["method"].as_str().unwrap());
            assert_eq!(actual.url.path(), expected_path(expected));
            assert_eq!(actual.headers["authorization"], "Bearer fixture-key");
            if expected["kind"] == "lb" {
                assert_eq!(actual.headers["accept"], "application/json");
                assert_eq!(
                    actual.url.query(),
                    reqwest::Url::parse(expected["url"].as_str().unwrap())
                        .unwrap()
                        .query()
                );
                if expected["body"].is_null() {
                    assert!(!actual.headers.contains_key("content-type"));
                }
            }
            let body: Value = if actual.body.is_empty() {
                Value::Null
            } else {
                actual.body_json().unwrap()
            };
            assert_eq!(body, expected["body"], "{case}");
        }
    }
    assert!(count > 0, "no cases for {name}");
}
macro_rules! source_tests { ($($name:ident),* $(,)?) => {$( #[tokio::test] async fn $name() { verify(stringify!($name)).await; } )*}; }
source_tests!(
    test_run_waits_for_output,
    test_run_reads_input_file_and_stdin,
    test_run_async,
    test_run_failed_job_exits_nonzero,
    test_stream_responses_can_omit_chunks,
    test_run_stream,
    test_run_stream_failure_and_json,
    test_run_rejects_bad_flags,
    test_load_balancer_plain_text_json,
    test_run_load_balancer,
    test_job_timeouts_must_be_positive_before_requests,
    queue_envelope_edges,
    queue_policy_merge,
    encoded_run_id,
    load_balancer_body_and_url,
    load_balancer_response_values
);

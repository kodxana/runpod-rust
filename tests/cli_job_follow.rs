use runpod::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn job_deadline_bounds_an_inflight_http_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/ep1/status/job1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(3))
                .set_body_json(json!({"id":"job1","status":"IN_QUEUE"})),
        )
        .mount(&server)
        .await;
    let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
        .args([
            "endpoint",
            "job",
            "wait",
            "ep1",
            "job1",
            "--timeout",
            "0.05",
        ])
        .env("RUNPOD_API_KEY", "fixture-key")
        .env("RUNPOD_ENDPOINT_BASE_URL", format!("{}/v2", server.uri()))
        .output()
        .await
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains("after 0.05s"), "{stderr}");
    assert!(
        stderr.contains("resume with: rp endpoint job wait ep1 job1"),
        "{stderr}"
    );
    assert!(result.stdout.is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn job_wait_and_stream_use_data_plane_and_print_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/ep1/status/job1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"job1","status":"COMPLETED","output":"done"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/ep1/stream/job1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"COMPLETED"})))
        .mount(&server)
        .await;
    for operation in ["wait", "stream"] {
        let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(["endpoint", "job", operation, "ep1", "job1"])
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_ENDPOINT_BASE_URL", format!("{}/v2", server.uri()))
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), "done");
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert_eq!(request.headers["authorization"], "Bearer fixture-key");
    }
}

#[tokio::test]
async fn job_wait_timeout_is_validated_before_credentials() {
    for value in ["0", "-1", "NaN", "inf"] {
        let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(["endpoint", "job", "wait", "ep1", "job1", "--timeout", value])
            .env_remove("RUNPOD_API_KEY")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .output()
            .await
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains("--timeout")
        );
    }
}

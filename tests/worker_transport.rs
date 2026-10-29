use runpod::{
    json,
    serverless::{JobsProgress, ResultKind, WorkerConfig, WorkerJob, WorkerTransport},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

async fn setup() -> (MockServer, WorkerTransport) {
    let server = MockServer::start().await;
    let config = WorkerConfig {
        worker_id: "worker".into(),
        api_key: "worker-token".into(),
        job_get_url: Some(format!("{}/job-take/$ID?token=secret", server.uri())),
        job_done_url: Some(format!(
            "{}/job-done/$ID?worker=$RUNPOD_POD_ID",
            server.uri()
        )),
        job_stream_url: Some(format!(
            "{}/job-stream/$ID?worker=$RUNPOD_POD_ID",
            server.uri()
        )),
        ..WorkerConfig::default()
    };
    (server, WorkerTransport::new(config).unwrap())
}

#[tokio::test]
async fn test_get_job_200() {
    let (server, transport) = setup().await;
    Mock::given(path("/job-take/worker"))
        .and(header("Authorization", "worker-token"))
        .and(query_param("job_in_progress", "0"))
        .and(query_param("token", "secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"123","input":{"number":1},"custom":42})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let jobs = transport.get_job(1, &JobsProgress::new()).await.unwrap();
    assert_eq!(jobs[0].input, json!({"number":1}));
    assert_eq!(jobs[0].metadata["custom"], json!(42));
}

#[tokio::test]
async fn test_get_job_empty_and_invalid_response() {
    for (status, body, expect_error) in [
        (204, "", false),
        (400, "", false),
        (429, "", true),
        (500, "", true),
        (200, "not json", false),
        (200, "{}", true),
        (200, "[]", false),
    ] {
        let (server, transport) = setup().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(&server)
            .await;
        let result = transport.get_job(1, &JobsProgress::new()).await;
        assert_eq!(
            result.is_err(),
            expect_error,
            "status {status}, body {body}"
        );
        if !expect_error {
            assert!(result.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn test_get_job_batch_and_stop_signals() {
    let (server, transport) = setup().await;
    let progress = JobsProgress::new();
    progress.add(WorkerJob::new("busy", json!({})));
    Mock::given(path("/job-take-batch/worker"))
        .and(query_param("batch_size", "2"))
        .and(query_param("job_in_progress", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"id":"a","input":1},{"id":"b","input":2}])),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(transport.get_job(2, &progress).await.unwrap().len(), 2);
    Mock::given(path("/job-stop/worker"))
        .and(query_param("token", "secret"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"jobsToStop":["a",123,null,"b"]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(transport.get_stop_signals().await.unwrap(), vec!["a", "b"]);
}

#[tokio::test]
async fn test_send_result_and_stream_result() {
    let (server, transport) = setup().await;
    for (route, stream, body) in [
        ("/job-done/job", "true", json!({"output":[1]})),
        ("/job-stream/job", "false", json!({"output":1})),
    ] {
        Mock::given(method("POST"))
            .and(path(route))
            .and(query_param("worker", "worker"))
            .and(query_param("isStream", stream))
            .and(header("X-Request-ID", "job"))
            .and(header("Content-Type", "application/x-www-form-urlencoded"))
            .and(body_json(body))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
    }
    let job = WorkerJob::new("job", json!({}));
    transport
        .send_result(&json!({"output":[1]}), &job, ResultKind::Stream)
        .await
        .unwrap();
    transport
        .stream_result(&json!({"output":1}), &job)
        .await
        .unwrap();
}

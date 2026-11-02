use super::*;
use runpod::apps::{App, NativeFunction, ResourceSpec, targets::FunctionDefinition};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Barrier, Notify};
use wiremock::matchers::{body_json, query_param};

fn queue_command(mode: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("RUNPOD_") || name.starts_with("FLASH_") {
            command.env_remove(key);
        }
    }
    command
        .args([
            "--exact",
            "worker_process_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNPOD_RUST_WORKER_PROCESS", mode)
        .env("RUNPOD_SKIP_FITNESS_CHECKS", "true")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

pub(super) fn apps_queue_child(mode: &str) {
    let init_calls = Arc::new(AtomicUsize::new(0));
    let initialized = init_calls.clone();
    let barrier = Arc::new(Barrier::new(if mode == "apps-queue-platform" {
        3
    } else {
        1
    }));
    let handle = App::new("native-worker").unwrap().queue(ResourceSpec::default(), FunctionDefinition {
        name: "echo".into(), parameters: vec!["value".into()], source: None,
    }, NativeFunction::streaming(move |call| {
        let init_calls = init_calls.clone(); let barrier = barrier.clone();
        async_stream::try_stream! {
            assert!(call.args.is_empty()); assert!(!call.kwargs.contains_key("__empty"));
            barrier.wait().await;
            yield json!({"value":call.kwargs["value"],"init":init_calls.load(Ordering::SeqCst)});
            yield json!("end");
        }
    })).unwrap();
    let fail = mode == "apps-queue-init-failure";
    handle
        .init(NativeFunction::new(move |_| {
            let initialized = initialized.clone();
            async move {
                initialized.fetch_add(1, Ordering::SeqCst);
                if fail {
                    Err(Error::InvalidInput("apps init failed".into()))
                } else {
                    Ok(json!(null))
                }
            }
        }))
        .unwrap();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(handle.start_queue_worker());
    if fail {
        assert!(result.unwrap_err().to_string().contains("apps init failed"));
    } else {
        result.unwrap();
    }
}

#[tokio::test]
async fn apps_queue_start_uses_environment_concurrency_and_aggregate_protocol() {
    let server = MockServer::start().await;
    let first = AtomicUsize::new(0);
    Mock::given(path("/job-take-batch/native-worker"))
        .and(query_param("batch_size", "3"))
        .respond_with(move |_: &wiremock::Request| {
            if first.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(200).set_body_json(json!([
                    {"id":"a","input":{"value":"a","__empty":true}},
                    {"id":"b","input":{"value":"b"}},
                    {"id":"c","input":{"value":"c"}}
                ]))
            } else {
                ResponseTemplate::new(204).set_delay(Duration::from_secs(30))
            }
        })
        .mount(&server)
        .await;
    Mock::given(path("/job-stop/native-worker"))
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let finished = Arc::new(Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    for id in ["a", "b", "c"] {
        for value in [json!({"value":id,"init":1}), json!("end")] {
            Mock::given(path(format!("/job-stream/{id}")))
                .and(body_json(json!({"output":value})))
                .and(query_param("isStream", "false"))
                .respond_with(ResponseTemplate::new(200))
                .expect(1)
                .mount(&server)
                .await;
        }
        let finished = finished.clone();
        let count = count.clone();
        Mock::given(path(format!("/job-done/{id}")))
            .and(body_json(json!({"output":[{"value":id,"init":1},"end"]})))
            .and(query_param("isStream", "true"))
            .respond_with(move |_: &wiremock::Request| {
                if count.fetch_add(1, Ordering::SeqCst) == 2 {
                    finished.notify_one();
                }
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;
    }
    let mut command = queue_command("apps-queue-platform");
    command
        .env("RUNPOD_MAX_CONCURRENCY", "3")
        .env("RUNPOD_POD_ID", "native-worker")
        .env(
            "RUNPOD_WEBHOOK_GET_JOB",
            format!("{}/job-take/$ID", server.uri()),
        )
        .env(
            "RUNPOD_WEBHOOK_POST_OUTPUT",
            format!("{}/job-done/$ID", server.uri()),
        )
        .env(
            "RUNPOD_WEBHOOK_POST_STREAM",
            format!("{}/job-stream/$ID", server.uri()),
        );
    let mut process = command.spawn().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), finished.notified()).await;
    process.kill().await.unwrap();
    let output = process.wait_with_output().await.unwrap();
    assert!(
        result.is_ok(),
        "worker failed to run three concurrent jobs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn apps_queue_start_local_runs_init_and_keeps_stream_aggregate() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("test_input.json"),
        r#"{"input":{"value":"local","__empty":false}}"#,
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        queue_command("apps-queue-local")
            .current_dir(directory.path())
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.status.success(), "{result:?}");
    let log = String::from_utf8_lossy(&result.stderr);
    let output = log
        .lines()
        .find_map(|line| line.strip_prefix("output="))
        .expect("local output log");
    let output: runpod::Value = serde_json::from_str(output).unwrap();
    assert_eq!(output, json!({"output":[{"value":"local","init":1},"end"]}));
}

#[tokio::test]
async fn apps_queue_init_failure_precedes_core_config_and_any_network() {
    let server = MockServer::start().await;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        queue_command("apps-queue-init-failure")
            .env("RUNPOD_PING_INTERVAL", "invalid")
            .env(
                "RUNPOD_WEBHOOK_GET_JOB",
                format!("{}/job-take/$ID", server.uri()),
            )
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

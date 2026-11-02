use runpod::{
    Value,
    apps::bootstrap::{PhaseError, report_error},
    json,
};
use std::{process::Stdio, time::Duration};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

#[test]
#[ignore = "spawned by the bootstrap reporting process tests"]
fn bootstrap_reporting_process_child() {
    if std::env::var("RUNPOD_RUST_REPORT_PROCESS").as_deref() != Ok("1") {
        return;
    }
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(report_error(&PhaseError::new("locate", "artifact missing")));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn command(kind: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("RUNPOD_") || name.starts_with("FLASH_") || name == "PORT" {
            command.env_remove(key);
        }
    }
    command
        .args([
            "--exact",
            "bootstrap_reporting_process_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNPOD_RUST_REPORT_PROCESS", "1")
        .env("RUNPOD_RUNTIME_KIND", kind)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_http(child: &mut tokio::process::Child, url: &str) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if client.get(format!("{url}/ping")).send().await.is_ok() {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "reporter exited before opening HTTP listener"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    client
}

#[tokio::test]
async fn task_bootstrap_error_never_enters_queue_loop() {
    let control = MockServer::start().await;
    Mock::given(path("/graphql"))
        .and(header("Authorization", "Bearer pod-key"))
        .and(header("X-RunPod-Pod-Id", "failed-pod"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"flashTaskTerminate":true}})),
        )
        .expect(1)
        .mount(&control)
        .await;
    let port = port();
    let mut child = command("task")
        .env("RUNPOD_TASK_PORT", port.to_string())
        .env("RUNPOD_TASK_TOKEN", "task-secret")
        .env("RUNPOD_TASK_IDLE_TIMEOUT", "1")
        .env("RUNPOD_API_KEY", "pod-key")
        .env("RUNPOD_POD_ID", "failed-pod")
        .env("RUNPOD_API_BASE_URL", control.uri())
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let http = wait_http(&mut child, &url).await;
    let public = http.get(format!("{url}/ping")).send().await.unwrap();
    assert_eq!(public.status(), 500);
    assert_eq!(
        public.json::<Value>().await.unwrap(),
        json!({"ready":false,"error_type":"BootstrapError"})
    );
    assert_eq!(
        http.get(format!("{url}/result"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = http
        .post(format!("{url}/execute"))
        .bearer_auth("task-secret")
        .body("malformed")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let mut expected = PhaseError::new("locate", "artifact missing").payload();
    expected["ready"] = json!(false);
    assert_eq!(response.json::<Value>().await.unwrap(), expected);
    // The production watchdog checks idle every fifteen seconds and exits only after ack.
    let output = tokio::time::timeout(Duration::from_secs(25), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!logs.contains("task-secret") && !logs.contains("pod-key"));
}

#[tokio::test]
async fn api_dispatch_uses_port_and_public_phase_error() {
    let port = port();
    let mut child = command("api")
        .env("PORT", port.to_string())
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let http = wait_http(&mut child, &url).await;
    let response = http.patch(format!("{url}/unknown")).send().await.unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        PhaseError::new("locate", "artifact missing").payload()
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn unknown_kind_dispatches_queue_jobs() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"failed-job"})))
        .mount(&server)
        .await;
    let (sent, done) = tokio::sync::oneshot::channel();
    let sent = std::sync::Mutex::new(Some(sent));
    Mock::given(method("POST"))
        .and(path("/out/failed-job"))
        .and(header("Authorization", "queue-key"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let payload: Value = serde_json::from_str(body["error"].as_str().unwrap()).unwrap();
            assert_eq!(
                payload,
                PhaseError::new("locate", "artifact missing").payload()
            );
            if let Some(sent) = sent.lock().unwrap().take() {
                let _ = sent.send(());
            }
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    let mut child = command("unknown")
        .env("RUNPOD_WEBHOOK_GET_JOB", format!("{}/get", server.uri()))
        .env(
            "RUNPOD_WEBHOOK_POST_OUTPUT",
            format!("{}/out/$ID", server.uri()),
        )
        .env("RUNPOD_AI_API_KEY", "queue-key")
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), done)
        .await
        .unwrap()
        .unwrap();
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn missing_webhooks_exit_with_failure_without_polling() {
    let output = tokio::time::timeout(Duration::from_secs(5), command("queue").output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no job webhook env"));
}

use runpod::{Value, apps::bootstrap::start_deployed, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::sync::oneshot;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, path},
};
#[path = "bootstrap_startup/mod.rs"]
mod bootstrap_startup;

#[test]
#[ignore = "spawned by bootstrap startup tests"]
fn bootstrap_startup_child() {
    if std::env::var("RUNPOD_RUST_BOOTSTRAP_CHILD").as_deref() != Ok("1") {
        return;
    }
    let initial_dir = std::env::var_os("RUNPOD_APP_DIR");
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(start_deployed(move |module, context| {
            assert_eq!(std::env::var_os("RUNPOD_APP_DIR"), initial_dir);
            bootstrap_startup::register(module, context)
        }));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn manifest(kind: &str) -> Value {
    json!({"app":"example","resources":[{"name":"work","kind":kind,"module":"user","qualname":"work"}]})
}
fn archive(root: &Path, manifest: &Value) {
    archive_text(root, &manifest.to_string());
}
fn archive_text(root: &Path, data: &str) {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        std::fs::File::create(root.join("artifact.tar.gz")).unwrap(),
        flate2::Compression::default(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(data.len()).unwrap());
    header.set_mode(0o644);
    tar.append_data(&mut header, "runpod_manifest.json", data.as_bytes())
        .unwrap();
    let mut asset = tar::Header::new_gnu();
    asset.set_size(13);
    asset.set_mode(0o644);
    tar.append_data(&mut asset, "payload.txt", b"artifact-data".as_slice())
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
}
fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn command(root: &Path, kind: &str) -> tokio::process::Command {
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
            "bootstrap_startup_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNPOD_RUST_BOOTSTRAP_CHILD", "1")
        .env("RUNPOD_RUST_BOOTSTRAP_ROOT", root)
        .env("RUNPOD_RUNTIME_KIND", kind)
        .env("FLASH_APP_NAME", "example")
        .env("FLASH_ENVIRONMENT", "default")
        .env("FLASH_RESOURCE_NAME", "work")
        .env("RUNPOD_BUILD_ID", "build")
        .env("RUNPOD_APP_DIR", root.join("unpacked"))
        .env("FLASH_BUILD_ARTIFACT_PATH", root.join("artifact.tar.gz"))
        .env("RUNPOD_ARTIFACT_WAIT", "0")
        .env("RUNPOD_SKIP_FITNESS_CHECKS", "true")
        .env("RUNPOD_TASK_TOKEN", "task-token")
        .env(
            "RUNPOD_MOUNTS",
            json!([{"kind":"network","reference":"data","id":"nv-1","path":"/data"}]).to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}
async fn ready(child: &mut tokio::process::Child, url: &str, status: u16) -> reqwest::Client {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(response) = http.get(format!("{url}/ping")).send().await
                && response.status() == status
            {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "bootstrap worker exited before expected readiness {status}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    http
}

#[tokio::test]
async fn test_deployed_path_serves_queue_from_prebuilt_directory() {
    let root = tempfile::tempdir().unwrap();
    let prebuilt = root.path().join("prebuilt");
    std::fs::create_dir(&prebuilt).unwrap();
    std::fs::write(prebuilt.join("payload.txt"), "artifact-data").unwrap();
    std::fs::write(
        prebuilt.join("runpod_manifest.json"),
        manifest("queue").to_string(),
    )
    .unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/job-take/pod"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":"job","input":{"value":42}})),
        )
        .mount(&server)
        .await;
    let (sent, done) = oneshot::channel();
    let sent = std::sync::Mutex::new(Some(sent));
    Mock::given(path("/job-done/job"))
        .and(body_json(
            json!({"output":{"value":42,"init":true,"mount":"/data","asset":"artifact-data"}}),
        ))
        .respond_with(move |_: &wiremock::Request| {
            if let Some(sent) = sent.lock().unwrap().take() {
                let _ = sent.send(());
            }
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    let mut child = command(root.path(), "queue")
        .env("RUNPOD_PREBUILT_APP_DIR", prebuilt)
        .env("RUNPOD_POD_ID", "pod")
        .env(
            "RUNPOD_WEBHOOK_GET_JOB",
            format!("{}/job-take/$ID", server.uri()),
        )
        .env(
            "RUNPOD_WEBHOOK_POST_OUTPUT",
            format!("{}/job-done/$ID", server.uri()),
        )
        .spawn()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), done).await;
    child.kill().await.unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(
        result.is_ok(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.path().join("unpacked").exists());
    assert!(root.path().join("loaded").is_file());
}

#[tokio::test]
async fn deployed_api_extracts_manifest_and_initializes_routes() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path(), &manifest("api"));
    let port = port();
    let mut child = command(root.path(), "api")
        .env("PORT", port.to_string())
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let http = ready(&mut child, &url, 200).await;
    let response = http.get(format!("{url}/value")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"value":42,"init":true,"mount":"/data","asset":"artifact-data"})
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    assert!(root.path().join("unpacked/.rp-unpacked").is_file());
}

#[tokio::test]
async fn deployed_task_runs_after_extraction_and_exits_on_acknowledged_deadline() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path(), &manifest("task"));
    let control = MockServer::start().await;
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"flashTaskTerminate":true}})),
        )
        .expect(1)
        .mount(&control)
        .await;
    let port = port();
    let mut child = command(root.path(), "task")
        .env("RUNPOD_TASK_PORT", port.to_string())
        .env("RUNPOD_TASK_EXECUTION_TIMEOUT_SECONDS", "0.5")
        .env("RUNPOD_POD_ID", "pod")
        .env("RUNPOD_API_KEY", "pod-key")
        .env("RUNPOD_API_BASE_URL", control.uri())
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let http = ready(&mut child, &url, 200).await;
    let response = http
        .post(format!("{url}/execute"))
        .bearer_auth("task-token")
        .json(&json!({"args":[42]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"success":true,"json_result":{"value":42,"init":true,"mount":"/data","asset":"artifact-data"},"stdout":""})
    );
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(root.path().join("unpacked/.rp-unpacked").is_file());
}

#[tokio::test]
async fn test_phase_error_reports_before_loading_application() {
    for (kind, phase, contents) in [
        ("api", "locate", ""),
        ("api", "system", ""),
        ("task", "attach", "{bad"),
        ("task", "attach", "[]"),
        ("task", "attach", "{\"app\":\"wrong\"}"),
        ("task", "identity", ""),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut cmd = command(root.path(), kind);
        let port = port();
        cmd.env("PORT", port.to_string())
            .env("RUNPOD_TASK_PORT", port.to_string());
        if phase == "attach" {
            archive_text(root.path(), contents);
        }
        if phase == "identity" {
            archive(root.path(), &manifest("task"));
            cmd.env_remove("RUNPOD_BUILD_ID");
        }
        if phase == "system" {
            let mut value = manifest("api");
            value["resources"][0]["systemDependencies"] = json!(["ffmpeg"]);
            archive(root.path(), &value);
            let empty_bin = root.path().join("empty-bin");
            std::fs::create_dir(&empty_bin).unwrap();
            cmd.env("PATH", empty_bin);
        }
        let mut child = cmd.spawn().unwrap();
        let url = format!("http://127.0.0.1:{port}");
        let http = ready(&mut child, &url, 500).await;
        let response = http
            .get(format!("{url}/result"))
            .bearer_auth("task-token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 500);
        assert!(
            response.json::<Value>().await.unwrap()["error_message"]
                .as_str()
                .unwrap()
                .contains(&format!("during '{phase}'"))
        );
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        assert!(!root.path().join("loaded").exists());
        if phase == "identity" {
            assert!(!root.path().join("unpacked").exists());
        }
    }
}

#[tokio::test]
async fn queue_preparation_failure_reports_jobs_without_loading_user_code() {
    let root = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/get"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"failed-job"})))
        .mount(&server)
        .await;
    let (sent, done) = oneshot::channel();
    let sent = std::sync::Mutex::new(Some(sent));
    Mock::given(path("/out/failed-job"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let error: Value = serde_json::from_str(body["error"].as_str().unwrap()).unwrap();
            assert_eq!(error["error_type"], "BootstrapError");
            assert!(
                error["error_message"]
                    .as_str()
                    .unwrap()
                    .contains("during 'locate'")
            );
            if let Some(sent) = sent.lock().unwrap().take() {
                let _ = sent.send(());
            }
            ResponseTemplate::new(200)
        })
        .mount(&server)
        .await;
    let mut child = command(root.path(), "queue")
        .env("RUNPOD_WEBHOOK_GET_JOB", format!("{}/get", server.uri()))
        .env(
            "RUNPOD_WEBHOOK_POST_OUTPUT",
            format!("{}/out/$ID", server.uri()),
        )
        .spawn()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), done).await;
    child.kill().await.unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert!(
        result.is_ok(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.path().join("loaded").exists());
}

use runpod::{
    Value,
    apps::{
        App, AppRegistry, NativeFunction, ResourceSpec, bootstrap::BootstrapConfig,
        deploy::SymbolReference, runtime::DeployedContext, targets::FunctionDefinition,
        task_runner,
    },
    json,
};
use std::{process::Stdio, time::Duration};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, path},
};

#[test]
#[ignore = "spawned by the parent protocol test"]
fn task_runner_process_child() {
    if std::env::var("RUNPOD_RUST_TASK_PROCESS").as_deref() != Ok("1") {
        return;
    }
    let path = BootstrapConfig::from_env().unwrap().locate().unwrap();
    let values = std::env::vars()
        .chain([("RUNPOD_APP_DIR".into(), path.to_str().unwrap().into())])
        .collect();
    let context = DeployedContext::from_values(&values).unwrap();
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(task_runner::start(async {
        context.load_task(|module,mounts| {
            assert_eq!(module,"user");
            let mounted=mounts.clone();
            let app=App::new("example")?;
            let task=app.task(ResourceSpec::default(),FunctionDefinition {name:"work".into(),parameters:vec![],source:None},NativeFunction::sync(move |args| {
                runpod::serverless::capture::stdout().write_text("task output\n")?;
                        if args.kwargs.get("block")==Some(&json!(true)) {
                            std::thread::sleep(Duration::from_secs(30));
                        }
                Ok(json!({"value":args.args[0],"mount":mounted.path("network","data")?}))
            }))?;
            task.set_symbol(SymbolReference {module:"user".into(),qualname:"work".into()})?;
            let mut registry=AppRegistry::new();registry.register(app);Ok(registry)
        }).await
    })).unwrap();
}

async fn subprocess(blocking: bool) {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("artifact.tar.gz");
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        std::fs::File::create(&archive).unwrap(),
        flate2::Compression::default(),
    ));
    let manifest=json!({"app":"example","resources":[{"name":"work","kind":"task","module":"user","qualname":"work"}]}).to_string();
    let mut entry = tar::Header::new_gnu();
    entry.set_mode(0o644);
    entry.set_size(u64::try_from(manifest.len()).unwrap());
    entry.set_cksum();
    tar.append_data(&mut entry, "runpod_manifest.json", manifest.as_bytes())
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    let control = MockServer::start().await;
    Mock::given(path("/graphql")).and(header("Authorization","Bearer internal-key"))
        .and(header("X-RunPod-Pod-Id","pod-integration"))
        .and(body_json(json!({"query":"mutation flashTaskTerminate($input: FlashTaskTerminateInput!) { flashTaskTerminate(input: $input) }","variables":{"input":{"podId":"pod-integration"}}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"flashTaskTerminate":true}})))
        .expect(1).mount(&control).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
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
            "task_runner_process_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNPOD_RUST_TASK_PROCESS", "1")
        .env("RUNPOD_RUNTIME_KIND", "task")
        .env("RUNPOD_BUILD_ID", "build")
        .env("FLASH_APP_NAME", "example")
        .env("FLASH_ENVIRONMENT", "default")
        .env("FLASH_RESOURCE_NAME", "work")
        .env("FLASH_BUILD_ARTIFACT_PATH", archive)
        .env("RUNPOD_APP_DIR", directory.path().join("unpacked"))
        .env("RUNPOD_ARTIFACT_WAIT", "0")
        .env("RUNPOD_TASK_TOKEN", "execution-token")
        .env("RUNPOD_TASK_PORT", port.to_string())
        .env("RUNPOD_TASK_EXECUTION_TIMEOUT_SECONDS", "0.5")
        .env("RUNPOD_POD_ID", "pod-integration")
        .env("RUNPOD_API_KEY", "internal-key")
        .env("RUNPOD_API_BASE_URL", control.uri())
        .env(
            "RUNPOD_MOUNTS",
            json!([{"kind":"network","reference":"data","id":"nv-1","path":"/data"}]).to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(response) = http.get(format!("{url}/ping")).send().await
                && response.status() == 200
            {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "task exited before readiness"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(control.received_requests().await.unwrap().is_empty());
    let endpoint = if blocking { "submit" } else { "execute" };
    let response = http
        .post(format!("{url}/{endpoint}"))
        .bearer_auth("execution-token")
        .json(&json!({"args":[42],"kwargs":{"block":blocking}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let expected = if blocking {
        json!({"status":"RUNNING"})
    } else {
        json!({"success":true,"json_result":{"value":42,"mount":"/data"},"stdout":"task output\n"})
    };
    assert_eq!(response.json::<Value>().await.unwrap(), expected);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!logs.contains("internal-key"));
    assert!(!logs.contains("execution-token"));
    assert!(directory.path().join("unpacked/.rp-unpacked").is_file());
}

#[tokio::test]
async fn test_real_bootstrap_archive_http_and_deadline_cleanup() {
    subprocess(false).await;
}
#[tokio::test]
async fn acknowledged_deletion_exits_even_with_blocking_user_code() {
    subprocess(true).await;
}

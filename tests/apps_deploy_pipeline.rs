use runpod::{
    Client, ClientConfig, Value,
    apps::{
        EnvValue, ResourceKind, ResourceSpec, Secret,
        build::{BuildOptions, BuildTools},
        deploy::{
            AppDefinition, ArtifactOptions, DeployOptions, ResourceDefinition, build_artifact,
            deploy_app,
        },
        events::EventSink,
    },
    json,
};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::Path,
    sync::{Arc, Mutex},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn fixture(root: &Path) -> (AppDefinition, ArtifactOptions) {
    let project = root.join("project");
    let sdk = root.join("sdk");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&sdk).unwrap();
    fs::write(project.join("main.py"), "source").unwrap();
    fs::write(project.join(".env"), "private").unwrap();
    fs::write(sdk.join("__init__.py"), "sdk").unwrap();
    #[cfg(windows)]
    let tools = BuildTools {
        python: "powershell.exe".into(),
        arguments: vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/build_tool.ps1")
                .into_os_string(),
            root.as_os_str().to_owned(),
            "ok".into(),
        ],
    };
    #[cfg(unix)]
    let tools = BuildTools {
        python: "sh".into(),
        arguments: vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/build_tool.sh")
                .into_os_string(),
            root.as_os_str().to_owned(),
            "ok".into(),
        ],
    };
    let options = ArtifactOptions {
        build: BuildOptions {
            sdk_source: Some(sdk),
            package_spec: None,
            ..BuildOptions::default()
        },
        tools,
    };
    let mut app = AppDefinition::new("demo", "default").unwrap();
    for name in ["zeta", "alpha"] {
        let mut spec = ResourceSpec::new(ResourceKind::Queue, name);
        spec.cpu = Some("cpu3c-1-2".into());
        spec.env
            .get_or_insert_default()
            .insert("FLASH_ENVIRONMENT".into(), json!("stale"));
        app.resources.push(ResourceDefinition {
            spec,
            symbol: None,
            referenced_secrets: Vec::new(),
        });
    }
    (app, options)
}
fn archive(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    tar.entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (name, bytes)
        })
        .collect()
}
fn client(server: &MockServer) -> Client {
    Client::with_config(ClientConfig {
        api_base_url: server.uri(),
        ..ClientConfig::new("api-key")
    })
    .unwrap()
}

struct Backend {
    server: MockServer,
    operations: Arc<Mutex<Vec<String>>>,
    inputs: Arc<Mutex<Vec<Value>>>,
    uploaded: Arc<Mutex<Vec<u8>>>,
}
impl Backend {
    async fn new(existing: bool, failure: Option<&'static str>) -> Self {
        let server = MockServer::start().await;
        let operations = Arc::new(Mutex::new(Vec::new()));
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let uploaded = Arc::new(Mutex::new(Vec::new()));
        let ops = operations.clone();
        let body_log = inputs.clone();
        let upload_url = format!("{}/upload", server.uri());
        Mock::given(path("/graphql")).respond_with(move |request:&wiremock::Request|{
            let body:Value=request.body_json().unwrap();let query=body["query"].as_str().unwrap();
            let (field,value)=if query.contains("flashAppByName") {("flashAppByName",if existing {json!({"id":"app-1","flashEnvironments":[{"id":"env-1","name":"prod","endpoints":[]}]})} else {Value::Null})}
            else if query.contains("createFlashApp") {("createFlashApp",json!({"id":"app-1"}))}
            else if query.contains("createFlashEnvironment") {("createFlashEnvironment",json!({"id":"env-1","name":"prod","endpoints":[]}))}
            else if query.contains("prepareFlashArtifactUpload") {("prepareFlashArtifactUpload",json!({"uploadUrl":upload_url,"objectKey":"object-key"}))}
            else if query.contains("finalizeFlashArtifactUpload") {("finalizeFlashArtifactUpload",json!({"id":"build-1"}))}
            else if query.contains("deployBuildToEnvironment") {("deployBuildToEnvironment",json!({"id":"env-1"}))}
            else if query.contains("saveEndpoint") {("saveEndpoint",json!({"id":format!("ep-{}",body["variables"]["input"]["name"].as_str().unwrap())}))}
            else {panic!("unexpected query {query}")};
            ops.lock().unwrap().push(field.into());body_log.lock().unwrap().push(json!({"operation":field,"variables":body["variables"]}));
            if failure==Some(field) {ResponseTemplate::new(200).set_body_json(json!({"errors":[{"message":"forced failure"}]}))} else {ResponseTemplate::new(200).set_body_json(json!({"data":{field:value}}))}
        }).mount(&server).await;
        let ops = operations.clone();
        let uploaded_body = uploaded.clone();
        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(move |request: &wiremock::Request| {
                assert!(!request.headers.contains_key("authorization"));
                assert_eq!(request.headers["content-type"], "application/gzip");
                ops.lock().unwrap().push("upload".into());
                uploaded_body.lock().unwrap().clone_from(&request.body);
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;
        Self {
            server,
            operations,
            inputs,
            uploaded,
        }
    }
}

#[tokio::test]
async fn test_deploy_app_full_flow() {
    let backend = Backend::new(false, None).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, artifact) = fixture(temp.path());
    let events = Arc::new(Mutex::new(Vec::new()));
    let record = events.clone();
    let sink = EventSink::new(move |name, args| {
        record
            .lock()
            .unwrap()
            .push((name.to_owned(), args.to_vec()));
        Ok(())
    });
    let result = deploy_app(
        &client(&backend.server),
        &app,
        &temp.path().join("project"),
        &DeployOptions {
            environment: Some("prod".into()),
            artifact,
        },
        Some(&sink),
    )
    .await
    .unwrap();
    assert_eq!(result.build_id, "build-1");
    assert_eq!(result.environment_id, "env-1");
    assert_eq!(result.resources, ["zeta", "alpha"]);
    assert_eq!(result.endpoints["alpha"], "ep-alpha");
    assert_eq!(
        *backend.operations.lock().unwrap(),
        [
            "flashAppByName",
            "createFlashApp",
            "createFlashEnvironment",
            "prepareFlashArtifactUpload",
            "upload",
            "finalizeFlashArtifactUpload",
            "deployBuildToEnvironment",
            "saveEndpoint",
            "saveEndpoint"
        ]
    );
    let files = archive(&backend.uploaded.lock().unwrap());
    assert_eq!(files["main.py"], b"source");
    assert_eq!(files["env/runpod/__init__.py"], b"sdk");
    assert!(!files.contains_key(".env"));
    let manifest: Value = serde_json::from_slice(&files["runpod_manifest.json"]).unwrap();
    let inputs = backend.inputs.lock().unwrap();
    let finalized = inputs
        .iter()
        .find(|v| v["operation"] == "finalizeFlashArtifactUpload")
        .unwrap();
    assert_eq!(finalized["variables"]["input"]["manifest"], manifest);
    let prepare = inputs
        .iter()
        .find(|v| v["operation"] == "prepareFlashArtifactUpload")
        .unwrap();
    assert_eq!(
        prepare["variables"]["input"]["tarballSize"],
        backend.uploaded.lock().unwrap().len()
    );
    let events = events.lock().unwrap();
    let phases: Vec<_> = events
        .iter()
        .filter(|(name, _)| name == "phase")
        .map(|(_, args)| args[0].as_str().unwrap())
        .collect();
    assert_eq!(phases, ["vendor", "package", "upload", "endpoints"]);
    let ready: Vec<_> = events
        .iter()
        .filter(|(name, _)| name == "endpoint_ready")
        .map(|(_, args)| args[0].as_str().unwrap())
        .collect();
    assert_eq!(ready, ["alpha", "zeta"]);
    let (_, progress) = events
        .iter()
        .rfind(|(name, _)| name == "upload_progress")
        .unwrap();
    assert_eq!(progress[0], progress[1]);
}

#[tokio::test]
async fn test_deploy_reuses_existing_app_and_env_and_propagates_environment_for_nested_calls() {
    let backend = Backend::new(true, None).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, artifact) = fixture(temp.path());
    deploy_app(
        &client(&backend.server),
        &app,
        &temp.path().join("project"),
        &DeployOptions {
            environment: Some("prod".into()),
            artifact,
        },
        None,
    )
    .await
    .unwrap();
    let ops = backend.operations.lock().unwrap();
    assert!(!ops.contains(&"createFlashApp".into()));
    assert!(!ops.contains(&"createFlashEnvironment".into()));
    for input in backend
        .inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|v| v["operation"] == "saveEndpoint")
    {
        let env = input["variables"]["input"]["template"]["env"]
            .as_array()
            .unwrap();
        assert!(env.contains(&json!({"key":"FLASH_ENVIRONMENT","value":"prod"})));
        assert!(env.contains(&json!({"key":"RUNPOD_API_KEY","value":"api-key"})));
    }
}

#[tokio::test]
async fn test_invalid_mutated_spec_fails_before_remote_deployment_or_local_build() {
    let backend = Backend::new(false, None).await;
    let temp = tempfile::tempdir().unwrap();
    let (mut app, artifact) = fixture(temp.path());
    app.resources[0].spec.workers = Some(runpod::apps::Workers::Range([5, 1]));
    assert!(
        deploy_app(
            &client(&backend.server),
            &app,
            &temp.path().join("project"),
            &DeployOptions {
                environment: None,
                artifact
            },
            None
        )
        .await
        .is_err()
    );
    assert!(backend.operations.lock().unwrap().is_empty());
    assert!(!temp.path().join("commands.txt").exists());
}

#[tokio::test]
async fn explicit_secret_references_fail_preflight_but_plain_templates_remain_plain() {
    let server = MockServer::start().await;
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"myself":{"secrets":[]}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let temp = tempfile::tempdir().unwrap();
    let (mut app, artifact) = fixture(temp.path());
    let reference = Secret::new("TOKEN").unwrap();
    app.resources[0].set_environment(&[
        ("LITERAL".into(), EnvValue::Text(reference.reference())),
        ("TOKEN".into(), reference.into()),
    ]);
    assert_eq!(app.resources[0].referenced_secrets, ["TOKEN"]);
    let error = deploy_app(
        &client(&server),
        &app,
        &temp.path().join("project"),
        &DeployOptions {
            environment: None,
            artifact,
        },
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("secret not found: TOKEN"));
    assert!(!temp.path().join("commands.txt").exists());
}

#[tokio::test]
async fn failed_finalize_or_activation_never_provisions_endpoints() {
    for phase in ["finalizeFlashArtifactUpload", "deployBuildToEnvironment"] {
        let backend = Backend::new(true, Some(phase)).await;
        let temp = tempfile::tempdir().unwrap();
        let (app, artifact) = fixture(temp.path());
        assert!(
            deploy_app(
                &client(&backend.server),
                &app,
                &temp.path().join("project"),
                &DeployOptions {
                    environment: Some("prod".into()),
                    artifact
                },
                None
            )
            .await
            .is_err()
        );
        let ops = backend.operations.lock().unwrap();
        assert_eq!(ops.last().unwrap(), phase);
        assert!(!ops.contains(&"saveEndpoint".into()));
    }
}

#[test]
fn standalone_artifact_retains_explicit_output_and_omits_build_scratch() {
    let temp = tempfile::tempdir().unwrap();
    let (app, options) = fixture(temp.path());
    let output = temp.path().join("artifact.tar.gz");
    let artifact = build_artifact(
        &app,
        &temp.path().join("project"),
        &options,
        Some(&output),
        None,
    )
    .unwrap();
    let files = archive(&fs::read(artifact.path()).unwrap());
    assert!(files.contains_key("runpod_manifest.json"));
    assert!(files.contains_key("env/runpod/__init__.py"));
    drop(artifact);
    assert!(output.exists());
}

#[test]
fn explicit_source_paths_resolve_modules_and_fall_back_outside_the_project() {
    use runpod::apps::deploy::module_path_for;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir_all(root.join("pkg")).unwrap();
    for name in ["main.py", "__init__.py", "pkg/worker.py", "pkg/__init__.py"] {
        fs::write(root.join(name), "").unwrap();
    }
    fs::write(temp.path().join("other.py"), "").unwrap();
    for (name, expected) in [
        ("main.py", "main"),
        ("__init__.py", "fallback"),
        ("pkg/worker.py", "pkg.worker"),
        ("pkg/__init__.py", "pkg"),
    ] {
        assert_eq!(
            module_path_for(Some(&root.join(name)), &root, "fallback"),
            expected
        );
    }
    assert_eq!(
        module_path_for(Some(&temp.path().join("other.py")), &root, "fallback"),
        "fallback"
    );
    assert_eq!(module_path_for(None, &root, "fallback"), "fallback");
}

#[tokio::test]
async fn blocking_build_yields_to_the_async_executor() {
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Duration,
    };
    let backend = Backend::new(true, None).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, mut artifact) = fixture(temp.path());
    *artifact.tools.arguments.last_mut().unwrap() = "delay".into();
    let building = Arc::new(AtomicBool::new(false));
    let ticks = Arc::new(AtomicUsize::new(0));
    let flag = building.clone();
    let count = ticks.clone();
    let ticker = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            if flag.load(Ordering::SeqCst) {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let flag = building.clone();
    let count = ticks.clone();
    let sink = EventSink::new(move |name, args| {
        if name == "phase" {
            if args[0] == "vendor" {
                flag.store(true, Ordering::SeqCst);
            } else if args[0] == "package" {
                assert!(
                    count.load(Ordering::SeqCst) > 0,
                    "build blocked the async executor"
                );
                flag.store(false, Ordering::SeqCst);
            }
        }
        Ok(())
    });
    let result = deploy_app(
        &client(&backend.server),
        &app,
        &temp.path().join("project"),
        &DeployOptions {
            environment: Some("prod".into()),
            artifact,
        },
        Some(&sink),
    )
    .await;
    ticker.abort();
    result.unwrap();
}

#[tokio::test]
async fn canceling_the_build_await_prevents_cloud_operations() {
    use std::time::Duration;
    let backend = Backend::new(true, None).await;
    let temp = Arc::new(tempfile::tempdir().unwrap());
    let (app, mut artifact) = fixture(temp.path());
    *artifact.tools.arguments.last_mut().unwrap() = "delay".into();
    let progress = Arc::new(tokio::sync::Notify::new());
    let packaged = Arc::new(tokio::sync::Notify::new());
    let p = progress.clone();
    let done = packaged.clone();
    let keep_project = temp.clone();
    let sink = EventSink::new(move |name, args| {
        let _keep_alive = &keep_project;
        if name == "vendor_progress" {
            p.notify_one();
        }
        if name == "phase" && args[0] == "package" {
            done.notify_one();
        }
        Ok(())
    });
    let client = client(&backend.server);
    let root = temp.path().join("project");
    let task = tokio::spawn(async move {
        deploy_app(
            &client,
            &app,
            &root,
            &DeployOptions {
                environment: Some("prod".into()),
                artifact,
            },
            Some(&sink),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), progress.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    // The already-running synchronous installer finishes locally after its await is dropped.
    tokio::time::timeout(Duration::from_secs(10), packaged.notified())
        .await
        .unwrap();
    assert!(backend.operations.lock().unwrap().is_empty());
}

// These header-only bytes exercise deployment preflight, not executable validity.
fn linux_header() -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
    bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
    bytes
}
fn native_fixture(root: &Path) -> (AppDefinition, std::path::PathBuf) {
    let (mut app, _) = fixture(root);
    for resource in &mut app.resources {
        resource.symbol = Some(runpod::apps::deploy::SymbolReference {
            module: "worker".into(),
            qualname: resource.spec.name.clone(),
        });
    }
    app.resources[0].spec.image = Some("example/native:stable".into());
    app.resources[1].spec.kind = ResourceKind::Api;
    app.resources[1]
        .spec
        .env
        .get_or_insert_default()
        .insert("RUNPOD_RUNTIME_KIND".into(), json!("stale"));
    let binary = root.join("worker");
    fs::write(&binary, linux_header()).unwrap();
    (app, binary)
}

#[tokio::test]
async fn native_deployment_uploads_exact_manifest_and_starts_both_endpoint_kinds() {
    use runpod::apps::deploy::{NATIVE_WORKER_PATH, deploy_native_app};
    let backend = Backend::new(false, None).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, binary) = native_fixture(temp.path());
    let phases = Arc::new(Mutex::new(Vec::new()));
    let recorded = phases.clone();
    let sink = EventSink::new(move |name, args| {
        if name == "phase" {
            recorded.lock().unwrap().push(args[0].clone());
        }
        Ok(())
    });
    let result = deploy_native_app(
        &client(&backend.server),
        &app,
        &temp.path().join("project"),
        &binary,
        Some("prod"),
        Some(&sink),
    )
    .await
    .unwrap();
    assert_eq!(result.build_id, "build-1");
    assert_eq!(result.resources, ["zeta", "alpha"]);
    assert_eq!(result.endpoints["alpha"], "ep-alpha");
    assert_eq!(
        *backend.operations.lock().unwrap(),
        [
            "flashAppByName",
            "createFlashApp",
            "createFlashEnvironment",
            "prepareFlashArtifactUpload",
            "upload",
            "finalizeFlashArtifactUpload",
            "deployBuildToEnvironment",
            "saveEndpoint",
            "saveEndpoint"
        ]
    );
    let files = archive(&backend.uploaded.lock().unwrap());
    assert_eq!(files[NATIVE_WORKER_PATH], linux_header());
    assert_eq!(files["main.py"], b"source");
    assert!(!files.contains_key(".env"));
    assert!(!files.keys().any(|key| key.starts_with("env/")));
    let manifest: Value = serde_json::from_slice(&files["runpod_manifest.json"]).unwrap();
    // The coordinator v1 field selects its existing base-image family, not an interpreter invocation.
    assert_eq!(manifest["pythonVersion"], "3.12");
    assert!(manifest.get("excludedPackages").is_none());
    let inputs = backend.inputs.lock().unwrap();
    let finalized = inputs
        .iter()
        .find(|v| v["operation"] == "finalizeFlashArtifactUpload")
        .unwrap();
    assert_eq!(finalized["variables"]["input"]["manifest"], manifest);
    for input in inputs.iter().filter(|v| v["operation"] == "saveEndpoint") {
        let value = &input["variables"]["input"];
        let template = &value["template"];
        let kind = if value["name"] == "alpha" {
            "api"
        } else {
            "queue"
        };
        assert_eq!(
            template["dockerArgs"],
            runpod::apps::shim::native_runtime_launcher()
        );
        assert!(!template["dockerArgs"].as_str().unwrap().contains("python"));
        if kind == "queue" {
            assert_eq!(template["imageName"], "example/native:stable");
        } else {
            assert!(
                template["imageName"]
                    .as_str()
                    .unwrap()
                    .starts_with("runpod/api:py3.12-")
            );
        }
        let env = template["env"].as_array().unwrap();
        assert_eq!(
            env.iter()
                .filter(|e| e["key"] == "RUNPOD_RUNTIME_KIND")
                .count(),
            1
        );
        assert!(env.contains(&json!({"key":"RUNPOD_RUNTIME_KIND", "value":kind})));
        assert!(env.contains(&json!({"key":"FLASH_ENVIRONMENT", "value":"prod"})));
        assert!(env.contains(&json!({"key":"RUNPOD_API_KEY", "value":"api-key"})));
    }
    assert_eq!(
        *phases.lock().unwrap(),
        [json!("package"), json!("upload"), json!("endpoints")]
    );
    assert!(!temp.path().join("commands.txt").exists());
}

#[tokio::test]
async fn native_preflight_rejects_tasks_missing_symbols_and_wrong_binary_targets() {
    use runpod::apps::deploy::deploy_native_app;
    let backend = Backend::new(false, None).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, binary) = native_fixture(temp.path());
    let client = client(&backend.server);
    let root = temp.path().join("project");
    let mut task_app = app.clone();
    task_app.resources[0].spec.kind = ResourceKind::Task;
    let error = deploy_native_app(&client, &task_app, &root, &binary, None, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("native task deployment"));
    let mut no_symbol = app.clone();
    no_symbol.resources[0].symbol = None;
    assert!(
        deploy_native_app(&client, &no_symbol, &root, &binary, None, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("symbol")
    );
    let mut invalid = vec![vec![], b"MZ-Windows".to_vec(), b"#! /bin/sh".to_vec()];
    for (index, value) in [
        (4, 1),
        (5, 2),
        (6, 0),
        (7, 9),
        (16, 1),
        (18, 183),
        (20, 0),
        (52, 32),
    ] {
        let mut bytes = linux_header();
        bytes[index] = value;
        invalid.push(bytes);
    }
    for bytes in invalid {
        fs::write(&binary, bytes).unwrap();
        let error = deploy_native_app(&client, &app, &root, &binary, None, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Linux x86_64 ELF"), "{error}");
    }
    assert!(backend.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_secret_validation_precedes_packaging_and_remote_mutation() {
    let server = MockServer::start().await;
    Mock::given(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data":{"myself":{"secrets":[]}}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let temp = tempfile::tempdir().unwrap();
    let (mut app, binary) = native_fixture(temp.path());
    app.resources[0].set_environment(&[("TOKEN".into(), Secret::new("TOKEN").unwrap().into())]);
    let sink = EventSink::new(|_, _| panic!("packaging must not start before secret validation"));
    let error = runpod::apps::deploy::deploy_native_app(
        &client(&server),
        &app,
        &temp.path().join("project"),
        &binary,
        None,
        Some(&sink),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("secret not found: TOKEN"));
}

#[tokio::test]
async fn native_finalization_failure_stops_before_activation_and_provisioning() {
    let backend = Backend::new(true, Some("finalizeFlashArtifactUpload")).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, binary) = native_fixture(temp.path());
    assert!(
        runpod::apps::deploy::deploy_native_app(
            &client(&backend.server),
            &app,
            &temp.path().join("project"),
            &binary,
            Some("prod"),
            None
        )
        .await
        .is_err()
    );
    let ops = backend.operations.lock().unwrap();
    assert_eq!(ops.last().unwrap(), "finalizeFlashArtifactUpload");
    assert!(!ops.contains(&"deployBuildToEnvironment".into()));
    assert!(!ops.contains(&"saveEndpoint".into()));
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "executed from the uploaded native artifact by its parent test"]
fn native_deployed_worker_child() {
    if std::env::var("RUNPOD_NATIVE_DEPLOY_CHILD").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(runpod::apps::bootstrap::start_deployed(
            |module, context| {
                use runpod::apps::{App, AppRegistry, NativeApi, NativeFunction, RouteSpec};
                assert_eq!(module, "worker");
                let root = context.app_dir().to_owned();
                let app = App::new("demo")?;
                let mut api = NativeApi::new("alpha");
                api.route(
                    RouteSpec {
                        method: "GET".into(),
                        path: "/asset".into(),
                        handler_name: "asset".into(),
                    },
                    NativeFunction::sync(move |_| {
                        Ok(json!({"asset":fs::read_to_string(root.join("main.py"))?}))
                    }),
                )?;
                api.set_symbol(runpod::apps::deploy::SymbolReference {
                    module: "worker".into(),
                    qualname: "alpha".into(),
                });
                app.api(ResourceSpec::default(), api)?;
                let mut registry = AppRegistry::new();
                registry.register(app);
                Ok(registry)
            },
        ))
        .unwrap();
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[tokio::test]
async fn uploaded_linux_worker_runs_through_endpoint_launcher_and_rust_bootstrap() {
    use std::{process::Stdio, time::Duration};
    let backend = Backend::new(true, None).await;
    let temp = tempfile::tempdir().unwrap();
    let (app, _) = native_fixture(temp.path());
    runpod::apps::deploy::deploy_native_app(
        &client(&backend.server),
        &app,
        &temp.path().join("project"),
        &std::env::current_exe().unwrap(),
        Some("prod"),
        None,
    )
    .await
    .unwrap();
    let artifact = temp.path().join("uploaded artifact.tar.gz");
    fs::write(&artifact, &*backend.uploaded.lock().unwrap()).unwrap();
    let template = backend
        .inputs
        .lock()
        .unwrap()
        .iter()
        .find(|v| v["operation"] == "saveEndpoint" && v["variables"]["input"]["name"] == "alpha")
        .unwrap()["variables"]["input"]["template"]
        .clone();
    let launcher = template["dockerArgs"].as_str().unwrap();
    let script = launcher
        .strip_prefix("sh -c '")
        .unwrap()
        .strip_suffix('\'')
        .unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut command = tokio::process::Command::new("sh");
    command.args([
        "-c",
        script,
        "native-worker-test",
        "--exact",
        "native_deployed_worker_child",
        "--ignored",
        "--nocapture",
    ]);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("RUNPOD_") || name.starts_with("FLASH_") || name == "PORT" {
            command.env_remove(key);
        }
    }
    for entry in template["env"].as_array().unwrap() {
        command.env(
            entry["key"].as_str().unwrap(),
            entry["value"].as_str().unwrap(),
        );
    }
    let destination = temp.path().join("native app");
    let mut child = command
        .env("RUNPOD_NATIVE_DEPLOY_CHILD", "1")
        .env("RUNPOD_APP_DIR", &destination)
        .env("FLASH_BUILD_ARTIFACT_PATH", &artifact)
        .env("RUNPOD_ARTIFACT_WAIT", "0")
        .env("PORT", port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(response) = http
                .get(format!("http://127.0.0.1:{port}/asset"))
                .send()
                .await
            {
                break response;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "uploaded worker exited before serving"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"asset":"source"})
    );
    assert!(destination.join(".rp-unpacked").is_file());
    assert!(destination.join("runpod-native/bootstrap").is_file());
    assert!(destination.join("runpod-native/worker").is_file());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

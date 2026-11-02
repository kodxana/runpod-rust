use runpod::{
    Error, Result, Value,
    apps::{
        App, AppRegistry, NativeApi, NativeFunction, ResourceSpec, RouteSpec,
        bootstrap::{BootstrapConfig, start_deployed},
        deploy::{AppDefinition, NATIVE_WORKER_PATH, SymbolReference, build_native_artifact},
    },
    json,
};
use std::{collections::BTreeMap, fs, io::Read, path::Path, process::Stdio, time::Duration};

fn declarations(root: &Path) -> Result<AppRegistry> {
    let app = App::new("packaged")?;
    let root = root.to_owned();
    let mut api = NativeApi::new("hello");
    api.route(
        RouteSpec {
            method: "GET".into(),
            path: "/asset".into(),
            handler_name: "asset".into(),
        },
        NativeFunction::sync(move |_| {
            Ok(json!({"asset":fs::read_to_string(root.join("payload.txt"))?}))
        }),
    )?;
    api.set_symbol(SymbolReference {
        module: "worker".into(),
        qualname: "hello".into(),
    });
    app.api(ResourceSpec::default(), api)?;
    let mut apps = AppRegistry::new();
    apps.register(app);
    Ok(apps)
}

fn definition(root: &Path) -> AppDefinition {
    declarations(root).unwrap().apps()[0].definition().unwrap()
}

fn files(path: &Path) -> (Vec<String>, BTreeMap<String, Vec<u8>>, u32) {
    let mut archive =
        tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(path).unwrap()));
    let mut names = Vec::new();
    let mut contents = BTreeMap::new();
    let mut mode = 0;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        assert!(entry.header().entry_type().is_file());
        let name = entry.path().unwrap().to_string_lossy().replace('\\', "/");
        if name == NATIVE_WORKER_PATH {
            mode = entry.header().mode().unwrap();
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        names.push(name.clone());
        assert!(
            contents.insert(name, bytes).is_none(),
            "duplicate archive member"
        );
    }
    (names, contents, mode)
}

#[test]
fn native_tarball_contains_worker_assets_and_manifest() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir_all(root.join("target/release")).unwrap();
    fs::create_dir_all(root.join("runpod-native")).unwrap();
    let executable = root.join("target/release/compiled-worker");
    fs::write(&executable, b"compiled\0worker\xff").unwrap();
    fs::write(root.join("target/cache.bin"), "build cache").unwrap();
    fs::write(root.join("runpod-native/worker"), "stale worker").unwrap();
    fs::write(root.join("runpod-native/extra"), "stale private file").unwrap();
    fs::write(root.join("payload.txt"), "asset contents").unwrap();
    fs::write(root.join(".env"), "secret").unwrap();
    fs::write(root.join("runpod_manifest.json"), "stale manifest").unwrap();
    fs::write(
        root.join(".runpodignore"),
        "!target/\n!runpod-native/\n!.env\n!runpod_manifest.json\n",
    )
    .unwrap();
    let app = definition(root);
    let artifact = build_native_artifact(&app, root, &executable, None, None).unwrap();
    let (names, contents, mode) = files(artifact.path());
    assert_eq!(contents[NATIVE_WORKER_PATH], b"compiled\0worker\xff");
    assert_eq!(contents["payload.txt"], b"asset contents");
    assert!(!names.iter().any(|name| name.starts_with("target/")
        || name == ".env"
        || name == "runpod-native/extra"));
    assert_eq!(mode & 0o777, 0o755);
    let manifest: Value = serde_json::from_slice(&contents["runpod_manifest.json"]).unwrap();
    assert_eq!(manifest["app"], "packaged");
    assert_eq!(manifest["resources"][0]["module"], "worker");
    assert_eq!(manifest["resources"][0]["qualname"], "hello");
    assert!(manifest.get("pythonVersion").is_none());
    assert!(manifest.get("excludedPackages").is_none());
    assert_eq!(
        &names[names.len() - 2..],
        &[NATIVE_WORKER_PATH, "runpod_manifest.json"]
    );
}

#[test]
fn native_source_ignores_keep_existing_package_contracts() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/package.json")).unwrap();
    for label in [
        "protected_cannot_be_unignored",
        "nested_precedence",
        "root_anchoring",
        "unicode_and_order",
    ] {
        let case = cases.iter().find(|case| case["name"] == label).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let project = scratch.path().join("project");
        fs::create_dir(&project).unwrap();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = project.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text.as_str().unwrap()).unwrap();
        }
        let binary = scratch.path().join("worker");
        fs::write(&binary, "compiled worker").unwrap();
        let artifact =
            build_native_artifact(&definition(&project), &project, &binary, None, None).unwrap();
        let (names, contents, _) = files(artifact.path());
        let names: Vec<_> = names
            .into_iter()
            .filter(|name| name != NATIVE_WORKER_PATH)
            .collect();
        assert_eq!(json!(names), case["names"], "{label}");
        for (name, encoded) in case["contents"].as_object().unwrap() {
            use base64::Engine;
            assert_eq!(
                base64::engine::general_purpose::STANDARD.encode(&contents[name]),
                encoded.as_str().unwrap(),
                "{label}: {name}"
            );
        }
    }
}

#[test]
fn explicit_worker_is_not_duplicated_as_source_and_output_is_retained() {
    let project = tempfile::tempdir().unwrap();
    let binary = project.path().join("compiled worker.bin");
    let output = project.path().join("bundle.bin");
    fs::write(&binary, "worker bytes").unwrap();
    let artifact = build_native_artifact(
        &definition(project.path()),
        project.path(),
        &binary,
        Some(&output),
        None,
    )
    .unwrap();
    let (names, _, _) = files(artifact.path());
    assert_eq!(names, [NATIVE_WORKER_PATH, "runpod_manifest.json"]);
    drop(artifact);
    assert!(output.is_file());
    assert_eq!(fs::read(&binary).unwrap(), b"worker bytes");
}

#[test]
fn invalid_native_inputs_leave_the_destination_and_executable_untouched() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let binary = root.join("worker");
    let output = root.join("output");
    fs::write(&binary, "worker bytes").unwrap();
    fs::write(&output, "keep output").unwrap();
    let app = definition(root);
    for invalid_root in [root.join("missing-root"), binary.clone()] {
        assert!(build_native_artifact(&app, &invalid_root, &binary, Some(&output), None).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"keep output");
    }
    for executable in [root.join("missing"), root.to_owned()] {
        assert!(build_native_artifact(&app, root, &executable, Some(&output), None).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"keep output");
    }
    let empty = root.join("empty");
    fs::write(&empty, "").unwrap();
    assert!(build_native_artifact(&app, root, &empty, Some(&output), None).is_err());
    assert!(build_native_artifact(&app, root, &binary, Some(&binary), None).is_err());
    let alias = root.join("worker-alias");
    fs::hard_link(&binary, &alias).unwrap();
    assert!(build_native_artifact(&app, root, &binary, Some(&alias), None).is_err());
    assert_eq!(fs::read(&binary).unwrap(), b"worker bytes");
    for symbol in [
        None,
        Some(SymbolReference {
            module: " ".into(),
            qualname: "hello".into(),
        }),
        Some(SymbolReference {
            module: "worker".into(),
            qualname: String::new(),
        }),
    ] {
        let mut invalid = app.clone();
        invalid.resources[0].symbol = symbol;
        assert!(build_native_artifact(&invalid, root, &binary, Some(&output), None).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"keep output");
    }
}

#[test]
fn automatic_native_artifacts_are_owned_and_packaging_emits_no_vendor_phase() {
    let project = tempfile::tempdir().unwrap();
    let binary = project.path().join("worker");
    fs::write(&binary, "worker").unwrap();
    let phases = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = phases.clone();
    let sink = runpod::apps::events::EventSink::new(move |name, args| {
        if name == "phase" {
            recorded.lock().unwrap().push(args[0].clone());
        }
        Ok(())
    });
    let artifact = build_native_artifact(
        &definition(project.path()),
        project.path(),
        &binary,
        None,
        Some(&sink),
    )
    .unwrap();
    let output = artifact.path().to_owned();
    drop(artifact);
    assert!(!output.exists());
    assert!(!output.parent().unwrap().exists());
    assert_eq!(*phases.lock().unwrap(), [json!("package")]);
}

#[test]
#[ignore = "executed from the extracted native artifact by its parent test"]
fn native_packaged_worker_child() {
    if std::env::var("RUNPOD_NATIVE_ARTIFACT_CHILD").as_deref() != Ok("1") {
        return;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(start_deployed(|module, context| {
            if module != "worker" {
                return Err(Error::InvalidInput(format!("unexpected module {module}")));
            }
            declarations(context.app_dir())
        }))
        .unwrap();
}

#[tokio::test]
async fn extracted_native_artifact_runs_its_compiled_worker_and_bundled_assets() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("payload.txt"), "packaged asset").unwrap();
    let artifact = build_native_artifact(
        &definition(project.path()),
        project.path(),
        &std::env::current_exe().unwrap(),
        None,
        None,
    )
    .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let root = BootstrapConfig {
        artifact_path: artifact.path().to_owned(),
        app_dir: destination.path().join("app"),
        artifact_wait: Duration::ZERO,
        prebuilt_app_dir: None,
    }
    .locate()
    .unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut command = tokio::process::Command::new(root.join(NATIVE_WORKER_PATH));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("RUNPOD_") || name.starts_with("FLASH_") || name == "PORT" {
            command.env_remove(key);
        }
    }
    let mut child = command
        .args([
            "--exact",
            "native_packaged_worker_child",
            "--ignored",
            "--nocapture",
        ])
        .env("RUNPOD_NATIVE_ARTIFACT_CHILD", "1")
        .env("RUNPOD_APP_DIR", &root)
        .env("RUNPOD_RUNTIME_KIND", "api")
        .env("FLASH_RESOURCE_NAME", "hello")
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
    let response = tokio::time::timeout(Duration::from_secs(10), async {
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
                "packaged worker exited before serving"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"asset":"packaged asset"})
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

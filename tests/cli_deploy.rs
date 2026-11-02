use runpod::{Value, json};
use std::{
    fs,
    io::Read,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn project(root: &Path, names: &[&str]) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"),"[package]\nname=\"cli-deploy-fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[profile.dev]\ndebug=0\nincremental=false\n").unwrap();
    let apps: Vec<_> = names
        .iter()
        .map(|name| {
            let mut app = runpod::apps::deploy::AppDefinition::new(*name, "default").unwrap();
            let mut spec =
                runpod::apps::ResourceSpec::new(runpod::apps::ResourceKind::Queue, "chat");
            spec.cpu = Some("cpu3c-1-2".into());
            app.resources
                .push(runpod::apps::deploy::ResourceDefinition {
                    spec,
                    symbol: Some(runpod::apps::deploy::SymbolReference {
                        module: "worker".into(),
                        qualname: "chat".into(),
                    }),
                    referenced_secrets: vec![],
                });
            app
        })
        .collect();
    let export = json!({"version":1,"apps":apps}).to_string();
    fs::write(root.join("src/main.rs"),format!(r##"fn main() {{ std::fs::write(std::env::var_os("RUNPOD_DISCOVERY_OUTPUT").unwrap(),r#"{export}"#).unwrap(); }}"##)).unwrap();
    fs::write(root.join("payload.txt"), "bundled asset").unwrap();
    // Header-only bytes for mock deployment; no remote executable is run by these tests.
    let mut elf = vec![0; 64];
    elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    elf[16] = 2;
    elf[18] = 62;
    elf[20] = 1;
    elf[52] = 64;
    fs::write(root.join("linux-worker"), elf).unwrap();
}
fn command(root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"));
    command
        .current_dir(root)
        .args(["flash", "deploy"])
        .env("RUNPOD_API_KEY", "fixture-key")
        .env("RUNPOD_CONFIG_FILE", root.join("no-config"))
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}
fn out(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}
fn err(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}
fn source_exit(name: &str, result: &std::process::Output) {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/deploy_cli.json")).unwrap();
    let case = cases
        .iter()
        .find(|case| case["name"] == format!("TestDeploy.{name}"))
        .unwrap();
    assert_eq!(result.status.code().map(i64::from), case["code"].as_i64());
    let source = case["stderr"].as_str().unwrap();
    if name == "test_missing_target" {
        assert!(source.ends_with("missing_dir does not exist\n"));
        assert!(err(result).ends_with("missing_dir does not exist\n"));
    } else {
        assert!(source.starts_with("Error: no runpod.App found."));
        assert!(err(result).starts_with("Error: no runpod.App found."));
    }
}
struct Backend {
    server: MockServer,
    requests: Arc<Mutex<Vec<Value>>>,
    uploaded: Arc<Mutex<Vec<Vec<u8>>>>,
}
impl Backend {
    async fn new(fail: bool) -> Self {
        let server = MockServer::start().await;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let uploaded = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let upload_url = format!("{}/upload", server.uri());
        Mock::given(path("/graphql")).respond_with(move |request:&wiremock::Request|{
            let body:Value=request.body_json().unwrap();let query=body["query"].as_str().unwrap();recorded.lock().unwrap().push(body.clone());
            let (key,value)=if query.contains("flashAppByName") {("flashAppByName",json!({"id":"app-1","flashEnvironments":[{"id":"env-1","name":"default","endpoints":[]}]}))}
                else if query.contains("createFlashEnvironment") {("createFlashEnvironment",json!({"id":"env-1","name":body["variables"]["input"]["name"],"endpoints":[]}))}
                else if query.contains("prepareFlashArtifactUpload") {("prepareFlashArtifactUpload",json!({"objectKey":"object","uploadUrl":upload_url}))}
                else if query.contains("finalizeFlashArtifactUpload") {("finalizeFlashArtifactUpload",json!({"id":"build-1"}))}
                else if query.contains("deployBuildToEnvironment") {("deployBuildToEnvironment",json!({"id":"env-1"}))}
                else if query.contains("saveEndpoint") {("saveEndpoint",json!({"id":"ep1"}))}
                else {panic!("unexpected query {query}")};
            ResponseTemplate::new(200).set_body_json(if fail && key=="finalizeFlashArtifactUpload" {json!({"errors":[{"message":"upload failed"}]})} else {json!({"data":{key:value}})})
        }).mount(&server).await;
        let recorded = uploaded.clone();
        Mock::given(method("PUT"))
            .and(path("/upload"))
            .respond_with(move |request: &wiremock::Request| {
                assert!(!request.headers.contains_key("authorization"));
                recorded.lock().unwrap().push(request.body.clone());
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;
        Self {
            server,
            requests,
            uploaded,
        }
    }
}
#[tokio::test]
async fn test_deploys_discovered_app() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), &["demo"]);
    let backend = Backend::new(false).await;
    let result = command(root.path())
        .args(["--worker", "linux-worker"])
        .env("RUNPOD_API_BASE_URL", backend.server.uri())
        .output()
        .await
        .unwrap();
    assert!(result.status.success(), "{}", err(&result));
    assert!(out(&result).contains("demo/default"));
    assert!(out(&result).contains("ep1"));
    assert_eq!(backend.uploaded.lock().unwrap().len(), 1);
    assert!(
        backend
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|v| v["query"].as_str().unwrap().contains("saveEndpoint"))
    );
}
#[tokio::test]
async fn test_env_flag() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), &["demo"]);
    let backend = Backend::new(false).await;
    let result = command(root.path())
        .args(["--worker", "linux-worker", "--env", "prod"])
        .env("RUNPOD_API_BASE_URL", backend.server.uri())
        .output()
        .await
        .unwrap();
    assert!(result.status.success(), "{}", err(&result));
    assert!(out(&result).contains("demo/prod"));
    let requests = backend.requests.lock().unwrap();
    let endpoint = requests
        .iter()
        .find(|v| v["query"].as_str().unwrap().contains("saveEndpoint"))
        .unwrap();
    assert!(
        endpoint["variables"]["input"]["template"]["env"]
            .as_array()
            .unwrap()
            .contains(&json!({"key":"FLASH_ENVIRONMENT","value":"prod"}))
    );
}
#[tokio::test]
async fn test_multi_app_plan() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), &["demo", "other"]);
    let backend = Backend::new(false).await;
    let result = command(root.path())
        .args(["--worker", "linux-worker"])
        .env("RUNPOD_API_BASE_URL", backend.server.uri())
        .output()
        .await
        .unwrap();
    assert!(result.status.success(), "{}", err(&result));
    let text = out(&result);
    assert!(text.contains("2 apps"));
    assert!(text.contains("demo/default"));
    assert!(text.contains("other/default"));
    assert_eq!(backend.uploaded.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn test_missing_target() {
    let root = tempfile::tempdir().unwrap();
    let result = command(root.path())
        .arg("missing_dir")
        .output()
        .await
        .unwrap();
    source_exit("test_missing_target", &result);
    assert!(err(&result).contains("does not exist"));
}
#[tokio::test]
async fn test_no_apps_found() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), &[]);
    let result = command(root.path()).output().await.unwrap();
    source_exit("test_no_apps_found", &result);
    assert!(err(&result).contains("no runpod.App found"));
}
#[tokio::test]
async fn test_engine_error_is_clean() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), &["demo", "other"]);
    let backend = Backend::new(true).await;
    let result = command(root.path())
        .args(["--worker", "linux-worker"])
        .env("RUNPOD_API_BASE_URL", backend.server.uri())
        .output()
        .await
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(err(&result).contains("upload failed"));
    assert!(!err(&result).contains("Traceback"));
    assert!(!out(&result).contains("is live"));
    assert_eq!(backend.uploaded.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn test_build_only() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), &["demo"]);
    let result = command(root.path())
        .args(["--worker", "linux-worker", "--build-only"])
        .env_remove("RUNPOD_API_KEY")
        .env("RUNPOD_API_BASE_URL", "http://127.0.0.1:1")
        .output()
        .await
        .unwrap();
    assert!(result.status.success(), "{}", err(&result));
    assert!(out(&result).contains("demo-artifact.tar.gz"));
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
        fs::File::open(root.path().join("demo-artifact.tar.gz")).unwrap(),
    ));
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == Path::new("payload.txt") {
            let mut content = String::new();
            entry.read_to_string(&mut content).unwrap();
            assert_eq!(content, "bundled asset");
            found = true;
        }
    }
    assert!(found);
}
#[tokio::test]
async fn native_and_python_specific_options_are_not_silently_ignored() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["--exclude", "torch"],
        vec!["--worker", "worker", "--profile", "dev"],
        vec!["--python-version", "3.12"],
        vec!["--runtime", "python", "--worker", "worker"],
        vec![
            "--runtime",
            "python",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
        ],
    ] {
        let result = command(root.path()).args(&args).output().await.unwrap();
        assert_eq!(result.status.code(), Some(2), "{args:?}: {}", err(&result));
        assert!(!err(&result).contains("no runpod.App"));
    }
}

#[path = "common/cargo_target.rs"]
mod cargo_target;
#[tokio::test]
async fn build_only_compiles_each_discovered_worker_and_supports_the_runpod_alias() {
    let root = tempfile::tempdir().unwrap();
    project(root.path(), &["demo", "other"]);
    let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_runpod"))
        .current_dir(root.path())
        .args([
            "flash",
            "deploy",
            "--build-only",
            "--target-triple",
            &cargo_target::host_target(),
            "--profile",
            "dev",
        ])
        .env_remove("RUNPOD_API_KEY")
        .env("RUNPOD_API_BASE_URL", "http://127.0.0.1:1")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(result.status.success(), "{}", err(&result));
    let text = out(&result);
    assert!(text.contains("demo-artifact.tar.gz"));
    assert!(text.contains("other-artifact.tar.gz"));
    assert_eq!(
        text.matches("compiling worker").count(),
        1,
        "one binary serves both apps"
    );
    let worker = root
        .path()
        .join("target")
        .join(cargo_target::host_target())
        .join("debug")
        .join(if cfg!(windows) {
            "cli-deploy-fixture.exe"
        } else {
            "cli-deploy-fixture"
        });
    let expected = fs::read(worker).unwrap();
    for name in ["demo", "other"] {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(root.path().join(format!("{name}-artifact.tar.gz"))).unwrap(),
        ));
        let mut found = false;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            assert!(!path.starts_with("target"));
            assert!(!path.to_string_lossy().ends_with("artifact.tar.gz"));
            if path == Path::new("runpod-native/worker") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, expected);
                found = true;
            }
        }
        assert!(found);
    }
}

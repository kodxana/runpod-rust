use runpod::apps::discovery::{DiscoveryError, discover_apps};
use std::{fs, path::Path};

#[test]
fn discovery_export_keeps_parent_environment_and_registry_isolated() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("apps.json");
    let before = std::env::var_os("RUNPOD_DISCOVERY_SCAN");
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "exporter_child", "--ignored", "--nocapture"])
        .env("RUNPOD_DISCOVERY_SCAN", "outer-scan")
        .env("RUNPOD_DISCOVERY_OUTPUT", &file)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let export: runpod::Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    assert_eq!(export["version"], 1);
    assert_eq!(export["apps"][0]["name"], "native");
    assert_eq!(export["apps"][0]["resources"][0]["spec"]["name"], "hello");
    assert!(!String::from_utf8_lossy(&result.stdout).contains("LOCAL ENTRYPOINT"));
    assert_eq!(std::env::var_os("RUNPOD_DISCOVERY_SCAN"), before);
}

#[tokio::test]
#[ignore = "child entry point exercised with an isolated environment by its parent"]
async fn exporter_child() {
    use runpod::apps::{
        App, AppConfig, AppRegistry, Context, ExecutionContext, NativeFunction, ResourceSpec,
        targets::FunctionDefinition,
    };
    let app = App::with_config(
        "native",
        AppConfig {
            context: Some(ExecutionContext {
                kind: Context::Worker,
                resource_name: Some("hello".into()),
                ..ExecutionContext::default()
            }),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let function = app
        .queue(
            ResourceSpec::default(),
            FunctionDefinition {
                name: "hello".into(),
                parameters: vec![],
                source: None,
            },
            NativeFunction::sync(|_| Ok(runpod::json!(42))),
        )
        .unwrap();
    let error = function
        .remote(vec![], serde_json::Map::new())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        runpod::Error::App(runpod::apps::AppError::DiscoveryInvocation(_))
    ));
    let mut registry = AppRegistry::new();
    registry.register(app);
    if runpod::apps::discovery::export_if_requested(&registry).unwrap() {
        return;
    }
    println!("LOCAL ENTRYPOINT");
}

fn project(root: &Path) {
    fs::create_dir_all(root.join("src/bin")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname=\"discovery-fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[profile.dev]\ndebug=0\nincremental=false\n").unwrap();
}
fn binary(root: &Path, name: &str, apps: &[&str], fail: bool) {
    let apps: Vec<_> = apps
        .iter()
        .map(|name| runpod::json!({"name":name,"environment":"default","resources":[]}))
        .collect();
    let envelope = runpod::json!({"version":1,"apps":apps}).to_string();
    let code = format!(
        r##"
fn main() {{
    assert_eq!(std::env::var("RUNPOD_DISCOVERY_SCAN").unwrap(), "1");
    std::fs::write(std::env::var_os("RUNPOD_DISCOVERY_OUTPUT").unwrap(), r#"{envelope}"#).unwrap();
    if {fail} {{ eprintln!("incomplete initialization"); std::process::exit(1); }}
}}
"##
    );
    fs::write(root.join("src/bin").join(format!("{name}.rs")), code).unwrap();
}

#[tokio::test]
async fn failed_targets_do_not_return_partial_apps_and_fresh_scans_replace_them() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path());
    binary(temp.path(), "a_broken", &["partial"], true);
    binary(
        temp.path(),
        "b_healthy",
        &["healthy", "healthy", "second"],
        false,
    );
    let apps = discover_apps(temp.path()).await.unwrap();
    assert_eq!(
        apps.iter()
            .map(|a| a.definition.name.as_str())
            .collect::<Vec<_>>(),
        ["healthy", "second"]
    );
    assert!(
        apps.iter()
            .all(|a| a.executable.is_file() && a.source.ends_with("b_healthy.rs"))
    );
    binary(temp.path(), "a_broken", &["recovered"], false);
    let apps = discover_apps(temp.path()).await.unwrap();
    assert_eq!(
        apps.iter()
            .map(|a| a.definition.name.as_str())
            .collect::<Vec<_>>(),
        ["recovered", "healthy", "second"]
    );
}

#[tokio::test]
async fn single_target_is_strict_and_failed_exports_never_survive_retry() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path());
    binary(temp.path(), "app", &["partial"], true);
    let source = temp.path().join("src/bin/app.rs");
    assert!(matches!(
        discover_apps(&source).await,
        Err(runpod::Error::Discovery(DiscoveryError::Load { .. }))
    ));
    binary(temp.path(), "app", &["recovered"], false);
    let apps = discover_apps(&source).await.unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].definition.name, "recovered");
    fs::write(&source, "fn main() {}\n").unwrap();
    assert!(
        discover_apps(&source).await.is_err(),
        "a previous successful export cannot be reused"
    );
}

#[tokio::test]
async fn all_failures_are_reported_and_empty_directories_have_no_apps() {
    let temp = tempfile::tempdir().unwrap();
    assert!(discover_apps(temp.path()).await.unwrap().is_empty());
    project(temp.path());
    binary(temp.path(), "broken", &["partial"], true);
    let error = discover_apps(temp.path()).await.unwrap_err();
    assert!(
        error.to_string().contains("incomplete initialization"),
        "{error}"
    );
    assert!(matches!(
        error,
        runpod::Error::Discovery(DiscoveryError::NoApps(_))
    ));
}

#[tokio::test]
async fn interrupted_target_stops_later_targets() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path());
    binary(temp.path(), "a_interrupted", &["partial"], true);
    let first = temp.path().join("src/bin/a_interrupted.rs");
    fs::write(
        &first,
        fs::read_to_string(&first)
            .unwrap()
            .replace("exit(1)", "exit(130)"),
    )
    .unwrap();
    fs::write(
        temp.path().join("src/bin/b_later.rs"),
        "fn main() { std::fs::write(\"later-started\", \"bad\").unwrap(); }",
    )
    .unwrap();
    assert!(matches!(
        discover_apps(temp.path()).await,
        Err(runpod::Error::Discovery(DiscoveryError::Interrupted))
    ));
    assert!(!temp.path().join("later-started").exists());
}

#[tokio::test]
async fn invalid_export_rejects_the_entire_target() {
    let temp = tempfile::tempdir().unwrap();
    project(temp.path());
    binary(temp.path(), "app", &["valid", ""], false);
    let source = temp.path().join("src/bin/app.rs");
    let error = discover_apps(&source).await.unwrap_err();
    assert!(
        error.to_string().contains("invalid app declaration"),
        "{error}"
    );
    binary(temp.path(), "app", &["valid"], false);
    fs::write(
        &source,
        fs::read_to_string(&source)
            .unwrap()
            .replace("\"version\":1", "\"version\":99"),
    )
    .unwrap();
    let error = discover_apps(&source).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported discovery export version 99"),
        "{error}"
    );
}

use super::*;
use crate::json;
use std::path::Path;

fn values(root: &Path, kind: &str) -> BTreeMap<String, String> {
    [
        ("RUNPOD_RUNTIME_KIND", kind.to_owned()),
        ("FLASH_APP_NAME", "example".into()),
        ("FLASH_ENVIRONMENT", "prod".into()),
        ("FLASH_RESOURCE_NAME", "work".into()),
        ("RUNPOD_BUILD_ID", "build".into()),
        (
            "RUNPOD_APP_DIR",
            root.join("unused").to_string_lossy().into_owned(),
        ),
        (
            "RUNPOD_PREBUILT_APP_DIR",
            root.to_string_lossy().into_owned(),
        ),
        (
            "FLASH_BUILD_ARTIFACT_PATH",
            root.join("missing.tar").to_string_lossy().into_owned(),
        ),
        ("RUNPOD_ARTIFACT_WAIT", "0".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
fn manifest(root: &Path, value: &crate::Value) {
    std::fs::write(root.join(super::super::MANIFEST_NAME), value.to_string()).unwrap();
}
fn missing_installer(root: &Path) -> SystemPackageInstaller {
    SystemPackageInstaller {
        program: root.join("missing-apt"),
        arguments: vec![],
    }
}

#[tokio::test]
async fn task_identity_precedes_artifact_location() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = values(root.path(), "task");
    settings.remove("RUNPOD_BUILD_ID");
    let error = prepare(&settings, &missing_installer(root.path()))
        .await
        .unwrap_err();
    assert!(matches!(error,Error::Bootstrap(error) if error.phase=="identity"));
    assert!(!root.path().join("unused").exists());
}

#[tokio::test]
async fn task_manifest_validation_precedes_system_installation() {
    for value in [
        json!([]),
        json!({"app":"wrong"}),
        json!({"app":"example","resources":[{"name":"work","kind":"queue","systemDependencies":["ffmpeg"]}]}),
    ] {
        let root = tempfile::tempdir().unwrap();
        manifest(root.path(), &value);
        let error = prepare(
            &values(root.path(), "task"),
            &missing_installer(root.path()),
        )
        .await
        .unwrap_err();
        assert!(matches!(error,Error::Bootstrap(error) if error.phase=="attach"));
    }
}

#[tokio::test]
async fn system_dependencies_are_installed_before_preparation_returns() {
    let root = tempfile::tempdir().unwrap();
    manifest(
        root.path(),
        &json!({"app":"example","resources":[{"name":"work","kind":"task","module":"user","qualname":"work","systemDependencies":["ffmpeg"]}]}),
    );
    let error = prepare(
        &values(root.path(), "task"),
        &missing_installer(root.path()),
    )
    .await
    .unwrap_err();
    assert!(matches!(error,Error::Bootstrap(error) if error.phase=="system"));
}

#[tokio::test]
async fn native_preparation_uses_prebuilt_directory_without_python_dependency_probes() {
    let root = tempfile::tempdir().unwrap();
    manifest(
        root.path(),
        &json!({"app":"example","resources":[{"name":"work","kind":"task","module":"user","qualname":"work"}],"excludedPackages":["not-a-native-package"]}),
    );
    let settings = values(root.path(), "task");
    assert_eq!(
        prepare(&settings, &missing_installer(root.path()))
            .await
            .unwrap(),
        root.path()
    );
    assert_eq!(
        settings["RUNPOD_APP_DIR"],
        root.path().join("unused").to_string_lossy()
    );
    assert!(!root.path().join(super::super::UNPACK_MARKER).exists());
}

#[tokio::test]
async fn deployed_entry_requires_resource_identity_in_queue_mode() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = values(root.path(), "queue");
    settings.remove("FLASH_RESOURCE_NAME");
    let error = prepare(&settings, &missing_installer(root.path()))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)));
}

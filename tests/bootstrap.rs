use base64::{Engine, engine::general_purpose::STANDARD};
use runpod::{
    Value,
    apps::bootstrap::{BootstrapConfig, PhaseError, dependency_paths, read_manifest},
    json,
};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

fn config(root: &Path) -> BootstrapConfig {
    BootstrapConfig {
        artifact_path: root.join("artifact.tar.gz"),
        app_dir: root.join("app"),
        prebuilt_app_dir: None,
        artifact_wait: Duration::ZERO,
    }
}

fn archive(path: &Path, files: &[(&str, &str)]) {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        fs::File::create(path).unwrap(),
        flate2::Compression::default(),
    ));
    for (name, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        tar.append_data(&mut header, name, text.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
}

#[test]
fn test_missing_artifact_fails_with_phase() {
    let root = tempfile::tempdir().unwrap();
    let error = config(root.path()).locate().unwrap_err();
    assert_eq!(error.phase, "locate");
    assert!(error.detail.contains("after 0s"));
}

#[test]
fn test_extracts_artifact() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    archive(&config.artifact_path, &[("main.rs", "fn main() {}")]);
    assert_eq!(config.locate().unwrap(), config.app_dir);
    assert_eq!(
        fs::read_to_string(config.app_dir.join("main.rs")).unwrap(),
        "fn main() {}"
    );
    assert!(
        fs::read_to_string(config.app_dir.join(".rp-unpacked"))
            .unwrap()
            .parse::<f64>()
            .unwrap()
            > 0.0
    );
}

#[test]
fn test_unpack_happens_once() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    archive(&config.artifact_path, &[("main.rs", "original")]);
    config.locate().unwrap();
    fs::remove_file(&config.artifact_path).unwrap();
    assert_eq!(config.locate().unwrap(), config.app_dir);
    assert_eq!(
        fs::read_to_string(config.app_dir.join("main.rs")).unwrap(),
        "original"
    );
}

#[test]
fn test_prebuilt_dir_skips_unpack() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let prebuilt = root.path().join("prebuilt");
    fs::create_dir(&prebuilt).unwrap();
    config.prebuilt_app_dir = Some(prebuilt.clone());
    assert_eq!(config.locate().unwrap(), prebuilt);
    assert!(!config.app_dir.exists());
}

#[test]
fn test_rejects_traversal() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/bootstrap.json")).unwrap();
    let case = fixtures["locate"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "traversal")
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    fs::write(
        &config.artifact_path,
        STANDARD.decode(case["archive"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(config.locate().unwrap_err().phase, "locate");
    assert!(!root.path().join("outside.txt").exists());
    assert!(!config.app_dir.join(".rp-unpacked").exists());
}

#[test]
fn test_env_dir_leads_path_order() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("env")).unwrap();
    assert_eq!(
        dependency_paths(root.path()),
        [root.path().join("env"), root.path().to_owned()]
    );
}
#[test]
fn test_no_env_dir_falls_back_to_source_only() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(dependency_paths(root.path()), [root.path().to_owned()]);
}
#[test]
fn test_manifest_read() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("runpod_manifest.json"),
        r#"{"resources":[{"name":"q1"}]}"#,
    )
    .unwrap();
    assert_eq!(
        read_manifest(root.path()).unwrap()["resources"][0]["name"],
        "q1"
    );
}
#[test]
fn test_manifest_missing_fails_with_phase() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(read_manifest(root.path()).unwrap_err().phase, "attach");
}
#[test]
fn test_error_payload_shape() {
    assert_eq!(
        PhaseError::new("verify", "missing torch").payload(),
        json!({
            "error_type":"BootstrapError", "error_message":"worker bootstrap failed during 'verify': missing torch"
        })
    );
}

fn snapshot(root: &Path, directory: &Path, output: &mut BTreeMap<String, String>) {
    if !directory.is_dir() {
        return;
    }
    for item in fs::read_dir(directory).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            snapshot(root, &path, output);
        } else if path.file_name().unwrap() != ".rp-unpacked" {
            output.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                fs::read_to_string(&path).unwrap(),
            );
        }
    }
}

#[test]
fn runtime_differential_fixtures() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/bootstrap.json")).unwrap();
    for case in fixtures["locate"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let mut config = config(root.path());
        for directory in case["directories"].as_array().unwrap() {
            fs::create_dir_all(root.path().join(directory.as_str().unwrap())).unwrap();
        }
        for (name, data) in case["files"].as_object().unwrap() {
            let path = root.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, data.as_str().unwrap()).unwrap();
        }
        if let Some(raw) = case["archive"].as_str() {
            fs::write(&config.artifact_path, STANDARD.decode(raw).unwrap()).unwrap();
        }
        config.prebuilt_app_dir = case["prebuilt"].as_str().map(|path| root.path().join(path));
        let mut result = config.locate();
        if result.is_ok() && case["retry"] == true {
            fs::remove_file(&config.artifact_path).unwrap();
            result = config.locate();
        }
        assert_eq!(
            result.is_err(),
            !case["exception"].is_null(),
            "{}: {result:?}",
            case["name"]
        );
        match result {
            Ok(path) => assert_eq!(path, root.path().join(case["result"].as_str().unwrap())),
            Err(error) => {
                if let Some(phase) = case["phase"].as_str() {
                    assert_eq!(error.phase, phase);
                }
            }
        }
        let mut output = BTreeMap::new();
        snapshot(root.path(), &root.path().join("app"), &mut output);
        snapshot(root.path(), &root.path().join("prebuilt"), &mut output);
        assert_eq!(json!(output), case["output"], "{}", case["name"]);
        assert_eq!(
            json!(config.app_dir.join(".rp-unpacked").is_file()),
            case["marker"],
            "{}",
            case["name"]
        );
        assert!(!root.path().join("outside.txt").exists());
    }
    for case in fixtures["manifests"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        if let Some(text) = case["text"].as_str() {
            fs::write(root.path().join("runpod_manifest.json"), text).unwrap();
        }
        match read_manifest(root.path()) {
            Ok(value) => {
                assert!(case["phase"].is_null());
                assert_eq!(value, case["result"]);
            }
            Err(error) => assert_eq!(json!(error.phase), case["phase"]),
        }
    }
    for case in fixtures["errors"].as_array().unwrap() {
        let error = PhaseError::new(
            case["phase"].as_str().unwrap(),
            case["detail"].as_str().unwrap(),
        );
        assert_eq!(json!(error.to_string()), case["display"]);
        assert_eq!(error.payload(), case["payload"]);
    }
}

#[test]
fn configuration_uses_owned_environment_without_mutating_process() {
    let env = [
        ("RUNPOD_APP_DIR", "custom"),
        ("FLASH_BUILD_ARTIFACT_PATH", "archive"),
        ("RUNPOD_PREBUILT_APP_DIR", "ready"),
        ("RUNPOD_ARTIFACT_WAIT", "  +2 "),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect();
    let config = BootstrapConfig::from_values(&env).unwrap();
    assert_eq!(config.app_dir, PathBuf::from("custom"));
    assert_eq!(config.artifact_path, PathBuf::from("archive"));
    assert_eq!(config.prebuilt_app_dir, Some(PathBuf::from("ready")));
    assert_eq!(config.artifact_wait, Duration::from_secs(2));
    assert_eq!(
        BootstrapConfig::from_values(&[("RUNPOD_ARTIFACT_WAIT".into(), "-1".into())].into())
            .unwrap()
            .artifact_wait,
        Duration::ZERO
    );
    assert!(
        BootstrapConfig::from_values(&[("RUNPOD_ARTIFACT_WAIT".into(), "banana".into())].into())
            .is_err()
    );
}

#[test]
fn accepts_platform_artifact_created_by_sdk_packager() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("main.rs"), "fn main() {}").unwrap();
    let binary = [0x7f, b'E', b'L', b'F', 0, 0xff, 0x80, 1];
    fs::write(project.path().join("worker.bin"), binary).unwrap();
    fs::write(project.path().join(".env"), "private").unwrap();
    let dependencies = tempfile::tempdir().unwrap();
    fs::write(dependencies.path().join("dependency"), "ready").unwrap();
    let manifest = json!({"app":"native","resources":[{"name":"worker","kind":"queue"}]});
    let artifact = runpod::apps::deploy::package_project(
        project.path(),
        &manifest,
        &runpod::apps::deploy::PackageOptions {
            env_dir: Some(dependencies.path().to_owned()),
            output: None,
        },
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.artifact_path = artifact.path().to_owned();
    let app = config.locate().unwrap();
    assert_eq!(read_manifest(&app).unwrap(), manifest);
    assert_eq!(
        fs::read_to_string(app.join("main.rs")).unwrap(),
        "fn main() {}"
    );
    assert_eq!(
        fs::read_to_string(app.join("env/dependency")).unwrap(),
        "ready"
    );
    assert!(!app.join(".env").exists());
    assert_eq!(fs::read(app.join("worker.bin")).unwrap(), binary);
    assert_eq!(dependency_paths(&app), [app.join("env"), app]);
}

#[test]
fn malformed_manifest_retains_native_cause() {
    use std::error::Error;
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("runpod_manifest.json"), "bad").unwrap();
    let error = read_manifest(root.path()).unwrap_err();
    assert_eq!(error.phase, "attach");
    assert!(error.source().is_some());
    let error: runpod::Error = error.into();
    assert!(matches!(error, runpod::Error::Bootstrap(_)));
}

#[test]
fn rejects_nonportable_paths_links_and_special_members() {
    for (name, kind) in [
        ("../outside", b'0'),
        ("..\\outside", b'0'),
        ("C:/outside", b'0'),
        ("/absolute", b'0'),
        ("stream:alternate", b'0'),
        ("symlink", b'2'),
        ("hardlink", b'1'),
        ("device", b'3'),
        ("fifo", b'6'),
    ] {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        let mut builder = tar::Builder::new(fs::File::create(&config.artifact_path).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::new(kind));
        if kind == b'1' || kind == b'2' {
            header.set_link_name("target").unwrap();
        }
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        builder.append(&header, io::empty()).unwrap();
        builder.finish().unwrap();
        drop(builder);
        assert_eq!(config.locate().unwrap_err().phase, "locate", "{name}");
        assert!(!config.app_dir.join(".rp-unpacked").exists());
        assert!(!root.path().join("outside").exists());
    }
}

#[test]
fn waits_for_platform_to_publish_artifact() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.artifact_wait = Duration::from_secs(5);
    let path = config.artifact_path.clone();
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(25));
        archive(&path, &[("ready", "yes")]);
    });
    let result = config.locate();
    producer.join().unwrap();
    assert_eq!(
        fs::read_to_string(result.unwrap().join("ready")).unwrap(),
        "yes"
    );
}

#[cfg(unix)]
#[test]
fn existing_symlink_cannot_redirect_extraction_outside_root() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&config.app_dir).unwrap();
    std::os::unix::fs::symlink(&outside, config.app_dir.join("redirect")).unwrap();
    archive(&config.artifact_path, &[("redirect/escape", "bad")]);
    assert_eq!(config.locate().unwrap_err().phase, "locate");
    assert!(!outside.join("escape").exists());
}

#[cfg(unix)]
#[test]
fn data_filter_permissions_preserve_executable_without_privilege_bits() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let mut builder = tar::Builder::new(fs::File::create(&config.artifact_path).unwrap());
    for (name, mode) in [
        ("worker", 0o7777),
        ("readonly", 0o0000),
        ("otherexec", 0o0077),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(mode);
        header.set_mtime(0);
        builder.append_data(&mut header, name, &b"ok"[..]).unwrap();
    }
    builder.finish().unwrap();
    drop(builder);
    config.locate().unwrap();
    for (name, mode) in [("worker", 0o755), ("readonly", 0o600), ("otherexec", 0o644)] {
        assert_eq!(
            fs::metadata(config.app_dir.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            mode
        );
    }
}

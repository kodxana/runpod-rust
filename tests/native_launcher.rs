use runpod::apps::{
    deploy::{AppDefinition, build_native_artifact},
    shim::native_runtime_launcher,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

fn shell() -> PathBuf {
    if let Some(shell) = std::env::var_os("RUNPOD_TEST_POSIX_SHELL") {
        return shell.into();
    }
    #[cfg(unix)]
    {
        "sh".into()
    }
    #[cfg(windows)]
    {
        // Locate Git's POSIX tools relative to PATH, including portable Git installations.
        for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            for candidate in [
                directory.join("sh.exe"),
                directory.join("../bin/sh.exe"),
                directory.join("../usr/bin/sh.exe"),
            ] {
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        panic!("launcher tests require Git sh on PATH or RUNPOD_TEST_POSIX_SHELL");
    }
}
fn command(root: &Path) -> Command {
    let launcher = native_runtime_launcher();
    let script = launcher
        .strip_prefix("sh -c '")
        .unwrap()
        .strip_suffix('\'')
        .unwrap();
    assert!(
        !script.contains('\''),
        "dockerArgs must survive platform shell lexing"
    );
    let mut command = Command::new(shell());
    command.args([
        "-c",
        script,
        "native-launcher-test",
        "argument with spaces",
        "literal;$HOME",
    ]);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("RUNPOD_") || name.starts_with("FLASH_") {
            command.env_remove(key);
        }
    }
    command
        .env("RUNPOD_APP_DIR", root.join("application with spaces"))
        .env(
            "FLASH_BUILD_ARTIFACT_PATH",
            root.join("artifact with spaces.tar.gz"),
        )
        .env("RUNPOD_ARTIFACT_WAIT", "0");
    command
}
fn assert_output(output: Output, expected: &str) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        expected
    );
}
fn worker(path: &Path, identity: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!("#!/bin/sh\nprintf '%s\\n' {identity} \"$1\" \"$2\"\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn prebuilt_tree_takes_precedence_and_missing_worker_does_not_fall_back() {
    let root = tempfile::tempdir().unwrap();
    let prebuilt = root.path().join("prebuilt with spaces");
    let warm = root.path().join("application with spaces");
    worker(&prebuilt.join("runpod-native/worker"), "prebuilt");
    worker(&warm.join("runpod-native/worker"), "warm");
    fs::write(warm.join(".rp-unpacked"), "").unwrap();
    assert_output(
        command(root.path())
            .env("RUNPOD_PREBUILT_APP_DIR", &prebuilt)
            .output()
            .unwrap(),
        "prebuilt\nargument with spaces\nliteral;$HOME\n",
    );
    fs::remove_file(prebuilt.join("runpod-native/worker")).unwrap();
    let failed = command(root.path())
        .env("RUNPOD_PREBUILT_APP_DIR", &prebuilt)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("worker is missing"));
}

#[test]
fn warm_marker_uses_existing_worker_without_an_artifact() {
    let root = tempfile::tempdir().unwrap();
    let warm = root.path().join("application with spaces");
    worker(&warm.join("runpod-native/worker"), "warm");
    fs::write(warm.join(".rp-unpacked"), "").unwrap();
    assert_output(
        command(root.path())
            .env("RUNPOD_PREBUILT_APP_DIR", root.path().join("absent"))
            .output()
            .unwrap(),
        "warm\nargument with spaces\nliteral;$HOME\n",
    );
}

#[test]
fn cold_start_extracts_only_bootstrap_copy_and_preserves_arguments() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let binary = root.path().join("input-worker");
    worker(&binary, "cold");
    fs::write(source.join("payload.txt"), "asset").unwrap();
    let output = root.path().join("artifact with spaces.tar.gz");
    let artifact = build_native_artifact(
        &AppDefinition::new("demo", "default").unwrap(),
        &source,
        &binary,
        Some(&output),
        None,
    )
    .unwrap();
    assert_output(
        command(root.path()).output().unwrap(),
        "cold\nargument with spaces\nliteral;$HOME\n",
    );
    let app = root.path().join("application with spaces");
    assert_eq!(
        fs::read(app.join("runpod-native/bootstrap")).unwrap(),
        fs::read(binary).unwrap()
    );
    assert!(!app.join("runpod-native/worker").exists());
    assert!(!app.join("payload.txt").exists());
    assert!(!app.join(".rp-unpacked").exists());
    drop(artifact);
}

#[test]
fn missing_artifact_invalid_wait_and_incomplete_archives_fail_before_execution() {
    let root = tempfile::tempdir().unwrap();
    for wait in ["0", "-1", "bad", ""] {
        let output = command(root.path())
            .env("RUNPOD_ARTIFACT_WAIT", wait)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(if matches!(wait, "bad" | "") {
                "must be an integer"
            } else {
                "artifact not found"
            }),
            "{stderr}"
        );
    }
    let output = root.path().join("artifact with spaces.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        fs::File::create(output).unwrap(),
        flate2::Compression::fast(),
    );
    let mut archive = tar::Builder::new(encoder);
    archive.finish().unwrap();
    archive.into_inner().unwrap().finish().unwrap();
    let failed = command(root.path()).output().unwrap();
    assert!(!failed.status.success());
    assert_eq!(failed.stdout, [] as [u8; 0]);
}

#[test]
fn cold_start_waits_for_the_platform_to_mount_the_artifact() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let binary = root.path().join("input-worker");
    worker(&binary, "delayed");
    let artifact = build_native_artifact(
        &AppDefinition::new("demo", "default").unwrap(),
        &source,
        &binary,
        None,
        None,
    )
    .unwrap();
    let mut child = command(root.path())
        .env("RUNPOD_ARTIFACT_WAIT", "10")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(child.try_wait().unwrap().is_none());
    fs::copy(
        artifact.path(),
        root.path().join("artifact with spaces.tar.gz"),
    )
    .unwrap();
    assert_output(
        child.wait_with_output().unwrap(),
        "delayed\nargument with spaces\nliteral;$HOME\n",
    );
}

//! Exercise the real POSIX launcher without installing packages or starting Python.
#![cfg(unix)]
use runpod::apps::{ResourceKind, shim::runtime_launcher};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn launcher_starts_installed_runtimes_installs_missing_packages_and_honors_overrides() {
    for kind in [ResourceKind::Queue, ResourceKind::Api, ResourceKind::Task] {
        for (installed, runtime, sdk, fail) in [
            (false, "", "", false),
            (true, "", "", false),
            (true, "runtime==2.1", "runpod==3.2", false),
            (false, "", "", true),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let log = temp.path().join("calls");
            let interpreter = temp.path().join("python3");
            fs::write(
                &interpreter,
                r#"#!/bin/sh
if [ "$1" = "-c" ]; then [ "$SHIM_INSTALLED" = "yes" ]; exit $?; fi
if [ "$2" = "pip" ]; then
    printf '%s\n' "$6" >> "$SHIM_LOG"
    exit "$SHIM_PIP_EXIT"
fi
printf 'start:%s\n' "$2" >> "$SHIM_LOG"
"#,
            )
            .unwrap();
            fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o755)).unwrap();
            let launcher = runtime_launcher(kind);
            let script = launcher
                .strip_prefix("sh -c '")
                .unwrap()
                .strip_suffix('\'')
                .unwrap();
            assert!(!script.contains('\''));
            let result = Command::new("/bin/sh")
                .args(["-c", script])
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", temp.path().display()))
                .env("SHIM_LOG", &log)
                .env("SHIM_INSTALLED", if installed { "yes" } else { "no" })
                .env("SHIM_PIP_EXIT", if fail { "7" } else { "0" })
                .env("RUNPOD_RUNTIME_PACKAGE_SPEC", runtime)
                .env("RUNPOD_PACKAGE_SPEC", sdk)
                .output()
                .unwrap();
            let mut expected = Vec::<String>::new();
            if !installed || !runtime.is_empty() {
                expected.push(
                    if runtime.is_empty() {
                        "runpod-sdk-runtime"
                    } else {
                        runtime
                    }
                    .into(),
                );
            }
            if !fail {
                if !installed || !sdk.is_empty() {
                    expected.push(if sdk.is_empty() { "runpod" } else { sdk }.into());
                }
                expected.push(format!(
                    "start:{}",
                    if kind == ResourceKind::Task {
                        "runpod_sdk_runtime.task.runner"
                    } else {
                        "runpod_sdk_runtime.bootstrap"
                    }
                ));
            }
            assert_eq!(
                result.status.success(),
                !fail,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                fs::read_to_string(log).unwrap().lines().collect::<Vec<_>>(),
                expected
            );
        }
    }
}

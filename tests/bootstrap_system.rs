use runpod::{apps::bootstrap::SystemPackageInstaller, json};
use std::{fs, path::Path};

fn installer(root: &Path, mode: &str) -> SystemPackageInstaller {
    #[cfg(windows)]
    let (program, arguments) = (
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/system_tool.ps1")
                .into_os_string(),
            root.as_os_str().to_owned(),
            mode.into(),
        ],
    );
    #[cfg(unix)]
    let (program, arguments) = (
        "sh",
        vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/system_tool.sh")
                .into_os_string(),
            root.as_os_str().to_owned(),
            mode.into(),
        ],
    );
    SystemPackageInstaller {
        program: program.into(),
        arguments,
    }
}

#[tokio::test]
async fn test_no_system_deps_no_op() {
    let root = tempfile::tempdir().unwrap();
    let tool = installer(root.path(), "ok");
    for dependencies in [json!(null), json!([])] {
        tool.install(
            &json!({"resources":[{"name":"q","systemDependencies":dependencies}]}),
            "q",
        )
        .await
        .unwrap();
    }
    tool.install(&json!({"resources":[{"name":"q"}]}), "q")
        .await
        .unwrap();
    assert!(!root.path().join("commands.txt").exists());
}

#[tokio::test]
async fn test_missing_apt_fails_with_phase() {
    let root = tempfile::tempdir().unwrap();
    let tool = SystemPackageInstaller {
        program: root.path().join("missing-apt-program"),
        arguments: vec![],
    };
    let error = tool
        .install(
            &json!({"resources":[{"name":"q","systemDependencies":["ffmpeg"]}]}),
            "q",
        )
        .await
        .unwrap_err();
    assert_eq!(error.phase, "system");
    assert!(error.detail.contains("ffmpeg"));
    assert!(error.detail.contains("not available"));
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn test_installs_resource_system_deps() {
    let root = tempfile::tempdir().unwrap();
    installer(root.path(),"ok").install(&json!({"resources":[{"name":"q","systemDependencies":["ffmpeg","sox=1","lib name;$HOME"]}]}),"q").await.unwrap();
    let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
    assert_eq!(
        commands.lines().collect::<Vec<_>>(),
        [
            "noninteractive|update|-qq",
            "noninteractive|install|-y|-qq|--no-install-recommends|ffmpeg|sox=1|lib name;$HOME",
        ]
    );
}

#[tokio::test]
async fn test_other_resources_deps_ignored() {
    let root = tempfile::tempdir().unwrap();
    installer(root.path(), "ok")
        .install(
            &json!({"resources":[{"name":"other","systemDependencies":["ffmpeg"]}]}),
            "q",
        )
        .await
        .unwrap();
    assert!(!root.path().join("commands.txt").exists());
}

#[tokio::test]
async fn only_first_matching_resource_supplies_system_packages() {
    let root = tempfile::tempdir().unwrap();
    installer(root.path(),"ok").install(&json!({"resources":[{"name":"q","systemDependencies":["first"]},{"name":"q","systemDependencies":["second"]}]}),"q").await.unwrap();
    let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
    assert!(commands.contains("|first"));
    assert!(!commands.contains("second"));
}

#[tokio::test]
async fn update_failure_stops_install_and_failure_diagnostics_keep_stderr_tail() {
    for mode in ["update-fail", "install-fail"] {
        let root = tempfile::tempdir().unwrap();
        let error = installer(root.path(), mode)
            .install(
                &json!({"resources":[{"name":"q","systemDependencies":["ffmpeg"]}]}),
                "q",
            )
            .await
            .unwrap_err();
        assert_eq!(error.phase, "system");
        assert!(error.detail.ends_with("TAIL"));
        assert_eq!(error.detail.matches('x').count(), 1996);
        let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
        assert_eq!(
            commands.lines().count(),
            if mode == "update-fail" { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn source_system_package_captures() {
    let fixture: runpod::Value =
        serde_json::from_str(include_str!("fixtures/bootstrap_system.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let mode = case["mode"].as_str().unwrap();
        let tool = if mode == "missing" {
            SystemPackageInstaller {
                program: root.path().join("missing-apt"),
                arguments: vec![],
            }
        } else {
            installer(root.path(), mode)
        };
        let result = tool
            .install(&case["manifest"], case["resource"].as_str().unwrap())
            .await;
        let calls = fs::read_to_string(root.path().join("commands.txt")).unwrap_or_default();
        let expected: Vec<_> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| {
                format!(
                    "{}|{}",
                    call["frontend"].as_str().unwrap(),
                    call["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|arg| arg.as_str().unwrap())
                        .collect::<Vec<_>>()
                        .join("|")
                )
            })
            .collect();
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            expected,
            "{}",
            case["name"]
        );
        if case["error"].is_null() {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.phase, case["error"]["phase"].as_str().unwrap());
            if mode != "missing" {
                assert_eq!(
                    error.detail.replace("[\"ffmpeg\"]", "['ffmpeg']"),
                    case["error"]["detail"].as_str().unwrap()
                );
            }
        }
    }
}

#[tokio::test]
async fn dropping_installation_kills_the_active_package_manager() {
    use std::time::Duration;
    let root = tempfile::tempdir().unwrap();
    let tool = installer(root.path(), "hang");
    let task = tokio::spawn(async move {
        tool.install(
            &json!({"resources":[{"name":"q","systemDependencies":["ffmpeg"]}]}),
            "q",
        )
        .await
    });
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = fs::read_to_string(root.path().join("pid"))
                && let Ok(pid) = text.parse::<u32>()
            {
                break sysinfo::Pid::from_u32(pid);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut system = sysinfo::System::new();
        loop {
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            if system.process(pid).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("package manager survived cancellation");
    assert_eq!(
        fs::read_to_string(root.path().join("commands.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

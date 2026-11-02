use super::*;
use crate::{
    apps::deploy::{AppDefinition, ResourceDefinition},
    json,
};

#[test]
fn test_exclude_passthrough() {
    let args = command()
        .try_get_matches_from([
            "deploy",
            "--runtime",
            "python",
            "--exclude",
            "torch,numpy",
            "--python-version",
            "3.11",
        ])
        .unwrap();
    let options = Options::read(&args).unwrap();
    let Runtime::Python {
        options, version, ..
    } = options.runtime
    else {
        panic!("wrong artifact runtime")
    };
    assert_eq!(options.build.exclude, ["torch", "numpy"]);
    assert_eq!(version.as_deref(), Some("3.11"));
}
#[test]
fn test_python_version_fallback_warns() {
    assert_eq!(select_python_version(None, "3.9"), ("3.12".into(), true));
    assert_eq!(select_python_version(None, "3.13"), ("3.13".into(), false));
    assert_eq!(
        select_python_version(Some("3.11"), "3.9"),
        ("3.11".into(), false)
    );
}
#[test]
fn test_deploy_banner() {
    let mut app = AppDefinition::new("demo", "default").unwrap();
    app.resources.push(ResourceDefinition {
        spec: crate::apps::ResourceSpec::new(crate::apps::ResourceKind::Queue, "chat"),
        symbol: None,
        referenced_secrets: vec![],
    });
    let text = banner(&app, "prod", "main.rs");
    for word in ["demo", "prod", "chat", "main.rs"] {
        assert!(text.contains(word));
    }
}
#[test]
fn test_deploy_banner_no_resources() {
    assert!(
        banner(&AppDefinition::new("demo", "default").unwrap(), "prod", "")
            .contains("(no resources)")
    );
}
#[test]
fn test_phase_lifecycle() {
    let mut progress = Progress::default();
    progress.event("phase", &[json!("vendor")]);
    progress.event("vendor_progress", &[json!(3), json!("torch")]);
    progress.event("phase", &[json!("upload")]);
    progress.event("upload_progress", &[json!(512 * 1024), json!(1024 * 1024)]);
    progress.event("phase", &[json!("endpoints")]);
    progress.event("endpoint_ready", &[json!("chat"), json!("ep1")]);
    progress.close();
    assert_eq!(progress.endpoints.get("chat"), Some(&"ep1".to_owned()));
    let output = progress.render(0);
    assert!(output.contains("3 packages"));
    assert!(output.contains("1.0 MB"));
    assert!(!output.contains("torch"));
}
#[test]
fn test_progress_updates_ignored_outside_phase() {
    let mut progress = Progress::default();
    progress.event("vendor_progress", &[json!(1), json!("torch")]);
    progress.event("upload_progress", &[json!(1), json!(2)]);
    progress.close();
    assert_eq!(progress.render(0), "");
}
#[test]
fn test_bar_bounds() {
    assert_eq!(bar(-1.0).chars().count(), 22);
    assert!(bar(0.5).contains('\u{2501}'));
    assert_eq!(bar(2.0), "\u{2501}".repeat(22));
}

#[test]
fn source_deployment_arguments_and_banners_match_captured_cases() {
    let cases: Vec<crate::Value> =
        serde_json::from_str(include_str!("../../tests/fixtures/deploy_cli.json")).unwrap();
    assert_eq!(cases.len(), 9);
    for case in cases {
        let mut args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        args.extend(["--runtime", "python"]);
        let matches = command().try_get_matches_from(args).unwrap();
        let options = Options::read(&matches).unwrap();
        for call in case["calls"].as_array().unwrap() {
            assert_eq!(options.build_only, call["operation"] == "build_artifact");
            assert_eq!(crate::json!(options.environment), call["environment"]);
            let Runtime::Python {
                options: artifact,
                version,
            } = &options.runtime
            else {
                panic!("runtime mismatch")
            };
            let expected = call["exclude"]
                .as_array()
                .map(|v| {
                    v.iter()
                        .map(|p| p.as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            assert_eq!(artifact.build.exclude, expected);
            let local = if case["name"] == "TestDeploy.test_python_version_fallback_warns" {
                "3.9"
            } else {
                "3.13"
            };
            assert_eq!(
                select_python_version(version.as_deref(), local).0,
                call["python_version"]
            );
            if options.build_only {
                assert_eq!(
                    call["output"],
                    format!("{}-artifact.tar.gz", call["app"].as_str().unwrap())
                );
            } else {
                let mut app = AppDefinition::new(
                    call["app"].as_str().unwrap(),
                    call["app_environment"].as_str().unwrap(),
                )
                .unwrap();
                for entry in call["resources"].as_array().unwrap() {
                    app.resources.push(ResourceDefinition {
                        spec: crate::apps::ResourceSpec::new(
                            serde_json::from_value(entry["kind"].clone()).unwrap(),
                            entry["name"].as_str().unwrap(),
                        ),
                        symbol: None,
                        referenced_secrets: vec![],
                    });
                }
                let environment = options.environment.as_deref().unwrap_or(&app.environment);
                let text = banner(&app, environment, call["source"].as_str().unwrap());
                assert!(
                    case["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|message| message.as_str().unwrap() == text.trim_end()),
                    "{}: {text}",
                    case["name"]
                );
            }
        }
    }
}

#[tokio::test]
async fn python_artifact_mode_builds_with_selected_version_exclusions_and_fallback_warning() {
    use std::{fs, io::Read};
    for (version, mode, expected_version, warn) in [
        (Some("3.11"), "ok", "3.11", false),
        (None, "version-39", "3.12", true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("Cargo.toml"),"[package]\nname=\"python-artifact-cli-test\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[profile.dev]\ndebug=0\nincremental=false\n").unwrap();
        fs::write(project.join(".runpodignore"), "target/\n").unwrap();
        fs::write(project.join("requirements.txt"), "torch\nnumpy\n").unwrap();
        let mut app = AppDefinition::new("demo", "default").unwrap();
        let mut spec = crate::apps::ResourceSpec::new(crate::apps::ResourceKind::Queue, "chat");
        spec.cpu = Some("cpu3c-1-2".into());
        app.resources.push(ResourceDefinition {
            spec,
            symbol: Some(crate::apps::deploy::SymbolReference {
                module: "main".into(),
                qualname: "chat".into(),
            }),
            referenced_secrets: vec![],
        });
        let export = json!({"version":1,"apps":[app]}).to_string();
        fs::write(project.join("src/main.rs"),format!(r##"fn main() {{std::fs::write(std::env::var_os("RUNPOD_DISCOVERY_OUTPUT").unwrap(),r#"{export}"#).unwrap();}}"##)).unwrap();
        fs::write(project.join("main.py"), "def chat(prompt): return prompt\n").unwrap();
        let sdk = root.path().join("sdk");
        fs::create_dir(&sdk).unwrap();
        fs::write(sdk.join("__init__.py"), "sdk").unwrap();
        let mut args = vec![
            "deploy",
            "project",
            "--runtime",
            "python",
            "--build-only",
            "--exclude",
            "torch,numpy",
        ];
        if let Some(version) = version {
            args.extend(["--python-version", version]);
        }
        let mut options = Options::read(&command().try_get_matches_from(args).unwrap()).unwrap();
        let Runtime::Python {
            options: artifact, ..
        } = &mut options.runtime
        else {
            panic!("runtime mismatch")
        };
        artifact.build.sdk_source = Some(sdk);
        artifact.build.package_spec = None;
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) {
            "tests/fixtures/build_tool.ps1"
        } else {
            "tests/fixtures/build_tool.sh"
        });
        #[cfg(windows)]
        {
            artifact.tools.python = "powershell.exe".into();
            artifact.tools.arguments = vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
            ];
        }
        #[cfg(unix)]
        {
            artifact.tools.python = "sh".into();
            artifact.tools.arguments.clear();
        }
        artifact.tools.arguments.extend([
            fixture.into_os_string(),
            root.path().as_os_str().to_owned(),
            mode.into(),
        ]);
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        options
            .run_in(
                root.path(),
                &mut Streams::new(&mut stdout, &mut stderr, false),
            )
            .await
            .unwrap();
        let output = String::from_utf8(stdout).unwrap();
        assert_eq!(output.contains("local python has no runtime image"), warn);
        assert!(output.contains("demo-artifact.tar.gz"));
        assert_eq!(stderr, [] as [u8; 0]);
        let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
        assert!(
            commands.contains(&format!("--python-version|{expected_version}")),
            "{commands}"
        );
        assert!(!commands.contains("|torch|"));
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(root.path().join("demo-artifact.tar.gz")).unwrap(),
        ));
        let mut checked = false;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            assert!(!path.starts_with("target"));
            if path == Path::new("runpod_manifest.json") {
                let mut text = String::new();
                entry.read_to_string(&mut text).unwrap();
                let manifest: crate::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(manifest["pythonVersion"], expected_version);
                assert_eq!(manifest["excludedPackages"], json!(["numpy", "torch"]));
                checked = true;
            }
        }
        assert!(checked);
    }
}

#[test]
fn test_deploy_plan() {
    let output = plan(&[("demo", "main.py".into(), 2), ("other", String::new(), 1)]);
    assert!(output.contains("2 apps"));
    assert!(output.contains("main.py, 2 resources"));
    assert!(output.contains("1 resource"));
}

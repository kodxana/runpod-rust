use runpod::{
    Error,
    apps::build::{BuildTools, VendorOptions, sync_running_package},
};
use std::{fs, path::Path};

fn tools(root: &Path, mode: &str) -> BuildTools {
    #[cfg(windows)]
    let (program, args) = (
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/build_tool.ps1")
                .into_os_string(),
            root.as_os_str().to_owned(),
            mode.into(),
        ],
    );
    #[cfg(unix)]
    let (program, args) = (
        "sh",
        vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/build_tool.sh")
                .into_os_string(),
            root.as_os_str().to_owned(),
            mode.into(),
        ],
    );
    BuildTools {
        python: program.into(),
        arguments: args,
    }
}

#[test]
fn test_default_is_published_package_and_version_pin_passes_through() {
    let root = tempfile::tempdir().unwrap();
    let tool = tools(root.path(), "ok");
    assert_eq!(
        tool.runpod_requirement(root.path(), None).unwrap(),
        "runpod"
    );
    assert_eq!(
        tool.runpod_requirement(root.path(), Some("runpod==1.8.0"))
            .unwrap(),
        "runpod==1.8.0"
    );
    assert!(!root.path().join("commands.txt").exists());
}

#[test]
fn test_url_spec_builds_wheel_and_selects_last_sorted_name() {
    let root = tempfile::tempdir().unwrap();
    let tool = tools(root.path(), "ok");
    let result = tool
        .runpod_requirement(root.path(), Some("https://example.com/runpod.tar.gz"))
        .unwrap();
    assert_eq!(
        Path::new(&result).file_name().unwrap(),
        "runpod-2-py3-none-any.whl"
    );
    let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
    assert!(commands.contains("-m|pip|wheel|--no-deps|-q|-w|"));
    assert!(commands.contains("|https://example.com/runpod.tar.gz"));
}

#[test]
fn test_progress_reports_collected_packages_and_exact_target_flags() {
    let root = tempfile::tempdir().unwrap();
    let tool = tools(root.path(), "ok");
    let mut seen = Vec::new();
    tool.vendor(
        root.path(),
        &["numpy".into()],
        "3.12",
        &VendorOptions { no_deps: true },
        Some(&mut |count, name| {
            seen.push((count, name.to_owned()));
            Ok(())
        }),
    )
    .unwrap();
    assert_eq!(seen, [(1, "numpy".into()), (2, "requests".into())]);
    let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
    assert!(commands.contains("-m|pip|install|--upgrade|--no-color|--target|"));
    assert!(commands.contains("|--python-version|3.12|--implementation|cp|--only-binary|:all:|--platform|manylinux_2_28_x86_64|--platform|manylinux_2_17_x86_64|--platform|manylinux2014_x86_64|--no-deps|numpy"));
}

#[test]
fn test_large_stderr_does_not_block_progress_or_error() {
    let root = tempfile::tempdir().unwrap();
    let tool = tools(root.path(), "large-error");
    let mut seen = Vec::new();
    let error = tool
        .vendor(
            root.path(),
            &["numpy".into()],
            "3.12",
            &VendorOptions::default(),
            Some(&mut |count, name| {
                seen.push((count, name.to_owned()));
                Ok(())
            }),
        )
        .unwrap_err();
    assert!(error.to_string().contains("resolver failed"));
    assert!(error.to_string().len() < 4000);
    assert_eq!(seen, [(1, "numpy".into()), (2, "requests".into())]);
}

#[test]
fn pip_bootstrap_and_failures_are_reported() {
    let root = tempfile::tempdir().unwrap();
    tools(root.path(), "bootstrap")
        .vendor(
            root.path(),
            &["numpy".into()],
            "3.12",
            &VendorOptions::default(),
            None,
        )
        .unwrap();
    let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
    assert!(commands.contains("-m|ensurepip|--upgrade"));
    let error = tools(root.path(), "bootstrap-fail")
        .vendor(
            root.path(),
            &["numpy".into()],
            "3.12",
            &VendorOptions::default(),
            None,
        )
        .unwrap_err();
    assert!(error.to_string().contains("bootstrap unavailable"));
    for (mode, message) in [
        ("wheel-fail", "wheel build failed"),
        ("no-wheel", "produced no runpod wheel"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let error = tools(temp.path(), mode)
            .runpod_requirement(temp.path(), Some("./checkout"))
            .unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
}

#[test]
fn empty_requirements_skip_processes_and_progress_errors_propagate() {
    let root = tempfile::tempdir().unwrap();
    let tool = tools(root.path(), "ok");
    tool.vendor(root.path(), &[], "3.12", &VendorOptions::default(), None)
        .unwrap();
    assert!(!root.path().join("commands.txt").exists());
    let error = tool
        .vendor(
            root.path(),
            &["numpy".into()],
            "3.12",
            &VendorOptions::default(),
            Some(&mut |_, _| Err(Error::InvalidInput("observer failed".into()))),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "observer failed");
}

#[test]
fn test_running_package_overlays_env_and_excludes_bytecode() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let env = root.path().join("env");
    fs::create_dir_all(source.join("apps/__pycache__")).unwrap();
    fs::create_dir_all(env.join("runpod/apps")).unwrap();
    fs::write(source.join("apps/build.py"), "new").unwrap();
    fs::write(source.join("apps/build.pyc"), "cache").unwrap();
    fs::write(source.join("apps/__pycache__/build.pyc"), "cache").unwrap();
    fs::write(env.join("runpod/apps/build.py"), "old").unwrap();
    fs::write(env.join("runpod/stale.py"), "stale").unwrap();
    sync_running_package(&env, &source).unwrap();
    assert_eq!(
        fs::read_to_string(env.join("runpod/apps/build.py")).unwrap(),
        "new"
    );
    assert!(env.join("runpod/stale.py").exists());
    assert!(!env.join("runpod/apps/build.pyc").exists());
    assert!(!env.join("runpod/apps/__pycache__").exists());
}

fn wheel(root: &Path, name: &str, requires: &[&str]) {
    let info = root.join(format!("{name}-1.dist-info"));
    fs::create_dir_all(&info).unwrap();
    let metadata = format!(
        "Name: {name}\n{}",
        requires
            .iter()
            .map(|r| format!("Requires-Dist: {r}\n"))
            .collect::<String>()
    );
    fs::write(info.join("METADATA"), metadata).unwrap();
    fs::create_dir_all(root.join(name)).unwrap();
    fs::write(root.join(name).join("__init__.py"), "").unwrap();
    fs::write(
        info.join("RECORD"),
        format!(
            "{name}/__init__.py,,\n{name}-1.dist-info/METADATA,,\n{name}-1.dist-info/RECORD,,\n"
        ),
    )
    .unwrap();
}

fn build_case(
    gpu: bool,
    custom: bool,
    cpu_dependency: Option<&str>,
    marker: &str,
    excluded: &[&str],
) {
    build_case_dependency(gpu, custom, cpu_dependency, marker, excluded, "peft");
}

fn build_case_dependency(
    gpu: bool,
    custom: bool,
    cpu_dependency: Option<&str>,
    marker: &str,
    excluded: &[&str],
    gpu_dependency: &str,
) {
    use runpod::apps::{
        ResourceSpec, Selection,
        build::{BuildOptions, build_environment},
        deploy::{AppDefinition, ResourceDefinition},
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let seed = root.join("seed");
    wheel(&seed, "peft", &["torch"]);
    wheel(&seed, "torch", &["sympy"]);
    wheel(&seed, "sympy", &[]);
    wheel(&seed, "models", &[&format!("torch; {marker}")]);
    let source = root.join("sdk-source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("__init__.py"), "sdk").unwrap();
    let mut app = AppDefinition::new("app", "default").unwrap();
    app.resources.push(ResourceDefinition {
        spec: ResourceSpec {
            name: "main".into(),
            gpu: gpu.then(|| Selection::One("AMPERE_16".into())),
            cpu: (!gpu).then(|| Selection::One("cpu3c-1-2".into())),
            image: custom.then(|| "custom:1".into()),
            dependencies: Some(vec![gpu_dependency.into()]),
            ..ResourceSpec::default()
        },
        symbol: None,
        referenced_secrets: Vec::new(),
    });
    if let Some(dependency) = cpu_dependency {
        app.resources.push(ResourceDefinition {
            spec: ResourceSpec {
                name: "prep".into(),
                gpu: None,
                cpu: Some(Selection::One("cpu3c-1-2".into())),
                dependencies: Some(vec![dependency.into()]),
                ..ResourceSpec::default()
            },
            symbol: None,
            referenced_secrets: Vec::new(),
        });
    }
    let options = BuildOptions {
        sdk_source: Some(source),
        package_spec: None,
        ..BuildOptions::default()
    };
    let result = build_environment(&app, root, &options, &tools(root, "ok"), None).unwrap();
    assert_eq!(result.excluded, excluded);
    assert!(result.env_dir().join("runpod/__init__.py").is_file());
    assert_eq!(result.env_dir().join("torch").is_dir(), !gpu || custom);
    assert_eq!(result.env_dir().join("sympy").is_dir(), !gpu || custom);
    assert!(result.env_dir().join("peft").is_dir());
    assert!(result.requirements.iter().any(|s| s == "aiohttp[speedups]"));
    let path = result.env_dir().to_owned();
    drop(result);
    assert!(!path.exists());
}

#[test]
fn test_gpu_builtin_image_prunes_transitive_torch() {
    build_case(true, false, None, "extra == 'torch'", &["torch"]);
}
#[test]
fn test_mixed_cpu_gpu_app_prunes_transitive_torch() {
    build_case(true, false, Some("pandas"), "extra == 'torch'", &[]);
}
#[test]
fn test_mixed_app_records_torch_only_when_cpu_needs_it() {
    build_case(true, false, Some("peft"), "extra == 'torch'", &["torch"]);
}
#[test]
fn test_mixed_app_ignores_torch_behind_an_unrequested_extra() {
    build_case(true, false, Some("models"), "extra == 'torch'", &[]);
}
#[test]
fn test_mixed_app_records_torch_for_a_requested_extra() {
    build_case(
        true,
        false,
        Some("models[torch]"),
        "extra == 'torch' or extra == 'all'",
        &["torch"],
    );
}
#[test]
fn test_mixed_app_records_torch_when_a_marker_branch_skips_the_extra() {
    build_case(
        true,
        false,
        Some("models"),
        "extra == 'torch' or python_version >= '3.12'",
        &["torch"],
    );
}
#[test]
fn test_custom_image_keeps_transitive_torch() {
    build_case(true, true, None, "extra == 'torch'", &[]);
}
#[test]
fn test_cpu_keeps_transitive_torch() {
    build_case(false, false, None, "extra == 'torch'", &[]);
}

#[test]
fn unsupported_versions_fail_before_any_process_or_output_directory() {
    use runpod::apps::{
        build::{BuildOptions, build_environment},
        deploy::AppDefinition,
    };
    let root = tempfile::tempdir().unwrap();
    let scratch = root.path().join("scratch");
    let options = BuildOptions {
        python_version: "3.9".into(),
        scratch_dir: Some(scratch.clone()),
        ..BuildOptions::default()
    };
    let error = build_environment(
        &AppDefinition::new("app", "default").unwrap(),
        root.path(),
        &options,
        &tools(root.path(), "ok"),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("python 3.9 is not supported"));
    assert!(!scratch.exists());
    assert!(!root.path().join("commands.txt").exists());
}

#[test]
fn sdk_override_and_pinned_fallback_install_no_deps_before_runtime_and_api_dependencies() {
    use runpod::apps::{
        ResourceKind, ResourceSpec,
        build::{BuildOptions, PINNED_SDK_SOURCE, build_environment},
        deploy::{AppDefinition, ResourceDefinition},
    };
    for override_spec in [Some("runpod==9"), None] {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("scratch");
        let mut app = AppDefinition::new("app", "default").unwrap();
        app.resources.push(ResourceDefinition {
            spec: ResourceSpec {
                kind: ResourceKind::Api,
                ..ResourceSpec::default()
            },
            symbol: None,
            referenced_secrets: Vec::new(),
        });
        let options = BuildOptions {
            scratch_dir: Some(scratch.clone()),
            sdk_source: None,
            package_spec: override_spec.map(str::to_owned),
            ..BuildOptions::default()
        };
        let result =
            build_environment(&app, root.path(), &options, &tools(root.path(), "ok"), None)
                .unwrap();
        assert!(result.requirements.contains(&"fastapi[standard]".into()));
        assert!(result.requirements.contains(&"uvicorn>=0.30".into()));
        let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
        let installs: Vec<_> = commands
            .lines()
            .filter(|line| line.contains("|install|"))
            .collect();
        assert_eq!(installs.len(), 2);
        assert!(installs[0].contains("|--no-deps|"));
        assert!(!installs[1].contains("|--no-deps|"));
        if let Some(spec) = override_spec {
            assert!(installs[0].ends_with(spec));
            assert!(!commands.contains("|wheel|"));
        } else {
            assert!(commands.contains(PINNED_SDK_SOURCE));
            assert!(installs[0].ends_with("runpod-2-py3-none-any.whl"));
        }
        drop(result);
        assert!(scratch.join("env").exists());
    }
}

#[test]
fn failed_builds_clean_automatic_storage_but_keep_explicit_scratch() {
    use runpod::apps::{
        build::{BuildOptions, build_environment},
        deploy::AppDefinition,
    };
    for explicit in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        let options = BuildOptions {
            scratch_dir: explicit.then(|| root.path().join("scratch")),
            sdk_source: Some(source),
            package_spec: None,
            ..BuildOptions::default()
        };
        let error = build_environment(
            &AppDefinition::new("app", "default").unwrap(),
            root.path(),
            &options,
            &tools(root.path(), "vendor-fail"),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("resolver failed"));
        let commands = fs::read_to_string(root.path().join("commands.txt")).unwrap();
        let target = commands
            .split("|--target|")
            .nth(1)
            .unwrap()
            .split('|')
            .next()
            .unwrap();
        assert_eq!(Path::new(target).exists(), explicit);
    }
}

#[test]
fn test_mixed_app_direct_gpu_torch_not_recorded() {
    build_case_dependency(
        true,
        false,
        Some("pandas"),
        "extra == 'torch'",
        &[],
        "torch",
    );
}

#[test]
fn test_mixed_app_evaluates_extra_markers() {
    for marker in [
        "(sys_platform == 'linux' or sys_platform == 'win32') and extra == 'torch'",
        "extra == 'torch' and (sys_platform == 'linux' or sys_platform == 'win32')",
        "'torch' == extra",
        "extra == 'torch' and python_version < '3.12'",
    ] {
        build_case(true, false, Some("models"), marker, &[]);
        build_case(true, false, Some("models[torch]"), marker, &["torch"]);
    }
}

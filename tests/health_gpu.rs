use runpod::serverless::health::{GpuChecks, GpuConfig, HealthError, parse_gpu_test_output};
use std::{collections::BTreeMap, time::Duration};

mod support;
use support::command;
fn checks(binary: &str, smi: &str) -> GpuChecks {
    GpuChecks::with_commands(GpuConfig::default(), Some(command(binary)), command(smi))
}

#[test]
fn python_gpu_output_fixtures() {
    let cases: Vec<runpod::Value> =
        serde_json::from_str(include_str!("fixtures/health.json")).unwrap();
    for case in cases.into_iter().filter(|c| c["operation"] == "gpu_parse") {
        assert_eq!(
            serde_json::to_value(parse_gpu_test_output(case["input"].as_str().unwrap())).unwrap(),
            case["output"],
            "{case}"
        );
    }
}

#[tokio::test]
async fn binary_success_uses_output_even_when_exit_status_is_nonzero() {
    let result = checks("success", "fallback_error")
        .run_gpu_test_binary()
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.gpu_count, 2);
    checks("success", "fallback_error")
        .check_gpu_health()
        .await
        .unwrap();
}

#[tokio::test]
async fn allocation_failure_never_falls_back_to_a_list_of_healthy_gpus() {
    let error = checks("allocation", "success")
        .check_gpu_health()
        .await
        .unwrap_err();
    assert!(matches!(error, HealthError::GpuAllocation(_)));
    assert!(error.to_string().contains("out of memory"));
}

#[tokio::test]
async fn loader_failure_falls_back_and_combined_failure_keeps_both_causes() {
    checks("loader", "list").check_gpu_health().await.unwrap();
    let error = checks("loader", "fallback_error")
        .check_gpu_health()
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("Binary test:"), "{text}");
    assert!(text.contains("NVML"), "{text}");
    assert!(text.contains("Fallback test:"), "{text}");
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn missing_binary_fallback_and_empty_gpu_list() {
    let worker = GpuChecks::with_commands(GpuConfig::default(), None, command("list"));
    worker.check_gpu_health().await.unwrap();
    let error = GpuChecks::with_commands(GpuConfig::default(), None, command("empty"))
        .check_gpu_health()
        .await
        .unwrap_err();
    assert!(error.to_string().contains("binary disabled/missing"));
    assert!(error.to_string().contains("No GPUs detected"));
}

#[tokio::test]
async fn binary_timeout_kills_and_reaps_the_process() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("finished");
    let mut binary = command("sleep");
    binary.environment.insert(
        "HEALTH_PROBE_FINISHED".into(),
        marker.clone().into_os_string(),
    );
    let worker = GpuChecks::with_commands(
        GpuConfig {
            test_timeout: Duration::from_millis(300),
            ..GpuConfig::default()
        },
        Some(binary),
        command("list"),
    );
    let error = worker.run_gpu_test_binary().await.unwrap_err();
    assert!(matches!(error, HealthError::Timeout { .. }));
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(
        !marker.exists(),
        "timed-out child survived and wrote its output"
    );
}

#[tokio::test]
async fn max_error_messages_preserves_python_negative_slice() {
    for (limit, expected) in [
        (1, "error one"),
        (0, "GPU memory allocation test failed: "),
        (-1, "error one; error two"),
    ] {
        let worker = GpuChecks::with_commands(
            GpuConfig {
                max_error_messages: limit,
                ..GpuConfig::default()
            },
            Some(command("errors")),
            command("list"),
        );
        let error = worker.run_gpu_test_binary().await.unwrap_err().to_string();
        assert!(error.ends_with(expected), "limit={limit}: {error}");
    }
}

#[tokio::test]
async fn gpu_detection_requires_success_and_nvidia_smi_marker() {
    assert!(checks("success", "detected").is_available().await);
    assert!(!checks("success", "list").is_available().await);
    assert!(!checks("success", "detection_error").is_available().await);
}

#[test]
fn gpu_configuration_is_owned_and_parsed_lazily_by_the_caller() {
    let env = BTreeMap::from([
        ("RUNPOD_GPU_TEST_TIMEOUT".into(), "7".into()),
        ("RUNPOD_GPU_MAX_ERROR_MESSAGES".into(), "-1".into()),
    ]);
    let config = GpuConfig::from_values(&env).unwrap();
    assert_eq!(config.test_timeout, Duration::from_secs(7));
    assert_eq!(config.max_error_messages, -1);
    assert!(
        GpuConfig::from_values(&BTreeMap::from([(
            "RUNPOD_GPU_TEST_TIMEOUT".into(),
            "invalid".into()
        )]))
        .is_err()
    );
}

#[tokio::test]
async fn missing_binary_and_fallback_timeouts_retain_error_categories() {
    let absent = GpuChecks::with_commands(GpuConfig::default(), None, command("list"));
    assert!(
        matches!(absent.run_gpu_test_binary().await.unwrap_err(),HealthError::Io {source,..} if source.kind()==std::io::ErrorKind::NotFound)
    );
    let worker = GpuChecks::with_commands(
        GpuConfig {
            fallback_timeout: Duration::from_millis(100),
            ..GpuConfig::default()
        },
        None,
        command("sleep"),
    );
    let error = worker.run_gpu_test_fallback().await.unwrap_err();
    assert_eq!(error.to_string(), "nvidia-smi timed out");
    assert!(std::error::Error::source(&error).is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn non_executable_allocation_binary_uses_the_fallback() {
    use std::os::unix::fs::PermissionsExt;
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .unwrap();
    let worker = GpuChecks::with_commands(
        GpuConfig::default(),
        Some(runpod::serverless::health::ProbeCommand::new(file.path())),
        command("list"),
    );
    assert!(
        matches!(worker.run_gpu_test_binary().await.unwrap_err(),HealthError::Io {source,..} if source.kind()==std::io::ErrorKind::PermissionDenied)
    );
    worker.check_gpu_health().await.unwrap();
}

#[tokio::test]
async fn explicit_binary_override_takes_precedence_over_bundled_probe() {
    let executable = support::probe_executable();
    let checks = GpuChecks::from_values(&BTreeMap::from([(
        "RUNPOD_BINARY_GPU_TEST_PATH".into(),
        executable.to_string_lossy().into_owned(),
    )]))
    .unwrap();
    assert_eq!(checks.run_gpu_test_binary().await.unwrap().gpu_count, 2);
}

#[tokio::test]
async fn canceling_a_gpu_probe_kills_the_owned_child() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("finished");
    let started = dir.path().join("started");
    let mut probe = command("sleep");
    probe.environment.insert(
        "HEALTH_PROBE_FINISHED".into(),
        marker.clone().into_os_string(),
    );
    probe.environment.insert(
        "HEALTH_PROBE_STARTED".into(),
        started.clone().into_os_string(),
    );
    let worker = GpuChecks::with_commands(GpuConfig::default(), Some(probe), command("list"));
    let task = tokio::spawn(async move { worker.run_gpu_test_binary().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!marker.exists());
}

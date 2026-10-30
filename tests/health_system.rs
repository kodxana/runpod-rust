use runpod::serverless::health::{
    DiskInfo, MemoryInfo, SystemChecks, SystemConfig, get_binary_path, is_early_check_eligible,
    is_serverless_environment, network_probe_target, parse_version,
};
use std::{collections::BTreeMap, time::Duration};
mod support;

#[test]
fn python_health_helper_fixtures() {
    let cases: Vec<runpod::Value> =
        serde_json::from_str(include_str!("fixtures/health.json")).unwrap();
    for c in cases {
        match c["operation"].as_str().unwrap() {
            "version" => assert_eq!(
                runpod::json!(parse_version(c["input"].as_str().unwrap())),
                c["output"]
            ),
            "network_target" => assert_eq!(
                runpod::json!(network_probe_target(c["input"].as_str())),
                c["output"],
                "{c}"
            ),
            "eligibility" => {
                let values = serde_json::from_value(c["environment"].clone()).unwrap();
                let args: Vec<String> = serde_json::from_value(c["args"].clone()).unwrap();
                assert_eq!(
                    runpod::json!({"serverless":is_serverless_environment(&values),"early":is_early_check_eligible(&values,&args)}),
                    c["output"],
                    "{c}"
                );
            }
            "gpu_parse" => {}
            _ => panic!("unknown fixture"),
        }
    }
}

#[test]
fn memory_and_disk_thresholds_and_zero_total() {
    let config = SystemConfig::default();
    let checks = SystemChecks::new(config);
    for available in [4.0, 12.0] {
        checks
            .check_memory_info(MemoryInfo {
                total_gb: 16.0,
                available_gb: available,
                used_percent: 0.0,
            })
            .unwrap();
    }
    assert!(
        checks
            .check_memory_info(MemoryInfo {
                total_gb: 4.0,
                available_gb: 2.0,
                used_percent: 50.0
            })
            .unwrap_err()
            .to_string()
            .contains("Insufficient memory")
    );
    checks
        .check_disk_info(DiskInfo {
            total_bytes: 1000,
            free_bytes: 100,
        })
        .unwrap();
    assert!(
        checks
            .check_disk_info(DiskInfo {
                total_bytes: 1000,
                free_bytes: 99
            })
            .is_err()
    );
    assert!(
        checks
            .check_disk_info(DiskInfo {
                total_bytes: 0,
                free_bytes: 0
            })
            .is_err()
    );
    let threshold = SystemChecks::new(SystemConfig {
        min_memory_gb: 8.0,
        ..SystemConfig::default()
    });
    assert!(
        threshold
            .check_memory_info(MemoryInfo {
                total_gb: 8.0,
                available_gb: 7.9,
                used_percent: 1.25
            })
            .is_err()
    );
}

#[test]
fn native_memory_and_disk_queries_read_the_host() {
    let memory = SystemChecks::memory_info();
    assert!(memory.total_gb > 0.0 && memory.available_gb > 0.0);
    let disk = SystemChecks::disk_info(std::env::current_dir().unwrap()).unwrap();
    assert!(disk.total_bytes > 0 && disk.free_bytes <= disk.total_bytes);
    SystemChecks::new(SystemConfig {
        min_disk_percent: 0.0,
        ..SystemConfig::default()
    })
    .check_disk_space()
    .unwrap();
}

#[test]
fn memory_proc_fallback_preserves_missing_keys_and_invalid_lines() {
    let info =
        MemoryInfo::from_meminfo("MemTotal: 8388608 kB\nMemAvailable: 4194304 kB\nOther: 123 kB\n")
            .unwrap();
    assert_eq!(
        info,
        MemoryInfo {
            total_gb: 8.0,
            available_gb: 4.0,
            used_percent: 50.0
        }
    );
    assert_eq!(MemoryInfo::from_meminfo("").unwrap().used_percent, 0.0);
    assert!(MemoryInfo::from_meminfo("bad line").is_err());
}

#[test]
fn binary_paths_prefer_valid_override_then_package_file() {
    let dir = tempfile::tempdir().unwrap();
    let packaged = dir.path().join("gpu_test");
    let override_path = dir.path().join("custom");
    std::fs::write(&packaged, []).unwrap();
    std::fs::write(&override_path, []).unwrap();
    let mut values = BTreeMap::from([(
        "RUNPOD_BINARY_GPU_TEST_PATH".into(),
        override_path.to_string_lossy().into_owned(),
    )]);
    assert_eq!(
        get_binary_path("gpu_test", &values, dir.path()),
        Some(override_path)
    );
    values.insert(
        "RUNPOD_BINARY_GPU_TEST_PATH".into(),
        dir.path().to_string_lossy().into_owned(),
    );
    assert_eq!(
        get_binary_path("gpu_test", &values, dir.path()),
        Some(packaged)
    );
    assert_eq!(get_binary_path("missing", &values, dir.path()), None);
}

#[tokio::test]
async fn network_readiness_uses_host_and_port_without_sending_http() {
    use tokio::io::AsyncReadExt;
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let accept = tokio::spawn(async move {
        let (mut stream, _) = server.accept().await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
    });
    let checks = SystemChecks::new(SystemConfig {
        job_url: Some(format!("http://user:secret@{address}/job?token=hidden")),
        ..SystemConfig::default()
    });
    checks.check_network_connectivity().await.unwrap();
    accept.await.unwrap();
}

#[tokio::test]
async fn network_refusal_is_bounded_and_never_exposes_url_credentials() {
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    drop(server);
    let checks = SystemChecks::new(SystemConfig {
        network_timeout: Duration::from_millis(200),
        job_url: Some(format!("http://user:secret@{address}/job?token=hidden")),
        ..SystemConfig::default()
    });
    let error = tokio::time::timeout(Duration::from_secs(1), checks.check_network_connectivity())
        .await
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Connection refused") || error.contains("Timeout"),
        "{error}"
    );
    assert!(!error.contains("secret") && !error.contains("hidden"));
}

#[test]
fn configuration_validates_timeout_but_preserves_negative_thresholds() {
    let mut values = BTreeMap::from([
        ("RUNPOD_MIN_MEMORY_GB".into(), "-1".into()),
        ("RUNPOD_NETWORK_CHECK_TIMEOUT".into(), "0".into()),
    ]);
    assert!(SystemConfig::from_values(&values).is_err());
    values.remove("RUNPOD_NETWORK_CHECK_TIMEOUT");
    assert_eq!(
        SystemConfig::from_values(&values).unwrap().min_memory_gb,
        -1.0
    );
}

#[tokio::test]
async fn cuda_version_prefers_nvcc_and_falls_back_only_when_needed() {
    let checks = SystemChecks::with_commands(
        SystemConfig::default(),
        support::command("nvcc"),
        support::command("fallback_error"),
    );
    assert_eq!(
        checks.cuda_version().await.as_deref(),
        Some("Release 12.2, V12.2.140")
    );
    checks.check_cuda_versions().await.unwrap();
    let checks = SystemChecks::with_commands(
        SystemConfig::default(),
        support::command("fallback_error"),
        support::command("smi_version"),
    );
    assert_eq!(
        checks.cuda_version().await.as_deref(),
        Some("CUDA Version: 12.7")
    );
    let checks = SystemChecks::with_commands(
        SystemConfig::default(),
        support::command("empty"),
        support::command("empty"),
    );
    assert_eq!(checks.cuda_version().await, None);
    checks.check_cuda_versions().await.unwrap();
    let checks = SystemChecks::with_commands(
        SystemConfig::default(),
        support::command("nvcc_old"),
        support::command("smi_version"),
    );
    assert!(
        checks
            .check_cuda_versions()
            .await
            .unwrap_err()
            .to_string()
            .contains("11.7 found, 11.8 required")
    );
    let dir = tempfile::tempdir().unwrap();
    let missing = runpod::serverless::health::ProbeCommand::new(dir.path().join("missing"));
    let checks = SystemChecks::with_commands(SystemConfig::default(), missing.clone(), missing);
    assert_eq!(checks.cuda_version().await, None);
    checks.check_cuda_versions().await.unwrap();
}

#[tokio::test]
async fn timed_out_cuda_probe_is_killed_before_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("finished");
    let mut command = support::command("sleep");
    command.environment.insert(
        "HEALTH_PROBE_FINISHED".into(),
        marker.clone().into_os_string(),
    );
    let checks = SystemChecks::with_commands(
        SystemConfig {
            cuda_probe_timeout: Duration::from_millis(300),
            ..SystemConfig::default()
        },
        command,
        support::command("smi_version"),
    );
    assert_eq!(
        checks.cuda_version().await.as_deref(),
        Some("CUDA Version: 12.7")
    );
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(!marker.exists());
}

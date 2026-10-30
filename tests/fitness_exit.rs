use runpod::{
    Error,
    serverless::health::{FitnessCheck, FitnessChecks, terminate_unhealthy},
};
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, sync::OnceLock, time::Duration};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, query_param},
};

async fn child(mode: &str, ping: Option<&str>, marker: &std::path::Path) -> std::process::Output {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "fitness_exit_child", "--ignored", "--nocapture"])
        .env("RUNPOD_RUST_FITNESS_CHILD", mode)
        .env("RUNPOD_RUST_EXIT_MARKER", marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(ping) = ping {
        command.env("RUNPOD_RUST_TEST_PING", ping);
    }
    tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .expect(
            "unhealthy worker must terminate without waiting for background threads or callbacks",
        )
        .unwrap()
}

#[tokio::test]
async fn hard_exit_skips_destructors_and_c_callbacks_with_live_threads() {
    let temp = tempfile::tempdir().unwrap();
    for (mode, code) in [("direct", 73), ("failure", 1), ("setup", 1)] {
        let marker = temp.path().join(mode);
        let output = child(mode, None, &marker).await;
        assert_eq!(output.status.code(), Some(code), "{mode}: {output:?}");
        assert!(
            !marker.exists(),
            "{mode}: process shutdown ran an exit callback or destructor"
        );
    }
}

#[tokio::test]
async fn failed_check_and_failed_setup_report_before_terminating() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    for (mode, check) in [("failure", "broken"), ("setup", "fitness_check_setup")] {
        Mock::given(method("GET"))
            .and(query_param("status", "unhealthy"))
            .and(query_param("check", check))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let output = child(mode, Some(&server.uri()), &temp.path().join(mode)).await;
        assert_eq!(output.status.code(), Some(1), "{output:?}");
    }
}

#[tokio::test]
async fn cancelling_an_unhealthy_report_still_terminates_the_process() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let output = child("cancel", Some(&server.uri()), &temp.path().join("cancel")).await;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

static EXIT_MARKER: OnceLock<PathBuf> = OnceLock::new();
extern "C" fn exit_callback() {
    if let Some(path) = EXIT_MARKER.get() {
        let _ = std::fs::write(path, "atexit");
    }
}
struct DropMarker(PathBuf);
impl Drop for DropMarker {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "drop");
    }
}

#[test]
#[ignore = "isolated child for testing unconditional process termination"]
#[expect(
    unsafe_code,
    reason = "register a C exit callback to prove _exit bypasses it"
)]
fn fitness_exit_child() {
    let Ok(mode) = std::env::var("RUNPOD_RUST_FITNESS_CHILD") else {
        return;
    };
    let marker = PathBuf::from(std::env::var_os("RUNPOD_RUST_EXIT_MARKER").unwrap());
    EXIT_MARKER.set(marker.clone()).unwrap();
    // SAFETY: the callback has C ABI, takes no arguments, does not unwind, and lives for the process.
    assert_eq!(unsafe { libc::atexit(exit_callback) }, 0);
    let _guard = DropMarker(marker);
    std::thread::spawn(|| {
        loop {
            std::thread::park();
        }
    });
    if mode == "direct" {
        terminate_unhealthy(73);
    }
    let mut values = BTreeMap::from([
        ("RUNPOD_SKIP_GPU_CHECK".into(), "true".into()),
        ("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS".into(), "true".into()),
    ]);
    if let Ok(ping) = std::env::var("RUNPOD_RUST_TEST_PING") {
        values.insert("RUNPOD_WEBHOOK_PING".into(), ping);
        values.insert("RUNPOD_AI_API_KEY".into(), "worker-key".into());
    }
    if mode == "setup" {
        values.remove("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS");
        values.insert("RUNPOD_MIN_MEMORY_GB".into(), "invalid".into());
    }
    let mut checks = FitnessChecks::new(values);
    checks.register_fitness_check(FitnessCheck::sync("broken", || {
        Err(Error::InvalidInput("bad threshold".into()))
    }));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            if mode == "cancel" {
                let _ =
                    tokio::time::timeout(Duration::from_millis(100), checks.run_fitness_checks())
                        .await;
            } else {
                checks.run_fitness_checks().await;
            }
            panic!("unhealthy exit returned");
        });
}

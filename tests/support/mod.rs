use runpod::serverless::health::ProbeCommand;
use std::collections::BTreeMap;
pub fn command(mode: &str) -> ProbeCommand {
    ProbeCommand {
        program: probe_executable(),
        args: Vec::new(),
        environment: BTreeMap::from([("HEALTH_PROBE_MODE".into(), mode.into())]),
    }
}
pub fn probe_executable() -> std::path::PathBuf {
    static PROGRAM: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PROGRAM
        .get_or_init(|| {
            let test_exe = std::env::current_exe().unwrap();
            let path = test_exe.with_file_name(format!(
                "{}-health-probe{}",
                test_exe.file_stem().unwrap().to_string_lossy(),
                std::env::consts::EXE_SUFFIX
            ));
            let rustc = std::path::Path::new(env!("CARGO"))
                .with_file_name(format!("rustc{}", std::env::consts::EXE_SUFFIX));
            let result = std::process::Command::new(rustc)
                .arg("--edition=2024")
                .arg(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/support/health_probe.rs"),
                )
                .arg("-o")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            path
        })
        .clone()
}

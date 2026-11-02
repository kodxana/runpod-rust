pub(super) fn host_target() -> String {
    let rustc = std::env::var_os("RUSTC").map_or_else(
        || {
            let home = std::env::var_os("CARGO_HOME").map_or_else(
                || {
                    std::path::PathBuf::from(
                        std::env::var_os("HOME")
                            .or_else(|| std::env::var_os("USERPROFILE"))
                            .unwrap(),
                    )
                    .join(".cargo")
                },
                std::path::PathBuf::from,
            );
            home.join("bin")
                .join(if cfg!(windows) { "rustc.exe" } else { "rustc" })
        },
        std::path::PathBuf::from,
    );
    let output = std::process::Command::new(rustc)
        .arg("-vV")
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap()
        .into()
}

pub(super) fn host_target() -> String {
    let rustc = std::env::var_os("RUSTC").map_or_else(
        || {
            // Cargo supplies its selected toolchain's executable at compile time.
            // CI toolchains need not install proxies under HOME/.cargo/bin.
            std::path::Path::new(env!("CARGO")).with_file_name(if cfg!(windows) {
                "rustc.exe"
            } else {
                "rustc"
            })
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

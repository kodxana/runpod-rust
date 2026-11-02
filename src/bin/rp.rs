#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    runpod::cli::run(std::env::args_os()).await
}

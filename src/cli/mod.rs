//! Native `rp` and `runpod` command entry points.
//!
//! Currently implements the catalog command. Other Python CLI commands are still pending.
mod catalog;
mod output;

use std::{ffi::OsString, io::Write, process::ExitCode};

/// Runs a CLI invocation, including its executable name, and writes to standard streams.
///
/// Returns success for help or a completed command, 2 for invalid arguments, and 1 for
/// configuration, API, or output errors. No network requests are made for invalid options.
pub async fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let command = clap::Command::new("rp")
        .about("Runpod command-line client")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(catalog::command());
    let matches = match command.try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    let Some(("catalog", args)) = matches.subcommand() else {
        return ExitCode::from(2);
    };
    let options = match catalog::Options::read(args) {
        Ok(options) => options,
        Err(message) => {
            let _ = writeln!(std::io::stderr().lock(), "Error: {message}");
            return ExitCode::from(2);
        }
    };
    match options.execute().await {
        Ok(output) => match std::io::stdout().lock().write_all(output.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(std::io::stderr().lock(), "Error: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "Error: {error}");
            ExitCode::FAILURE
        }
    }
}

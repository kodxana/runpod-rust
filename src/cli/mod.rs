//! Native `rp` and `runpod` command entry points.
//!
//! Implements catalog, volume and endpoint resource commands. Remaining commands are pending.
mod api;
mod catalog;
mod editor;
mod endpoint;
mod interaction;
mod output;
mod streams;
mod volume;

use std::{
    ffi::OsString,
    io::{IsTerminal, Write},
    process::ExitCode,
};

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Api(#[from] crate::Error),
    #[error(transparent)]
    Prompt(#[from] dialoguer::Error),
    #[error("Aborted!")]
    Aborted,
}
impl CliError {
    fn code(&self) -> u8 {
        if matches!(self, Self::Usage(_)) { 2 } else { 1 }
    }
}
type CliResult<T> = std::result::Result<T, CliError>;

/// Runs a CLI invocation, including its executable name, and writes to standard streams.
///
/// Returns success for help or a completed command, 2 for invalid arguments, and 1 for
/// configuration, API, or output errors. Argument syntax and mode conflicts are checked
/// before network requests; checks involving existing resources may require a read.
/// Interactive commands can block while waiting for terminal input.
pub async fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let command = clap::Command::new("rp")
        .about("Runpod command-line client")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(catalog::command())
        .subcommand(volume::command())
        .subcommand(endpoint::command());
    let matches = match command.try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let terminal = stdout.is_terminal();
    let mut streams = streams::Streams::new(&mut stdout, &mut stderr, terminal);
    match dispatch(&matches, &mut streams).await {
        Ok(output) => match std::io::stdout().lock().write_all(output.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(error) => {
                let _ = writeln!(std::io::stderr().lock(), "Error: {error}");
                ExitCode::FAILURE
            }
        },
        Err(CliError::Api(crate::Error::Io(error)))
            if error.kind() == std::io::ErrorKind::BrokenPipe =>
        {
            ExitCode::SUCCESS
        }
        Err(error) => {
            if matches!(error, CliError::Aborted) {
                let _ = writeln!(std::io::stderr().lock(), "{error}");
            } else {
                let _ = writeln!(std::io::stderr().lock(), "Error: {error}");
            }
            ExitCode::from(error.code())
        }
    }
}

async fn dispatch(
    matches: &clap::ArgMatches,
    streams: &mut streams::Streams<'_>,
) -> CliResult<String> {
    match matches.subcommand() {
        Some(("catalog", args)) => Ok(catalog::Options::read(args)
            .map_err(CliError::Usage)?
            .execute()
            .await?),
        Some(("volume", args)) => {
            let options = volume::Options::read(args)?;
            options
                .execute(&crate::Client::from_env()?, &mut interaction::Terminal)
                .await
        }
        Some(("endpoint", args)) => {
            let options = endpoint::Options::read(args)?;
            options.validate_interaction(&interaction::Terminal)?;
            options
                .execute(
                    &crate::Client::from_env()?,
                    &mut interaction::Terminal,
                    streams,
                )
                .await?;
            Ok(String::new())
        }
        _ => Err(CliError::Usage("a command is required".into())),
    }
}

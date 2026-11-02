//! Native `rp` and `runpod` command entry points.
//!
//! Implements login, catalog, volume, pod, endpoint, registry, secret and flash management commands.
//! Remaining commands are pending.
mod api;
mod catalog;
#[cfg(test)]
mod credential_tests;
mod credentials;
mod editor;
mod endpoint;
mod environment;
mod flash;
mod interaction;
mod login;
mod logs;
mod output;
mod pod;
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
    #[error("{0}")]
    Failed(String),
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
        .subcommand(pod::command())
        .subcommand(credentials::Kind::Registry.command())
        .subcommand(credentials::Kind::Secret.command())
        .subcommand(login::command())
        .subcommand(flash::command())
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
        Some(("flash", args)) => {
            flash::execute(args, streams).await?;
            Ok(String::new())
        }
        Some(("login", args)) => {
            login::Options::read(args)?.run(streams).await?;
            Ok(String::new())
        }
        Some((name @ ("registry" | "secret"), args)) => {
            let kind = if name == "registry" {
                credentials::Kind::Registry
            } else {
                credentials::Kind::Secret
            };
            let operation = credentials::Operation::read(kind, args, &mut interaction::Terminal)?;
            let api = crate::apps::AppsApiClient::new(crate::Client::from_env()?)?;
            operation.execute(&api).await
        }
        Some(("pod", args)) => {
            pod::execute(args, streams).await?;
            Ok(String::new())
        }
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
            if let Some(args) = args.subcommand_matches("logs") {
                logs::Options::read(args, "endpoint-id")
                    .execute(streams)
                    .await?;
                return Ok(String::new());
            }
            if let Some(args) = args.subcommand_matches("run") {
                endpoint::RunOptions::read(args)?.execute(streams).await?;
                return Ok(String::new());
            }
            if matches!(args.subcommand_name(), Some("job" | "purge")) {
                endpoint::execute_job(args, &mut interaction::Terminal, streams).await?;
                return Ok(String::new());
            }
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

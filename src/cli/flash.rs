use super::{
    CliError, CliResult,
    interaction::Interaction,
    streams::{Destination, Streams},
};
use crate::{
    Value,
    apps::{
        AppsApiClient,
        events::EventSink,
        manage::{self, UndeployResult},
    },
    client::{required_str, truthy},
    control::python_string,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::{fmt::Write, future::Future};

#[cfg(test)]
#[path = "flash_tests.rs"]
mod tests;

pub(super) fn command() -> Command {
    Command::new("flash")
        .about("Manage Runpod applications")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(super::deployment::command())
        .subcommand(super::environment::command())
        .subcommand(super::environment::undeploy_command())
        .subcommand(
            Command::new("app")
                .about("Manage deployed apps")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(Command::new("list").about("List apps and their environments"))
                .subcommand(
                    Command::new("get")
                        .about("Show an app and its environments")
                        .arg(Arg::new("name").required(true)),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Undeploy all environments and delete the app")
                        .arg(Arg::new("name").required(true))
                        .arg(
                            Arg::new("yes")
                                .long("yes")
                                .short('y')
                                .action(ArgAction::SetTrue),
                        ),
                ),
        )
}

enum Operation {
    List,
    Get(String),
    Delete(String),
}
impl Operation {
    fn read(args: &ArgMatches, ui: &mut impl Interaction) -> CliResult<Self> {
        let args = args
            .subcommand_matches("app")
            .ok_or_else(|| CliError::Usage("an app command is required".into()))?;
        match args.subcommand() {
            Some(("list", _)) => Ok(Self::List),
            Some((operation @ ("get" | "delete"), args)) => {
                let name = args
                    .get_one::<String>("name")
                    .ok_or_else(|| CliError::Usage("pass an APP_NAME".into()))?;
                if operation == "get" {
                    return Ok(Self::Get(name.clone()));
                }
                ui.confirm(
                    &format!("delete app '{name}' and all its environments?"),
                    args.get_flag("yes"),
                )?;
                Ok(Self::Delete(name.clone()))
            }
            _ => Err(CliError::Usage("an app command is required".into())),
        }
    }

    async fn execute(&self, api: &mut impl Backend, streams: &mut Streams<'_>) -> CliResult<()> {
        let output = match self {
            Self::List => show_list(&api.list().await?)?,
            Self::Get(name) => show_app(&api.get(name).await?)?,
            Self::Delete(name) => {
                let result = delete(name, api, streams).await?;
                if !result.failures.is_empty() {
                    for failure in result.failures {
                        streams.write(Destination::Out, &format!("✗ {failure}\n"))?;
                    }
                    return Err(CliError::Failed("undeploy incomplete; app kept".into()));
                }
                format!(
                    "✓ deleted {name} {} endpoints removed\n",
                    result.endpoints_deleted
                )
            }
        };
        streams.write(Destination::Out, &output)
    }
}

pub(super) async fn execute(args: &ArgMatches, streams: &mut Streams<'_>) -> CliResult<()> {
    if let Some(args) = args.subcommand_matches("deploy") {
        return super::deployment::execute(args, streams).await;
    }
    if matches!(args.subcommand_name(), Some("env" | "undeploy")) {
        return super::environment::execute(args, streams).await;
    }
    let operation = Operation::read(args, &mut super::interaction::Terminal)?;
    let mut api = AppsApiClient::new(crate::Client::from_env()?)?;
    tokio::select! {
        result = operation.execute(&mut api, streams) => result,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(crate::Error::from)?;
            Err(CliError::Aborted)
        }
    }
}

// A private seam for replaying the source CLI's mocked management calls.
trait Backend: Send {
    fn list(&mut self) -> impl Future<Output = crate::Result<Vec<Value>>> + Send;
    fn get(&mut self, name: &str) -> impl Future<Output = crate::Result<Value>> + Send;
    fn delete(
        &mut self,
        name: &str,
        events: &EventSink,
    ) -> impl Future<Output = crate::Result<UndeployResult>> + Send;
}
impl Backend for AppsApiClient {
    async fn list(&mut self) -> crate::Result<Vec<Value>> {
        manage::list_apps(self).await
    }
    async fn get(&mut self, name: &str) -> crate::Result<Value> {
        manage::get_app(name, self).await
    }
    async fn delete(&mut self, name: &str, events: &EventSink) -> crate::Result<UndeployResult> {
        manage::delete_app(name, self, Some(events)).await
    }
}

async fn delete(
    name: &str,
    api: &mut impl Backend,
    streams: &mut Streams<'_>,
) -> CliResult<UndeployResult> {
    cleanup(
        streams,
        |events| async move { api.delete(name, &events).await },
    )
    .await
}

pub(super) async fn cleanup<T, Fut: Future<Output = crate::Result<T>>>(
    streams: &mut Streams<'_>,
    operation: impl FnOnce(EventSink) -> Fut,
) -> CliResult<T> {
    // Keep output on this task; the SDK observer owns only a channel sender.
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let events = EventSink::new(move |event, args| {
        let name = args.first().map(python_string).unwrap_or_default();
        let message = match event {
            "deleting" => format!("  deleting {name}\n"),
            "delete_failed" => format!(" ! could not delete {name}\n"),
            _ => return Ok(()),
        };
        let _ = sender.send(message);
        Ok(())
    });
    let deletion = operation(events);
    tokio::pin!(deletion);
    let result = loop {
        tokio::select! {
            result = &mut deletion => break result,
            Some(message) = receiver.recv() => streams.write(Destination::Out, &message)?,
        }
    };
    // A final synchronous event can precede completion in the same poll.
    while let Ok(message) = receiver.try_recv() {
        streams.write(Destination::Out, &message)?;
    }
    Ok(result?)
}

fn environments(entry: &Value) -> crate::Result<&[Value]> {
    match entry.get("flashEnvironments").filter(|v| truthy(v)) {
        None => Ok(&[]),
        Some(value) => value.as_array().map(Vec::as_slice).ok_or_else(|| {
            crate::Error::InvalidResponse("flashEnvironments must be an array".into())
        }),
    }
}
fn show_list(entries: &[Value]) -> CliResult<String> {
    if entries.is_empty() {
        return Ok("\n  no apps deployed. run rp flash deploy\n\n".into());
    }
    let rows = entries
        .iter()
        .map(|entry| {
            let name = required_str(entry, "name")?;
            let envs = environments(entry)?
                .iter()
                .map(|env| required_str(env, "name"))
                .collect::<crate::Result<Vec<_>>>()?
                .join(", ");
            Ok((name, envs))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let width = rows
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let mut output = String::from("\n");
    for (name, envs) in rows {
        let envs = if envs.is_empty() { "-" } else { &envs };
        writeln!(output, "  {name:<width$}  {envs}").expect("writing to a string cannot fail");
    }
    output.push('\n');
    Ok(output)
}
fn show_app(entry: &Value) -> CliResult<String> {
    let mut output = format!("\n  {}\n", required_str(entry, "name")?);
    let rows = environments(entry)?
        .iter()
        .map(|env| {
            let name = required_str(env, "name")?;
            let build = match env.get("activeBuildId").filter(|v| truthy(v)) {
                None => "no build",
                Some(value) => value.as_str().ok_or_else(|| {
                    crate::Error::InvalidResponse("activeBuildId must be a string".into())
                })?,
            };
            Ok((name, build.chars().take(12).collect::<String>()))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let width = rows
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    for (name, build) in rows {
        writeln!(output, "    {name:<width$}  {build}").expect("writing to a string cannot fail");
    }
    output.push('\n');
    Ok(output)
}

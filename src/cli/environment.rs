use super::{
    CliError, CliResult,
    interaction::Interaction,
    streams::{Destination, Streams},
};
use crate::{
    Value,
    apps::{
        AppsApiClient,
        discovery::{self, DiscoveryError},
        events::EventSink,
        manage::{self, EnvironmentDisposition, UndeployResult},
    },
    client::{required_str, truthy},
    control::python_string,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::{fmt::Write, future::Future, path::PathBuf};

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;

fn app_arg() -> Arg {
    Arg::new("app")
        .long("app")
        .short('a')
        .allow_hyphen_values(true)
}
fn yes_arg() -> Arg {
    Arg::new("yes")
        .long("yes")
        .short('y')
        .action(ArgAction::SetTrue)
}
pub(super) fn command() -> Command {
    let mut group = Command::new("env")
        .about("Manage app environments")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for name in ["list", "get", "add", "delete"] {
        let mut command = Command::new(name).args_override_self(true).arg(app_arg());
        if name != "list" {
            command = command.arg(Arg::new("name").required(true));
        }
        if name == "delete" {
            command = command.arg(yes_arg());
        }
        group = group.subcommand(command);
    }
    group
}
pub(super) fn undeploy_command() -> Command {
    Command::new("undeploy")
        .about("Delete an environment's endpoints and environment; keep app and builds")
        .args_override_self(true)
        .arg(app_arg())
        .arg(yes_arg())
        .arg(
            Arg::new("name")
                .long("env")
                .short('e')
                .allow_hyphen_values(true)
                .default_value("default"),
        )
}
fn arguments(matches: &ArgMatches) -> CliResult<(&str, &ArgMatches)> {
    match matches.subcommand() {
        Some(("env", args)) => args
            .subcommand()
            .ok_or_else(|| CliError::Usage("an environment command is required".into())),
        Some(("undeploy", args)) => Ok(("undeploy", args)),
        _ => Err(CliError::Usage("an environment command is required".into())),
    }
}
async fn resolve_app_name(
    app: Option<&str>,
    directory: impl FnOnce() -> std::io::Result<PathBuf>,
) -> CliResult<String> {
    if let Some(app) = app.filter(|name| !name.is_empty()) {
        return Ok(app.into());
    }
    let mut apps = match discovery::discover_apps(&directory().map_err(crate::Error::from)?).await {
        Ok(apps) => apps,
        Err(crate::Error::Discovery(DiscoveryError::Interrupted)) => return Err(CliError::Aborted),
        Err(crate::Error::Discovery(_)) => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    if apps.len() == 1 {
        return Ok(apps.remove(0).definition.name);
    }
    Err(CliError::Failed(
        "pass --app <name> (no unique app found in the current directory)".into(),
    ))
}

enum Operation {
    List,
    Get(String),
    Add(String),
    Delete(String),
    Undeploy(String),
}
impl Operation {
    fn read(
        command: &str,
        args: &ArgMatches,
        app: &str,
        ui: &mut impl Interaction,
    ) -> CliResult<Self> {
        if command == "list" {
            return Ok(Self::List);
        }
        let name = args
            .get_one::<String>("name")
            .ok_or_else(|| CliError::Usage("pass an ENV_NAME".into()))?;
        match command {
            "get" => Ok(Self::Get(name.clone())),
            "add" => Ok(Self::Add(name.clone())),
            "delete" | "undeploy" => {
                let action = if command == "delete" {
                    "undeploy and delete"
                } else {
                    "undeploy"
                };
                ui.confirm(&format!("{action} '{app}/{name}'?"), args.get_flag("yes"))?;
                Ok(if command == "delete" {
                    Self::Delete(name.clone())
                } else {
                    Self::Undeploy(name.clone())
                })
            }
            _ => Err(CliError::Usage("unknown environment command".into())),
        }
    }
    async fn execute(
        &self,
        app: &str,
        api: &mut impl Backend,
        streams: &mut Streams<'_>,
    ) -> CliResult<()> {
        let output = match self {
            Self::List => show_list(app, &api.app(app).await?)?,
            Self::Get(name) => show_environment(app, &api.environment(app, name).await?)?,
            Self::Add(name) => {
                let entry = api.app(app).await?;
                api.create(required_str(&entry, "id")?, name).await?;
                format!("✓ created {app}/{name}\n")
            }
            Self::Delete(name) | Self::Undeploy(name) => {
                let result = super::flash::cleanup(streams, |events| async move {
                    api.undeploy(app, name, &events).await
                })
                .await?;
                if !result.failures.is_empty() {
                    for failure in result.failures {
                        streams.write(Destination::Out, &format!("✗ {failure}\n"))?;
                    }
                    return Err(CliError::Failed(
                        if matches!(self, Self::Delete(_)) {
                            "undeploy incomplete; environment kept"
                        } else {
                            "undeploy incomplete"
                        }
                        .into(),
                    ));
                }
                let action = if matches!(self, Self::Delete(_)) {
                    "deleted"
                } else {
                    "undeployed"
                };
                format!(
                    "✓ {action} {app}/{name} {} endpoints removed\n",
                    result.endpoints_deleted
                )
            }
        };
        streams.write(Destination::Out, &output)
    }
}
pub(super) async fn execute(args: &ArgMatches, streams: &mut Streams<'_>) -> CliResult<()> {
    let (command, args) = arguments(args)?;
    let app = resolve_app_name(
        args.get_one::<String>("app").map(String::as_str),
        std::env::current_dir,
    )
    .await?;
    let operation = Operation::read(command, args, &app, &mut super::interaction::Terminal)?;
    let mut api = AppsApiClient::new(crate::Client::from_env()?)?;
    tokio::select! {
        result = operation.execute(&app, &mut api, streams) => result,
        signal = tokio::signal::ctrl_c() => { signal.map_err(crate::Error::from)?; Err(CliError::Aborted) },
    }
}
trait Backend: Send {
    fn app(&mut self, name: &str) -> impl Future<Output = crate::Result<Value>> + Send;
    fn environment(
        &mut self,
        app: &str,
        env: &str,
    ) -> impl Future<Output = crate::Result<Value>> + Send;
    fn create(&mut self, id: &str, env: &str) -> impl Future<Output = crate::Result<Value>> + Send;
    fn undeploy(
        &mut self,
        app: &str,
        env: &str,
        events: &EventSink,
    ) -> impl Future<Output = crate::Result<UndeployResult>> + Send;
}
impl Backend for AppsApiClient {
    async fn app(&mut self, name: &str) -> crate::Result<Value> {
        manage::get_app(name, self).await
    }
    async fn environment(&mut self, app: &str, env: &str) -> crate::Result<Value> {
        manage::get_environment(app, env, self).await
    }
    async fn create(&mut self, id: &str, env: &str) -> crate::Result<Value> {
        self.create_environment(id, env).await
    }
    async fn undeploy(
        &mut self,
        app: &str,
        env: &str,
        events: &EventSink,
    ) -> crate::Result<UndeployResult> {
        manage::undeploy_environment(app, env, EnvironmentDisposition::Delete, self, Some(events))
            .await
    }
}
fn records<'a>(entry: &'a Value, field: &str) -> crate::Result<&'a [Value]> {
    if !entry.is_object() {
        return Err(crate::Error::InvalidResponse(
            "environment metadata must be an object".into(),
        ));
    }
    match entry.get(field).filter(|v| truthy(v)) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| crate::Error::InvalidResponse(format!("{field} must be an array"))),
    }
}
fn build(entry: &Value) -> crate::Result<String> {
    let value = match entry.get("activeBuildId").filter(|v| truthy(v)) {
        None => "-",
        Some(value) => value.as_str().ok_or_else(|| {
            crate::Error::InvalidResponse("activeBuildId must be a string".into())
        })?,
    };
    Ok(value.chars().take(12).collect())
}
fn timestamp(value: &Value) -> String {
    if truthy(value) {
        python_string(value).chars().take(10).collect()
    } else {
        "-".into()
    }
}
fn show_list(app: &str, entry: &Value) -> CliResult<String> {
    let envs = records(entry, "flashEnvironments")?;
    if envs.is_empty() {
        return Ok(format!(
            "\n  no environments in {app}. run rp flash deploy\n\n"
        ));
    }
    let rows = envs
        .iter()
        .map(|env| {
            Ok((
                required_str(env, "name")?,
                build(env)?,
                timestamp(&env["createdAt"]),
            ))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let width = rows
        .iter()
        .map(|(name, _, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let mut output = String::from("\n");
    for (name, build, date) in rows {
        writeln!(output, "  {name:<width$} {build}, {date}")
            .expect("writing to a string cannot fail");
    }
    output.push('\n');
    Ok(output)
}
fn show_environment(app: &str, entry: &Value) -> CliResult<String> {
    let mut output = format!(
        "\n  {app}/{} build {}\n",
        required_str(entry, "name")?,
        build(entry)?
    );
    let rows = records(entry, "endpoints")?
        .iter()
        .map(|ep| Ok((required_str(ep, "name")?, required_str(ep, "id")?)))
        .collect::<crate::Result<Vec<_>>>()?;
    let width = rows
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    for (name, id) in rows {
        writeln!(output, "    {name:<width$}  {id} ↗").expect("writing to a string cannot fail");
    }
    output.push('\n');
    Ok(output)
}

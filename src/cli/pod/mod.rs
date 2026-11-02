mod config;
mod options;
#[cfg(test)]
mod tests;
mod view;
mod wait;

use super::{
    CliError, CliResult, api,
    interaction::{Interaction, Terminal},
    logs, output,
    streams::{Destination, Streams},
};
use crate::{
    Client, Error,
    client::{required_str, segment},
    json,
};
use options::Options;
pub(super) use options::command;
use options::{Action, Operation};

fn path(id: &str) -> String {
    format!("/v2/pods/{}", segment(id))
}
fn usage(error: Error) -> CliError {
    match error {
        Error::InvalidInput(message) => CliError::Usage(message),
        other => other.into(),
    }
}
fn say(message: &str, as_json: bool, streams: &mut Streams<'_>) -> CliResult<()> {
    streams.write(
        if as_json {
            Destination::Err
        } else {
            Destination::Out
        },
        &format!("{message}\n"),
    )
}
async fn resolve_id(
    client: &Client,
    id: Option<&str>,
    action: Option<Action>,
    ui: &mut impl Interaction,
) -> CliResult<String> {
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        return Ok(id.into());
    }
    let mut choices = vec![];
    if ui.interactive() {
        for pod in api::list_all(client, "/v2/pods", "pods", &[]).await? {
            if action.is_some_and(|action| {
                !pod["actions"]
                    .as_array()
                    .is_some_and(|actions| actions.iter().any(|v| v == action.name()))
            }) {
                continue;
            }
            choices.push((
                required_str(&pod, "id")?.into(),
                format!(
                    "{}  {}",
                    required_str(&pod, "name")?,
                    required_str(&pod, "status")?.to_lowercase()
                ),
            ));
        }
    }
    ui.pick("pod", &choices, "pass a POD_ID")
}
pub(super) async fn execute(args: &clap::ArgMatches, streams: &mut Streams<'_>) -> CliResult<()> {
    let mut ui = Terminal;
    if let Some(args) = args.subcommand_matches("logs") {
        let id = args.get_one::<String>("pod-id").map(String::as_str);
        if id.is_none_or(str::is_empty) && !ui.interactive() {
            return Err(CliError::Usage("pass a POD_ID".into()));
        }
        let client = Client::from_env()?;
        return show_logs(args, client, &mut ui, streams).await;
    }
    let options = Options::read(args)?;
    options.validate_interaction(&ui)?;
    options
        .execute(
            &Client::from_env()?,
            &mut ui,
            streams,
            &mut wait::Waiter::default(),
        )
        .await
}
async fn show_logs(
    args: &clap::ArgMatches,
    client: Client,
    ui: &mut impl Interaction,
    streams: &mut Streams<'_>,
) -> CliResult<()> {
    let id = resolve_id(
        &client,
        args.get_one::<String>("pod-id").map(String::as_str),
        None,
        ui,
    )
    .await?;
    logs::Options::pod(args, id)
        .execute_with_client(client, streams)
        .await
}
impl Options {
    fn validate_interaction(&self, ui: &impl Interaction) -> CliResult<()> {
        if ui.interactive() {
            return Ok(());
        }
        let id = match &self.operation {
            Operation::Get(id) | Operation::Action { id, .. } | Operation::Terminate { id, .. } => {
                Some(id)
            }
            Operation::Update(options) => Some(&options.id),
            Operation::Create(options) if options.name.is_none() => {
                return Err(CliError::Usage("pass a NAME".into()));
            }
            _ => None,
        };
        if id.is_some_and(|id| id.as_ref().is_none_or(String::is_empty)) {
            return Err(CliError::Usage("pass a POD_ID".into()));
        }
        Ok(())
    }
    async fn execute(
        &self,
        client: &Client,
        ui: &mut impl Interaction,
        streams: &mut Streams<'_>,
        waiter: &mut wait::Waiter<impl wait::Clock>,
    ) -> CliResult<()> {
        let result = match &self.operation {
            Operation::List {
                name,
                statuses,
                include_cluster,
            } => {
                let params = if *include_cluster {
                    vec![("includeClusterPods", "true")]
                } else {
                    vec![]
                };
                let mut pods = vec![];
                for pod in api::list_all(client, "/v2/pods", "pods", &params).await? {
                    if name.as_ref().is_some_and(|name| {
                        !pod["name"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&name.to_lowercase())
                    }) {
                        continue;
                    }
                    if !statuses.is_empty() && !statuses.contains(&view::status(&pod)?) {
                        continue;
                    }
                    pods.push(pod);
                }
                return streams.write(
                    Destination::Out,
                    &if self.as_json {
                        output::json(&pods)?
                    } else {
                        view::list(&pods)?
                    },
                );
            }
            Operation::Get(id) => {
                let id = resolve_id(client, id.as_deref(), None, ui).await?;
                client
                    .run_rest_request("GET", &path(&id), None, &[])
                    .await?
            }
            Operation::Create(options) => {
                let name = match &options.name {
                    Some(name) => name.clone(),
                    None => ui.text_prompt("pod name")?,
                };
                let body = options.body(client, name, self.as_json, streams).await?;
                let since = waiter.now();
                let created = client
                    .run_rest_request("POST", "/v2/pods", Some(&body), &[])
                    .await?;
                let id = required_str(&created, "id")?;
                say(
                    &format!("created pod {} ({id})", required_str(&created, "name")?),
                    self.as_json,
                    streams,
                )?;
                if options.wait {
                    waiter.run(client, id, since, None).await?
                } else {
                    created
                }
            }
            Operation::Update(options) => {
                let id = resolve_id(client, options.id.as_deref(), None, ui).await?;
                let current = if options.container.needs_current() || !options.unset_env.is_empty()
                {
                    Some(
                        client
                            .run_rest_request("GET", &path(&id), None, &[])
                            .await?,
                    )
                } else {
                    None
                };
                let mut body = options
                    .container
                    .body(client, current.as_ref(), &options.unset_env)
                    .await?;
                if let Some(name) = options.name.as_ref().filter(|s| !s.is_empty()) {
                    body["name"] = json!(name);
                }
                if let Some(locked) = options.locked {
                    body["locked"] = json!(locked);
                }
                let updated = client
                    .run_rest_request("PATCH", &path(&id), Some(&body), &[])
                    .await?;
                say(&format!("updated pod {id}"), self.as_json, streams)?;
                updated
            }
            Operation::Action { id, action, wait } => {
                let id = resolve_id(client, id.as_deref(), Some(*action), ui).await?;
                let previous = if *wait && matches!(action, Action::Restart) {
                    Some(
                        client
                            .run_rest_request("GET", &path(&id), None, &[])
                            .await?,
                    )
                } else {
                    None
                };
                let since = waiter.now();
                let result = client
                    .run_rest_request(
                        "POST",
                        &format!("{}/action", path(&id)),
                        Some(&json!({"action":action.name()})),
                        &[],
                    )
                    .await?;
                say(
                    &format!("{} pod {id}", action.done()),
                    self.as_json,
                    streams,
                )?;
                if *wait {
                    waiter.run(client, &id, since, previous.as_ref()).await?
                } else {
                    result
                }
            }
            Operation::Terminate { id, yes } => {
                let id = resolve_id(client, id.as_deref(), Some(Action::Terminate), ui).await?;
                if !yes {
                    let record = client
                        .run_rest_request("GET", &path(&id), None, &[])
                        .await?;
                    ui.confirm(
                        &format!(
                            "terminate pod {id} ({})? its disk and persistent volume are deleted",
                            required_str(&record, "name")?
                        ),
                        false,
                    )?;
                }
                client
                    .run_rest_request("DELETE", &path(&id), None, &[])
                    .await?;
                return streams.write(Destination::Out, &format!("terminated pod {id}\n"));
            }
        };
        streams.write(Destination::Out, &view::show(&result, self.as_json)?)
    }
}

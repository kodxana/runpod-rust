mod job_follow;
mod jobs;
mod options;
mod run;
#[cfg(test)]
mod tests;
mod view;
mod watch;
use super::{
    CliError, CliResult, api,
    interaction::Interaction,
    output,
    streams::{Destination, Streams},
};
use crate::{
    Client, Error, Value,
    client::{required_str, truthy},
    configuration, json,
};
pub(super) use jobs::execute as execute_job;
use options::Document;
pub(super) use options::{Options, command};
pub(super) use run::RunOptions;
use std::io::Read;

fn path(id: &str, parts: &[&str]) -> String {
    const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    let segments = std::iter::once(id)
        .chain(parts.iter().copied())
        .map(|value| percent_encoding::utf8_percent_encode(value, SEGMENT).to_string())
        .collect::<Vec<_>>();
    format!("/v2/serverless/{}", segments.join("/"))
}
fn usage(error: Error) -> CliError {
    match error {
        Error::InvalidInput(message) => CliError::Usage(message),
        other => other.into(),
    }
}
fn normalized_name(name: &str) -> String {
    let name = name.to_lowercase();
    if cfg!(windows) {
        name.replace('\\', "/")
    } else {
        name
    }
}
// Endpoint names use fnmatch-style globs, where repeated stars are ordinary stars
// and an unclosed character class is literal. Matching itself stays in the glob crate.
fn name_pattern(name: &str) -> CliResult<glob::Pattern> {
    let chars: Vec<_> = normalized_name(name).chars().collect();
    let mut pattern = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '*' {
            pattern.push('*');
            while chars.get(index) == Some(&'*') {
                index += 1;
            }
        } else if chars[index] == '[' {
            let mut start = index + 1;
            if chars.get(start) == Some(&'!') {
                start += 1;
            }
            if chars.get(start) == Some(&']') {
                start += 1;
            }
            if let Some(end) = (start..chars.len()).find(|i| chars[*i] == ']') {
                pattern.extend(chars[index..=end].iter());
                index = end + 1;
            } else {
                pattern.push_str("[[]");
                index += 1;
            }
        } else {
            pattern.push(chars[index]);
            index += 1;
        }
    }
    glob::Pattern::new(&pattern).map_err(|e| CliError::Usage(format!("invalid name pattern: {e}")))
}
async fn pick(client: &Client, id: Option<&str>, ui: &mut impl Interaction) -> CliResult<String> {
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        return Ok(id.into());
    }
    if !ui.interactive() {
        return Err(CliError::Usage("pass an ENDPOINT_ID".into()));
    }
    let rows = api::list_all(client, "/v2/serverless", "endpoints", &[]).await?;
    let choices = rows
        .iter()
        .map(|e| {
            let id = required_str(e, "id")?;
            Ok((
                id.into(),
                format!(
                    "{id}  {}  {}",
                    e["name"].as_str().unwrap_or_default(),
                    view::kind(e).as_str().unwrap_or_default()
                ),
            ))
        })
        .collect::<crate::Result<Vec<_>>>()?;
    ui.pick("endpoint", &choices, "pass an ENDPOINT_ID")
}
impl Options {
    pub(super) fn validate_interaction(&self, ui: &impl Interaction) -> CliResult<()> {
        if matches!(self.action.as_str(), "get" | "edit")
            && self.id.as_deref().unwrap_or_default().is_empty()
            && !ui.interactive()
        {
            return Err(CliError::Usage("pass an ENDPOINT_ID".into()));
        }
        Ok(())
    }

    pub(super) async fn execute(
        &self,
        client: &Client,
        ui: &mut impl Interaction,
        streams: &mut Streams<'_>,
    ) -> CliResult<()> {
        self.execute_with_clock(client, ui, streams, &mut watch::Timer::default())
            .await
    }
    async fn execute_with_clock(
        &self,
        client: &Client,
        ui: &mut impl Interaction,
        streams: &mut Streams<'_>,
        clock: &mut impl watch::Clock,
    ) -> CliResult<()> {
        match self.action.as_str() {
            "list" => {
                let mut rows = api::list_all(client, "/v2/serverless", "endpoints", &[]).await?;
                if let Some(name) = self.config.name.as_deref().filter(|v| !v.is_empty()) {
                    let pattern = name_pattern(name)?;
                    let options = glob::MatchOptions {
                        case_sensitive: true,
                        require_literal_separator: false,
                        require_literal_leading_dot: false,
                    };
                    rows.retain(|row| {
                        pattern.matches_with(
                            &normalized_name(row["name"].as_str().unwrap_or_default()),
                            options,
                        )
                    });
                }
                if let Some(kind) = self.kind {
                    rows.retain(|row| view::kind(row) == kind.to_string());
                }
                streams.write(
                    Destination::Out,
                    &if self.json {
                        output::json(&rows)?
                    } else {
                        view::listing(&rows)?
                    },
                )
            }
            "get" => {
                let id = pick(client, self.id.as_deref(), ui).await?;
                let record = client
                    .run_rest_request("GET", &path(&id, &[]), None, &[])
                    .await?;
                streams.write(
                    Destination::Out,
                    &if self.json {
                        output::json(&record)?
                    } else {
                        view::detail(&record)?
                    },
                )
            }
            "create" => {
                let kind = self
                    .kind
                    .ok_or_else(|| CliError::Usage("pass --type".into()))?;
                let body = client
                    .create_endpoint_config(&self.config, kind)
                    .await
                    .map_err(usage)?;
                let created = client
                    .run_rest_request("POST", "/v2/serverless", Some(&body), &[])
                    .await?;
                if self.json {
                    return streams.write(Destination::Out, &output::json(&created)?);
                }
                streams.write(
                    Destination::Out,
                    &format!(
                        "✓ created {} {}\n",
                        required_str(&created, "name")?,
                        required_str(&created, "id")?
                    ),
                )?;
                streams.write(Destination::Out, &view::detail(&created)?)
            }
            "update" => {
                let id = self.required_id()?;
                let current = client
                    .run_rest_request("GET", &path(id, &[]), None, &[])
                    .await?;
                let patch = client
                    .update_endpoint_config(&self.config, &current)
                    .await
                    .map_err(usage)?;
                if !truthy(&patch) {
                    return Err(CliError::Usage(
                        "nothing to update. pass an option (see --help) or use rp endpoint edit"
                            .into(),
                    ));
                }
                self.apply(client, id, &patch, self.wait, streams, clock)
                    .await
            }
            "edit" => {
                let id = pick(client, self.id.as_deref(), ui).await?;
                let current = client
                    .run_rest_request("GET", &path(&id, &[]), None, &[])
                    .await?;
                let before = configuration::mutable_endpoint(&current)?;
                let after = if let Some(document) = &self.document {
                    let mut text = String::new();
                    match document {
                        Document::File(file) => {
                            let mut reader = file;
                            reader.read_to_string(&mut text)
                        }
                        Document::Stdin => std::io::stdin().read_to_string(&mut text),
                    }
                    .map_err(Error::from)?;
                    Some(configuration::validate_endpoint_document(&text, &before)?)
                } else if !ui.interactive() {
                    return Err(CliError::Usage("edit opens $EDITOR and needs an interactive terminal. pass --file or use rp endpoint update".into()));
                } else {
                    edit_document(&before, ui, streams)?
                };
                let patch = after
                    .as_ref()
                    .map(|after| configuration::endpoint_patch(&before, after))
                    .transpose()?
                    .unwrap_or_else(|| json!({}));
                if !truthy(&patch) {
                    return streams.write(
                        Destination::Out,
                        if self.json { "{}\n" } else { "no changes\n" },
                    );
                }
                if !self.json {
                    streams.write(Destination::Out, "patch:\n")?;
                    streams.write(Destination::Out, &output::json(&patch)?)?;
                }
                ui.confirm(&format!("apply to endpoint {id}?"), self.yes)?;
                self.apply(
                    client,
                    &id,
                    &patch,
                    !(self.no_follow || self.json),
                    streams,
                    clock,
                )
                .await
            }
            "scale" => {
                let id = self.required_id()?;
                let current = client
                    .run_rest_request("GET", &path(id, &[]), None, &[])
                    .await?;
                let patch =
                    configuration::scale_patch(&self.config.scale, &current).map_err(usage)?;
                if !truthy(&patch) {
                    return Err(CliError::Usage(
                        "pass --min-workers, --max-workers, or a scaler option".into(),
                    ));
                }
                let updated = client
                    .run_rest_request("PATCH", &path(id, &[]), Some(&patch), &[])
                    .await?;
                if self.json {
                    return streams.write(Destination::Out, &output::json(&updated)?);
                }
                streams.write(
                    Destination::Out,
                    &format!("✓ scaled {}\n", required_str(&updated, "name")?),
                )?;
                streams.write(Destination::Out, &view::scale(&updated, id)?)
            }
            "delete" => {
                let id = self.required_id()?;
                let current = client
                    .run_rest_request("GET", &path(id, &[]), None, &[])
                    .await?;
                let name = required_str(&current, "name")?;
                ui.confirm(&format!("delete endpoint '{name}' ({id})?"), self.yes)?;
                client
                    .run_rest_request("DELETE", &path(id, &[]), None, &[])
                    .await?;
                streams.write(Destination::Out, &format!("✓ deleted {name} {id}\n"))
            }
            "workers" => {
                watch::workers(
                    client,
                    self.required_id()?,
                    self.watch,
                    self.interval,
                    self.json,
                    clock,
                    streams,
                )
                .await
            }
            "releases" => {
                let path = path(self.required_id()?, &["releases"]);
                let state = client
                    .run_rest_request("GET", &path, None, &[("limit", "1")])
                    .await?;
                let records = api::list_all(client, &path, "releases", &[]).await?;
                let text = if self.json {
                    output::json(
                        &json!({"endpointVersion":state["endpointVersion"],"rollout":state["rollout"],"releases":records}),
                    )?
                } else {
                    view::releases(&state, &records)?
                };
                streams.write(Destination::Out, &text)
            }
            _ => Err(CliError::Usage("unknown endpoint command".into())),
        }
    }
    fn required_id(&self) -> CliResult<&str> {
        self.id
            .as_deref()
            .ok_or_else(|| CliError::Usage("pass an ENDPOINT_ID".into()))
    }
    async fn apply(
        &self,
        client: &Client,
        id: &str,
        patch: &Value,
        mut follow: bool,
        streams: &mut Streams<'_>,
        clock: &mut impl watch::Clock,
    ) -> CliResult<()> {
        if patch.as_object().is_some_and(|p| {
            p.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "name" | "workers" | "scaling" | "timeout" | "flashboot"
                )
            })
        }) {
            follow = false;
        }
        let mut previous = Value::Null;
        if follow {
            match client
                .run_rest_request("GET", &path(id, &["releases"]), None, &[("limit", "1")])
                .await
            {
                Ok(state) => previous = state["endpointVersion"].clone(),
                Err(error) => {
                    streams.write(
                        Destination::Err,
                        &format!("warning: cannot follow rollout: {error}\n"),
                    )?;
                    follow = false;
                }
            }
        }
        let updated = client
            .run_rest_request("PATCH", &path(id, &[]), Some(patch), &[])
            .await?;
        streams.write(
            Destination::Out,
            &if self.json {
                output::json(&updated)?
            } else {
                format!("✓ updated {} {id}\n", required_str(&updated, "name")?)
            },
        )?;
        if follow {
            watch::follow(
                client,
                id,
                &previous,
                self.timeout,
                clock,
                streams,
                if self.json {
                    Destination::Err
                } else {
                    Destination::Out
                },
            )
            .await?;
        }
        Ok(())
    }
}
fn edit_document(
    before: &Value,
    ui: &mut impl Interaction,
    streams: &mut Streams<'_>,
) -> CliResult<Option<Value>> {
    let mut text = output::json(before)?;
    loop {
        let Some(edited) = ui.edit(&text)? else {
            return Ok(None);
        };
        match configuration::validate_endpoint_document(&edited, before) {
            Ok(document) => return Ok(Some(document)),
            Err(error) => {
                streams.write(Destination::Out, &format!("✗ {error}\n"))?;
                if !ui.retry_edit()? {
                    return Err(error.into());
                }
                text = edited;
            }
        }
    }
}

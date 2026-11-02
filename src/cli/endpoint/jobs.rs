use super::super::{
    CliError, CliResult,
    interaction::Interaction,
    output,
    streams::{Destination, Streams},
};
use crate::{
    Client, Error, Value,
    client::{segment, truthy},
    control::python_string,
};
use clap::ArgMatches;
use std::time::Duration;

#[derive(Clone, Copy)]
enum Action {
    Get,
    Cancel,
    Retry,
    Purge,
}
impl Action {
    fn route(self) -> &'static str {
        match self {
            Self::Get => "status",
            Self::Cancel => "cancel",
            Self::Retry => "retry",
            Self::Purge => "purge-queue",
        }
    }
}

fn argument<'a>(args: &'a ArgMatches, name: &str) -> CliResult<&'a str> {
    args.get_one::<String>(name)
        .map(String::as_str)
        .ok_or_else(|| CliError::Usage(format!("missing {name}")))
}

pub(in crate::cli) async fn execute(
    args: &ArgMatches,
    ui: &mut impl Interaction,
    streams: &mut Streams<'_>,
) -> CliResult<()> {
    if let Some(("job", args)) = args.subcommand()
        && let Some((operation @ ("wait" | "stream"), args)) = args.subcommand()
    {
        let follow = super::job_follow::Follow {
            endpoint_id: argument(args, "endpoint-id")?,
            job_id: argument(args, "job-id")?,
            stream: operation == "stream",
            as_json: args.get_flag("json"),
            timeout: args.try_get_one::<f64>("timeout").ok().flatten().copied(),
        };
        return follow.run(&Client::from_env()?, streams).await;
    }
    let (action, args) = match args.subcommand() {
        Some(("purge", args)) => (Action::Purge, args),
        Some(("job", args)) => match args.subcommand() {
            Some(("get", args)) => (Action::Get, args),
            Some(("cancel", args)) => (Action::Cancel, args),
            Some(("retry", args)) => (Action::Retry, args),
            _ => return Err(CliError::Usage("a job command is required".into())),
        },
        _ => return Err(CliError::Usage("a job or purge command is required".into())),
    };
    let endpoint_id = argument(args, "endpoint-id")?;
    let job_id = if matches!(action, Action::Purge) {
        ui.confirm(
            &format!("remove every queued job from {endpoint_id}?"),
            args.get_flag("yes"),
        )?;
        None
    } else {
        Some(argument(args, "job-id")?)
    };
    let method = if matches!(action, Action::Get) {
        reqwest::Method::GET
    } else {
        reqwest::Method::POST
    };
    let record = request(
        &Client::from_env()?,
        method,
        action.route(),
        endpoint_id,
        job_id,
        None,
        Duration::from_secs(10),
    )
    .await?;
    let text = if args.get_flag("json") {
        output::json(&record)?
    } else {
        match action {
            Action::Get => super::view::job(&record)?,
            Action::Cancel | Action::Retry => {
                let id = if matches!(action, Action::Retry) && truthy(&record["id"]) {
                    python_string(&record["id"])
                } else {
                    job_id.unwrap_or_default().into()
                };
                format!("✓ job {id} {}\n", python_string(&record["status"]))
            }
            Action::Purge => format!(
                "✓ purged {} queued jobs from {endpoint_id}\n",
                python_string(record.get("removed").unwrap_or(&Value::from(0))),
            ),
        }
    };
    streams.write(Destination::Out, &text)
}

pub(super) async fn request(
    client: &Client,
    method: reqwest::Method,
    operation: &str,
    endpoint_id: &str,
    job_id: Option<&str>,
    body: Option<&Value>,
    timeout: Duration,
) -> crate::Result<Value> {
    let mut path = format!("{}/{operation}", segment(endpoint_id));
    if let Some(id) = job_id.filter(|id| !id.is_empty()) {
        path.push('/');
        path.push_str(&segment(id));
    }
    let mut request = client
        .http
        .request(
            method.clone(),
            format!(
                "{}/{path}",
                client.config.endpoint_base_url.trim_end_matches('/')
            ),
        )
        .timeout(timeout);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Error::Authentication(
            "401 Unauthorized | Make sure Runpod API key is set and valid.".into(),
        ));
    }
    let text = response.text().await?;
    if status.is_client_error() || status.is_server_error() {
        let detail = if text.trim().is_empty() {
            status.canonical_reason().unwrap_or_default()
        } else {
            text.trim()
        };
        return Err(Error::Query {
            message: format!("{} {detail}", status.as_u16()).trim_end().into(),
            query: format!("{method} {path}"),
            status_code: Some(status.as_u16()),
            errors: Box::new(Value::Null),
        });
    }
    Ok(serde_json::from_str(&text)?)
}

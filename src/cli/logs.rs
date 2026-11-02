use super::{
    CliResult,
    streams::{Destination, Streams},
};
use crate::{
    Client, Error, Value,
    apps::logs::{EventIdObserver, LogStreamOptions, LogsClient},
    client::{required_str, segment},
};
use chrono::{DateTime, Utc};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use futures_util::{
    FutureExt, StreamExt,
    future::BoxFuture,
    stream::{BoxStream, SelectAll},
};
use std::{collections::HashSet, future::Future, time::Duration};

#[cfg(test)]
#[path = "logs_tests.rs"]
mod tests;

pub(super) fn command(id: &'static str) -> Command {
    let mut command = Command::new("logs")
        .about("Show historical logs or follow live output")
        .args_override_self(true)
        .arg(Arg::new(id).required(true))
        .arg(
            Arg::new("follow")
                .long("follow")
                .short('f')
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("source")
                .long("source")
                .value_parser(["container", "system"]),
        )
        .arg(
            Arg::new("tail")
                .long("tail")
                .allow_hyphen_values(true)
                .value_parser(value_parser!(i64).range(0..=5000))
                .default_value("100"),
        )
        .arg(Arg::new("since").long("since"))
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue));
    if id == "endpoint-id" {
        command = command.arg(Arg::new("worker").long("worker").action(ArgAction::Append));
    }
    command
}

enum Target {
    Pod(String),
    Endpoint { id: String, workers: Vec<String> },
}
pub(super) struct Options {
    target: Target,
    follow: bool,
    source: Option<String>,
    tail: i64,
    since: Option<String>,
    as_json: bool,
}
#[derive(Clone, Copy)]
struct Timing {
    started: DateTime<Utc>,
    idle: Duration,
    reconnect: Duration,
    poll: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            started: Utc::now(),
            idle: Duration::from_secs(3),
            reconnect: Duration::from_secs(1),
            poll: Duration::from_secs(5),
        }
    }
}
trait Reader: Sync {
    fn workers(&self, endpoint: &str) -> impl Future<Output = crate::Result<Vec<String>>> + Send;
    fn stream(
        &self,
        worker: &str,
        options: LogStreamOptions,
        observer: EventIdObserver,
    ) -> BoxStream<'static, crate::Result<Value>>;
}
struct HttpReader {
    client: Client,
    logs: LogsClient,
}
impl Reader for HttpReader {
    async fn workers(&self, endpoint: &str) -> crate::Result<Vec<String>> {
        let response = self
            .client
            .run_rest_request(
                "GET",
                &format!("/v2/serverless/{}/workers", segment(endpoint)),
                None,
                &[],
            )
            .await?;
        response
            .get("workers")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::InvalidResponse("workers must be an array".into()))?
            .iter()
            .map(|worker| required_str(worker, "id").map(str::to_owned))
            .collect()
    }
    fn stream(
        &self,
        worker: &str,
        options: LogStreamOptions,
        observer: EventIdObserver,
    ) -> BoxStream<'static, crate::Result<Value>> {
        self.logs.stream_pod_logs(worker, options, Some(observer))
    }
}
enum Message {
    Event(String, Value),
    Done(String, CliResult<()>),
}
enum Discovery {
    Due,
    Ready(Vec<String>),
}

impl Options {
    pub fn read(args: &ArgMatches, id: &str) -> Self {
        let value = args
            .get_one::<String>(id)
            .expect("required resource ID")
            .clone();
        Self {
            target: if id == "pod-id" {
                Target::Pod(value)
            } else {
                Target::Endpoint {
                    id: value,
                    workers: args
                        .get_many::<String>("worker")
                        .map(|v| v.cloned().collect())
                        .unwrap_or_default(),
                }
            },
            follow: args.get_flag("follow"),
            source: args.get_one::<String>("source").cloned(),
            tail: *args.get_one::<i64>("tail").expect("default tail"),
            since: args.get_one::<String>("since").cloned(),
            as_json: args.get_flag("json"),
        }
    }
    pub async fn execute(&self, streams: &mut Streams<'_>) -> CliResult<()> {
        let client = Client::from_env()?;
        let reader = HttpReader {
            logs: LogsClient::new(client.clone())?,
            client,
        };
        tokio::select! {
            result=self.show(&reader,streams,Timing::default())=>result,
            signal=tokio::signal::ctrl_c()=>{signal.map_err(Error::from)?; Ok(())}
        }
    }
    fn endpoint(&self) -> Option<&str> {
        match &self.target {
            Target::Pod(_) => None,
            Target::Endpoint { id, .. } => Some(id),
        }
    }
    async fn workers(&self, reader: &impl Reader) -> CliResult<Vec<String>> {
        match &self.target {
            Target::Pod(id) => Ok(vec![id.clone()]),
            Target::Endpoint { id, workers } => Ok(reader
                .workers(id)
                .await?
                .into_iter()
                .filter(|id| workers.is_empty() || workers.contains(id))
                .collect()),
        }
    }
    fn consume<'a>(
        &self,
        reader: &'a impl Reader,
        worker: String,
        timing: Timing,
    ) -> BoxStream<'a, Message> {
        let follow = self.follow;
        let endpoint = self.endpoint().is_some();
        let options = LogStreamOptions {
            endpoint_id: self.endpoint().map(str::to_owned),
            log_type: self.source.clone(),
            tail: self.tail,
            since: self.since.clone(),
            last_event_id: None,
            idle_timeout: (!follow).then_some(timing.idle),
        };
        Box::pin(async_stream::stream! {
            let worker_ref = &worker;
            let mut events = Box::pin(async_stream::try_stream! {
                let (cursor, latest) = tokio::sync::watch::channel(None::<String>);
                loop {
                    let options = LogStreamOptions {
                        last_event_id: latest.borrow().clone(),
                        ..options.clone()
                    };
                    let cursor = cursor.clone();
                    let mut stream = reader.stream(worker_ref, options, Box::new(move |id| {
                        cursor.send_replace(Some(id.into()));
                        Ok(())
                    }));
                    while let Some(event) = stream.next().await {
                        match event {
                            Ok(event) => {
                                if !follow {
                                    let timestamp = DateTime::parse_from_rfc3339(required_str(&event, "ts")?)
                                        .map_err(|error| Error::InvalidResponse(format!("invalid log timestamp: {error}")))?;
                                    if timestamp >= timing.started {
                                        return;
                                    }
                                }
                                yield event;
                            }
                            Err(error) => {
                                if endpoint && error.status_code() == Some(404) {
                                    return;
                                }
                                if !follow || !retryable(&error) {
                                    Err(error)?;
                                }
                                break;
                            }
                        }
                    }
                    drop(stream);
                    if !follow {
                        return;
                    }
                    tokio::time::sleep(timing.reconnect).await;
                }
            });
            while let Some(event) = events.next().await {
                match event {
                    Ok(event) => yield Message::Event(worker.clone(), event),
                    Err(error) => {
                        yield Message::Done(worker.clone(), Err(super::CliError::Api(error)));
                        return;
                    }
                }
            }
            yield Message::Done(worker.clone(), Ok(()));
        })
    }
    async fn show(
        &self,
        reader: &impl Reader,
        streams: &mut Streams<'_>,
        timing: Timing,
    ) -> CliResult<()> {
        let mut active = HashSet::new();
        let mut incoming = SelectAll::new();
        let mut width = 0;
        let ids = self.workers(reader).await?;
        self.attach(reader, ids, timing, &mut active, &mut incoming, &mut width);
        if active.is_empty() && !self.as_json {
            streams.write(
                Destination::Out,
                if self.follow {
                    "waiting for workers ...\n"
                } else {
                    "no workers\n"
                },
            )?;
        }
        if !self.follow {
            let mut records = vec![];
            while let Some(message) = incoming.next().await {
                match message {
                    Message::Event(worker, event) => records.push((worker, event)),
                    Message::Done(_, result) => result?,
                }
            }
            // The source sorts the original timestamp strings, not the parsed instants.
            records.sort_by(|a, b| a.1["ts"].as_str().cmp(&b.1["ts"].as_str()));
            for (worker, event) in records {
                self.emit(&worker, event, width, streams)?;
            }
            return Ok(());
        }
        let mut completed = vec![];
        let mut discovery = poll_after(timing.poll);
        loop {
            tokio::select! {
                Some(message)=incoming.next(), if !incoming.is_empty()=>match message {
                    Message::Event(worker,event)=>self.emit(&worker,event,width,streams)?,
                    Message::Done(worker,result)=>completed.push((worker,result)),
                },
                result=&mut discovery=>match result? {
                    Discovery::Due=>{
                        for (worker,result) in completed.drain(..) {result?;active.remove(&worker);}
                        discovery=async {self.workers(reader).await.map(Discovery::Ready)}.boxed();
                    }
                    Discovery::Ready(ids)=>{
                        self.attach(reader,ids,timing,&mut active,&mut incoming,&mut width);
                        discovery=poll_after(timing.poll);
                    }
                }
            }
        }
    }
    fn attach<'a>(
        &self,
        reader: &'a impl Reader,
        ids: Vec<String>,
        timing: Timing,
        active: &mut HashSet<String>,
        incoming: &mut SelectAll<BoxStream<'a, Message>>,
        width: &mut usize,
    ) {
        for id in ids {
            *width = (*width).max(id.chars().count());
            if active.insert(id.clone()) {
                incoming.push(self.consume(reader, id, timing));
            }
        }
    }
    fn emit(
        &self,
        worker: &str,
        event: Value,
        width: usize,
        streams: &mut Streams<'_>,
    ) -> CliResult<()> {
        let text = if self.as_json {
            let event = if self.endpoint().is_some() {
                let object = event
                    .as_object()
                    .ok_or_else(|| Error::InvalidResponse("log event must be an object".into()))?;
                let mut merged = serde_json::Map::new();
                merged.insert("worker".into(), Value::String(worker.into()));
                merged.extend(object.clone());
                Value::Object(merged)
            } else {
                event
            };
            format!("{}\n", serde_json::to_string(&event).map_err(Error::from)?)
        } else {
            let prefix = if self.endpoint().is_some() {
                format!("{worker:<width$} ")
            } else {
                String::new()
            };
            format!(
                "{prefix}[{}] {}\n",
                required_str(&event, "source")?,
                required_str(&event, "line")?
            )
        };
        streams.write(Destination::Out, &text)
    }
}
fn retryable(error: &Error) -> bool {
    matches!(error.status_code(), Some(429 | 500..=599))
        // A server disappearing before headers is a request error in reqwest.
        || matches!(error,Error::Http(error) if error.is_connect() || error.is_timeout() || error.is_request())
}
fn poll_after(delay: Duration) -> BoxFuture<'static, CliResult<Discovery>> {
    async move {
        tokio::time::sleep(delay).await;
        Ok(Discovery::Due)
    }
    .boxed()
}

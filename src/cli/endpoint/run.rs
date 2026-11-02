use super::super::{
    CliError, CliResult, output,
    streams::{Destination, Streams},
};
use crate::{
    Client, Error, Value,
    client::{required_str, truthy},
    control::python_string,
};
use clap::ArgMatches;
use std::{io::Read, time::Duration};

#[derive(Clone, Copy)]
enum Mode {
    Wait,
    Stream,
    Async,
}

pub(in crate::cli) struct RunOptions {
    endpoint_id: String,
    body: Option<Value>,
    mode: Mode,
    json: bool,
    webhook: Option<String>,
    execution_timeout: Option<u64>,
    ttl: Option<u64>,
    low_priority: bool,
    timeout: Option<f64>,
    path: Option<String>,
    method: Option<String>,
}
fn string(args: &ArgMatches, name: &str) -> Option<String> {
    args.get_one::<String>(name).cloned()
}
impl RunOptions {
    pub(in crate::cli) fn read(args: &ArgMatches) -> CliResult<Self> {
        let input = string(args, "input");
        // File options are opened before callback-level flag checks, as Click does.
        let mut file = input
            .as_deref()
            .filter(|path| *path != "-")
            .map(|path| {
                std::fs::File::open(path).map_err(|error| {
                    CliError::Usage(format!("Invalid value for --input: '{path}': {error}"))
                })
            })
            .transpose()?;
        let mode = match (args.get_flag("async"), args.get_flag("stream")) {
            (true, true) => return Err(CliError::Usage("use either --async or --stream".into())),
            (true, false) => Mode::Async,
            (false, true) => Mode::Stream,
            (false, false) => Mode::Wait,
        };
        let data = string(args, "data");
        if input.is_some() && data.as_deref().is_some_and(|value| !value.is_empty()) {
            return Err(CliError::Usage("use either --input or --data".into()));
        }
        let text = if input.is_some() {
            let mut text = String::new();
            if let Some(file) = &mut file {
                file.read_to_string(&mut text).map_err(Error::from)?;
            } else {
                std::io::stdin()
                    .read_to_string(&mut text)
                    .map_err(Error::from)?;
            }
            Some(text)
        } else {
            data
        };
        let body = text
            .map(|text| {
                serde_json::from_str::<Value>(&text).map_err(|error| {
                    CliError::Usage(format!("request body is not valid JSON: {error}"))
                })
            })
            .transpose()?
            .filter(|value| !value.is_null());
        Ok(Self {
            endpoint_id: string(args, "endpoint-id")
                .ok_or_else(|| CliError::Usage("pass an ENDPOINT_ID".into()))?,
            body,
            mode,
            json: args.get_flag("json"),
            webhook: string(args, "webhook"),
            execution_timeout: args.get_one::<u64>("execution-timeout").copied(),
            ttl: args.get_one::<u64>("ttl").copied(),
            low_priority: args.get_flag("low-priority"),
            timeout: args.get_one::<f64>("timeout").copied(),
            path: string(args, "path"),
            method: string(args, "method"),
        })
    }
    pub(in crate::cli) async fn execute(self, streams: &mut Streams<'_>) -> CliResult<()> {
        let client = Client::from_env()?;
        let record = client
            .run_rest_request("GET", &super::path(&self.endpoint_id, &[]), None, &[])
            .await?;
        if !record.is_object() {
            return Err(Error::InvalidResponse("endpoint must be an object".into()).into());
        }
        if record["type"] == "LOAD_BALANCER" {
            if !matches!(self.mode, Mode::Wait)
                || self.webhook.as_deref().is_some_and(|s| !s.is_empty())
                || self.execution_timeout.is_some()
                || self.ttl.is_some()
                || self.low_priority
            {
                return Err(CliError::Usage("load-balancer endpoints take --path, --method, and a body; job options apply to queue endpoints".into()));
            }
            return self.call_load_balancer(&client, &record, streams).await;
        }
        if self.path.as_deref().is_some_and(|s| !s.is_empty())
            || self.method.as_deref().is_some_and(|s| !s.is_empty())
        {
            return Err(CliError::Usage(
                "--path and --method apply to load-balancer endpoints".into(),
            ));
        }
        let body = self
            .body
            .ok_or_else(|| CliError::Usage("pass --input FILE or --data JSON".into()))?;
        let mut request = match body {
            Value::Object(values) if values.contains_key("input") => values,
            value => serde_json::Map::from_iter([("input".into(), value)]),
        };
        if let Some(webhook) = self.webhook.filter(|s| !s.is_empty()) {
            request.insert("webhook".into(), webhook.into());
        }
        let mut policy = request
            .get("policy")
            .filter(|value| truthy(value))
            .map(|value| {
                value
                    .as_object()
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput("job policy must be a JSON object".into()))
            })
            .transpose()?
            .unwrap_or_default();
        for (key, value) in [
            ("executionTimeout", self.execution_timeout),
            ("ttl", self.ttl),
        ] {
            if let Some(value) = value {
                let milliseconds = value.checked_mul(1000).ok_or_else(|| {
                    CliError::Usage(format!("{key} exceeds the supported duration"))
                })?;
                policy.insert(key.into(), milliseconds.into());
            }
        }
        if self.low_priority {
            policy.insert("lowPriority".into(), true.into());
        }
        if !policy.is_empty() {
            request.insert("policy".into(), policy.into());
        }
        let submitted = super::jobs::request(
            &client,
            reqwest::Method::POST,
            "run",
            &self.endpoint_id,
            None,
            Some(&Value::Object(request)),
            Duration::from_secs(10),
        )
        .await?;
        let job_id = required_str(&submitted, "id")?;
        if matches!(self.mode, Mode::Async) {
            let text = if self.json {
                output::json(&submitted)?
            } else {
                format!(
                    "✓ submitted job {job_id} {}\nrp endpoint job wait {} {job_id}\n",
                    python_string(&submitted["status"]),
                    self.endpoint_id
                )
            };
            return streams.write(Destination::Out, &text);
        }
        if matches!(self.mode, Mode::Stream) && !self.json {
            streams.write(Destination::Err, &format!("job {job_id}\n"))?;
        }
        super::job_follow::Follow {
            endpoint_id: &self.endpoint_id,
            job_id,
            stream: matches!(self.mode, Mode::Stream),
            as_json: self.json,
            timeout: self.timeout,
        }
        .run(&client, streams)
        .await
    }
    async fn call_load_balancer(
        &self,
        client: &Client,
        record: &Value,
        streams: &mut Streams<'_>,
    ) -> CliResult<()> {
        let path = self
            .path
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("/");
        let url = load_balancer_url(record, path)?;
        let method = self
            .method
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(if self.body.is_some() { "POST" } else { "GET" })
            .to_uppercase();
        let parsed_method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| Error::InvalidInput(error.to_string()))?;
        let timeout = Duration::try_from_secs_f64(self.timeout.unwrap_or(600.0)).map_err(|_| {
            Error::InvalidInput("request timeout exceeds the supported duration".into())
        })?;
        // Load-balancer requests have no Content-Type unless there is a JSON body.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(Error::from)?;
        let mut request = http
            .request(parsed_method, &url)
            .bearer_auth(&client.config.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(timeout);
        if let Some(body) = &self.body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(Error::from)?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(Error::from)?;
        if status >= 400 {
            return Err(CliError::Failed(format!(
                "{method} {url} returned {status}: {}",
                text.trim()
            )));
        }
        let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
        let text = match value.as_str().filter(|_| !self.json) {
            Some(text) => format!("{text}\n"),
            None => output::json(&value)?,
        };
        streams.write(Destination::Out, &text)
    }
}
fn load_balancer_url(record: &Value, path: &str) -> crate::Result<String> {
    let base = match record["requestUrls"]["base"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        Some(base) => base.to_owned(),
        None => format!("https://{}.api.runpod.ai", required_str(record, "id")?),
    };
    Ok(format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn load_balancer_urls_match_python() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../../../tests/fixtures/endpoint_run.json"))
                .unwrap();
        let mut checked = 0;
        for case in cases {
            let requests = case["requests"].as_array().unwrap();
            if let Some(call) = requests.iter().find(|call| call["kind"] == "lb") {
                let record = &requests
                    .iter()
                    .find(|call| call["kind"] == "control")
                    .unwrap()["response"];
                let args = case["args"].as_array().unwrap();
                let path = args
                    .iter()
                    .position(|arg| arg == "--path")
                    .map_or("/", |i| args[i + 1].as_str().unwrap());
                assert_eq!(
                    load_balancer_url(record, path).unwrap(),
                    call["url"].as_str().unwrap()
                );
                checked += 1;
            }
        }
        assert!(checked > 0);
    }
}

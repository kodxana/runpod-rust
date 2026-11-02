use super::super::{
    CliError, CliResult, output,
    streams::{Destination, Streams},
};
use super::watch::{Clock, Timer};
use crate::{
    Client, Error, Value,
    client::{required_str, truthy},
    control::python_string,
    endpoint::is_completed,
};
use std::time::Duration;

#[cfg(test)]
#[path = "job_follow_tests.rs"]
mod tests;

enum ReadFailure {
    Request(Error),
    ReadTimeout(Error),
    ConnectTimeout(Error),
    Interrupted,
}
impl From<Error> for ReadFailure {
    fn from(error: Error) -> Self {
        if let Error::Http(source) = &error
            && source.is_timeout()
        {
            return if source.is_connect() {
                Self::ConnectTimeout(error)
            } else {
                Self::ReadTimeout(error)
            };
        }
        Self::Request(error)
    }
}
impl ReadFailure {
    fn into_cli(self) -> CliError {
        match self {
            Self::Request(e) | Self::ReadTimeout(e) | Self::ConnectTimeout(e) => e.into(),
            Self::Interrupted => CliError::Aborted,
        }
    }
}
trait JobReader {
    async fn get(&mut self, operation: &str, timeout: Duration) -> Result<Value, ReadFailure>;
}
struct HttpReader<'a> {
    client: &'a Client,
    endpoint: &'a str,
    job: &'a str,
}
impl JobReader for HttpReader<'_> {
    async fn get(&mut self, operation: &str, timeout: Duration) -> Result<Value, ReadFailure> {
        super::jobs::request(
            self.client,
            reqwest::Method::GET,
            operation,
            self.endpoint,
            Some(self.job),
            None,
            timeout,
        )
        .await
        .map_err(ReadFailure::from)
    }
}
async fn read(
    reader: &mut impl JobReader,
    operation: &str,
    timeout: f64,
) -> Result<Value, ReadFailure> {
    let timeout = Duration::try_from_secs_f64(timeout).map_err(|_| {
        ReadFailure::Request(Error::InvalidInput(
            "job timeout exceeds the supported duration".into(),
        ))
    })?;
    tokio::select! {
        result = reader.get(operation,timeout) => result,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(Error::from).map_err(ReadFailure::from)?;
            Err(ReadFailure::Interrupted)
        }
    }
}

pub(super) struct Follow<'a> {
    pub endpoint_id: &'a str,
    pub job_id: &'a str,
    pub stream: bool,
    pub as_json: bool,
    pub timeout: Option<f64>,
}
impl Follow<'_> {
    pub(super) async fn run(&self, client: &Client, streams: &mut Streams<'_>) -> CliResult<()> {
        let mut reader = HttpReader {
            client,
            endpoint: self.endpoint_id,
            job: self.job_id,
        };
        self.observe(&mut reader, &mut Timer::default(), streams)
            .await
    }
    fn resume(&self) -> String {
        format!(
            "rp endpoint job {} {} {}",
            if self.stream { "stream" } else { "wait" },
            self.endpoint_id,
            self.job_id
        )
    }
    fn remaining(&self, elapsed: f64, status: &str, limit: f64) -> CliResult<f64> {
        if let Some(timeout) = self.timeout.filter(|_| !is_completed(status)) {
            let remaining = timeout - elapsed;
            if remaining <= 0.0 {
                return Err(CliError::Failed(format!(
                    "job {} is still {status} after {}s. resume with: {}",
                    self.job_id,
                    output::general(timeout, 6),
                    self.resume()
                )));
            }
            return Ok(limit.min(remaining));
        }
        Ok(limit)
    }
    fn interrupted(&self, streams: &mut Streams<'_>) -> CliResult<()> {
        streams.write(
            Destination::Err,
            &format!(
                "\njob {} keeps running. resume with: {}\n",
                self.job_id,
                self.resume()
            ),
        )?;
        // Click's Abort is a RuntimeError, so the source data-plane wrapper
        // converts this interruption into a ClickException with an empty message.
        Err(CliError::Failed(String::new()))
    }
    async fn observe(
        &self,
        reader: &mut impl JobReader,
        clock: &mut impl Clock,
        streams: &mut Streams<'_>,
    ) -> CliResult<()> {
        let started = clock.now();
        let mut status = String::from("pending");
        let (mut streamed, mut open_line) = (false, false);
        let mut job = loop {
            let operation = if self.stream { "stream" } else { "status" };
            let limit = if self.stream { 60.0 } else { 10.0 };
            let timeout = self.remaining(clock.now() - started, &status, limit)?;
            let job = match read(reader, operation, timeout).await {
                Ok(job) => job,
                Err(ReadFailure::Interrupted) => return self.interrupted(streams),
                Err(error @ (ReadFailure::ReadTimeout(_) | ReadFailure::ConnectTimeout(_))) => {
                    self.remaining(clock.now() - started, &status, 0.0)?;
                    if self.stream
                        && matches!(error, ReadFailure::ReadTimeout(_))
                        && !is_completed(&status)
                    {
                        continue;
                    }
                    return Err(error.into_cli());
                }
                Err(error) => return Err(error.into_cli()),
            };
            status = required_str(&job, "status")?.into();
            let chunks = if self.stream && truthy(&job["stream"]) {
                job["stream"]
                    .as_array()
                    .ok_or_else(|| Error::InvalidResponse("job stream must be an array".into()))?
                    .as_slice()
            } else {
                &[]
            };
            streamed |= !chunks.is_empty();
            for chunk in chunks {
                let value = chunk
                    .get("output")
                    .ok_or_else(|| Error::InvalidResponse("stream chunk has no output".into()))?;
                if let Some(text) = value.as_str().filter(|_| !self.as_json) {
                    streams.write(Destination::Out, text)?;
                    open_line = !text.ends_with('\n');
                } else {
                    if open_line {
                        streams.write(Destination::Out, "\n")?;
                        open_line = false;
                    }
                    streams.write(
                        Destination::Out,
                        &format!("{}\n", serde_json::to_string(value).map_err(Error::from)?),
                    )?;
                }
            }
            if is_completed(&status) && chunks.is_empty() {
                break job;
            }
            if chunks.is_empty() {
                let seconds = self.remaining(
                    clock.now() - started,
                    &status,
                    if self.stream { 0.5 } else { 1.0 },
                )?;
                let interrupted = tokio::select! {
                    result = clock.sleep(seconds) => result?,
                    signal = tokio::signal::ctrl_c() => { signal.map_err(Error::from)?; true }
                };
                if interrupted {
                    return self.interrupted(streams);
                }
            }
        };
        if open_line {
            streams.write(Destination::Out, "\n")?;
        }
        if streamed && status == "COMPLETED" {
            return Ok(());
        }
        if self.stream {
            let timeout = self.remaining(clock.now() - started, &status, 10.0)?;
            // This final fallback read is outside the source's resume-hint handler.
            job = match read(reader, "status", timeout).await {
                Err(ReadFailure::Interrupted) => {
                    streams.write(Destination::Err, "\n")?;
                    return Err(CliError::Aborted);
                }
                result => result.map_err(ReadFailure::into_cli)?,
            };
        }
        print_job(&job, self.as_json, streams)
    }
}
fn print_job(job: &Value, as_json: bool, streams: &mut Streams<'_>) -> CliResult<()> {
    if as_json {
        streams.write(Destination::Out, &output::json(job)?)?;
    }
    let status = required_str(job, "status")?;
    if status == "COMPLETED" {
        if !as_json {
            let text = match job["output"].as_str() {
                Some(value) => format!("{value}\n"),
                None => output::json(&job["output"])?,
            };
            streams.write(Destination::Out, &text)?;
        }
        return Ok(());
    }
    let detail = if truthy(&job["error"]) {
        format!(": {}", python_string(&job["error"]))
    } else {
        String::new()
    };
    Err(CliError::Failed(format!(
        "job {} {}{detail}",
        required_str(job, "id")?,
        status.to_lowercase()
    )))
}

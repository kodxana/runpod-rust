use super::super::{
    CliResult, output,
    streams::{Destination, Streams},
};
use super::{path, view};
use crate::{Client, Error, Value, client::truthy, control::python_string, json_value};
use std::time::Duration;
use tokio::time::Instant;

pub(super) trait Clock {
    fn now(&self) -> f64;
    /// Returns true when a test-controlled interruption occurs while waiting.
    async fn sleep(&mut self, seconds: f64) -> CliResult<bool>;
}
pub(super) struct Timer {
    start: Instant,
}
impl Default for Timer {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}
impl Clock for Timer {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
    async fn sleep(&mut self, seconds: f64) -> CliResult<bool> {
        let duration = Duration::try_from_secs_f64(seconds).map_err(|_| {
            Error::InvalidInput("refresh interval exceeds the supported duration".into())
        })?;
        tokio::time::sleep(duration).await;
        Ok(false)
    }
}
pub(super) async fn follow(
    client: &Client,
    id: &str,
    previous: &Value,
    timeout: f64,
    clock: &mut impl Clock,
    streams: &mut Streams<'_>,
    destination: Destination,
) -> CliResult<()> {
    let interrupted = tokio::select! {
        result=follow_loop(client,id,previous,timeout,clock,streams,destination)=>result?,
        signal=tokio::signal::ctrl_c()=>{ signal.map_err(Error::from)?; true },
    };
    if interrupted {
        streams.write(
            destination,
            &format!("\ndetached; the rollout continues. follow with: rp endpoint releases {id}\n"),
        )?;
    }
    Ok(())
}
async fn follow_loop(
    client: &Client,
    id: &str,
    previous: &Value,
    timeout: f64,
    clock: &mut impl Clock,
    streams: &mut Streams<'_>,
    destination: Destination,
) -> CliResult<bool> {
    let started = clock.now();
    let mut last = None;
    loop {
        let state = client
            .run_rest_request("GET", &path(id, &["releases"]), None, &[("limit", "1")])
            .await?;
        let waiting =
            !previous.is_null() && json_value::equivalent(&state["endpointVersion"], previous);
        if !waiting {
            let line = view::rollout(&state);
            if last.as_ref() != Some(&line) {
                streams.write(destination, &format!("{line}\n"))?;
                last = Some(line);
            }
            if !truthy(&state["rollout"]["inProgress"]) {
                streams.write(
                    destination,
                    &format!(
                        "✓ release v{} is live\n",
                        python_string(&state["endpointVersion"])
                    ),
                )?;
                return Ok(false);
            }
        }
        if clock.now() - started >= timeout {
            let phase = if waiting {
                "no new release observed"
            } else {
                "rollout still in progress"
            };
            streams.write(
                destination,
                &format!(
                    "! {phase} after {}s. follow with: rp endpoint releases {id}\n",
                    output::general(timeout, 6)
                ),
            )?;
            return Ok(false);
        }
        if clock.sleep(2.0).await? {
            return Ok(true);
        }
    }
}
pub(super) async fn workers(
    client: &Client,
    id: &str,
    watch: bool,
    interval: f64,
    as_json: bool,
    clock: &mut impl Clock,
    streams: &mut Streams<'_>,
) -> CliResult<()> {
    tokio::select! {
        result=workers_loop(client,id,watch,interval,as_json,clock,streams)=>result,
        signal=tokio::signal::ctrl_c()=>{ signal.map_err(Error::from)?; Ok(()) },
    }
}
async fn workers_loop(
    client: &Client,
    id: &str,
    watch: bool,
    interval: f64,
    as_json: bool,
    clock: &mut impl Clock,
    streams: &mut Streams<'_>,
) -> CliResult<()> {
    loop {
        let data = client
            .run_rest_request("GET", &path(id, &["workers"]), None, &[])
            .await?;
        if as_json {
            let text = if watch {
                format!("{}\n", serde_json::to_string(&data).map_err(Error::from)?)
            } else {
                output::json(&data)?
            };
            streams.write(Destination::Out, &text)?;
        } else {
            let text = view::worker_view(&data)?;
            if watch {
                streams.frame(&text)?;
            } else {
                streams.write(Destination::Out, &text)?;
            }
        }
        if !watch || clock.sleep(interval).await? {
            return Ok(());
        }
    }
}

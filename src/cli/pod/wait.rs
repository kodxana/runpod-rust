use super::{CliError, CliResult, path, view};
use crate::{Client, Error, Value, client::truthy};
use std::time::Duration;

pub(super) trait Clock {
    fn now(&self) -> Duration;
    async fn sleep(&mut self, duration: Duration);
}
pub(super) struct Timer(tokio::time::Instant);
impl Default for Timer {
    fn default() -> Self {
        Self(tokio::time::Instant::now())
    }
}
impl Clock for Timer {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    async fn sleep(&mut self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}
pub(super) struct Waiter<C> {
    pub clock: C,
    pub timeout: Duration,
}
impl Default for Waiter<Timer> {
    fn default() -> Self {
        Self {
            clock: Timer::default(),
            timeout: Duration::from_secs(600),
        }
    }
}
impl<C: Clock> Waiter<C> {
    pub fn now(&self) -> Duration {
        self.clock.now()
    }
    pub async fn run(
        &mut self,
        client: &Client,
        id: &str,
        since: Duration,
        previous: Option<&Value>,
    ) -> CliResult<Value> {
        tokio::select! {
            result=self.poll(client,id,since,previous)=>result,
            signal=tokio::signal::ctrl_c()=>{signal.map_err(Error::from)?;Err(CliError::Aborted)}
        }
    }
    async fn poll(
        &mut self,
        client: &Client,
        id: &str,
        since: Duration,
        previous: Option<&Value>,
    ) -> CliResult<Value> {
        let started = self.clock.now();
        let previous_uptime = previous
            .and_then(|p| p["runtime"]["uptime"].as_f64())
            .unwrap_or(0.0);
        let mut restarted = previous.is_none();
        loop {
            let pod = client.run_rest_request("GET", &path(id), None, &[]).await?;
            let status = view::status(&pod)?;
            let uptime = if status == "running" {
                pod["runtime"]["uptime"].as_f64().ok_or_else(|| {
                    Error::InvalidResponse("runtime uptime must be numeric".into())
                })?
            } else {
                0.0
            };
            if status != "running" || uptime < previous_uptime {
                restarted = true;
            }
            let new_start = previous.is_none_or(|previous| {
                truthy(&pod["startedAt"]) && pod["startedAt"] != previous["startedAt"]
            });
            if status == "running"
                && new_start
                && restarted
                && uptime <= self.clock.now().saturating_sub(since).as_secs_f64() + 1.0
            {
                return Ok(pod);
            }
            if matches!(status.as_str(), "error" | "terminated") {
                return Err(CliError::Failed(format!("pod {id} is {status}")));
            }
            if self.clock.now().saturating_sub(started) >= self.timeout {
                let status = if status.is_empty() {
                    "unknown"
                } else {
                    &status
                };
                return Err(CliError::Failed(format!(
                    "pod {id} is still {status} after {}s",
                    self.timeout.as_secs()
                )));
            }
            self.clock.sleep(Duration::from_secs(5)).await;
        }
    }
}

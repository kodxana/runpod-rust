use super::{JobsProgress, PingJobMirror, WorkerConfig};
use crate::{Error, Result};
use reqwest::{Client, header};
use std::{future::Future, io, time::Duration};
use tokio::sync::oneshot;

/// Connection pool and GET retry settings for an explicitly driven heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatOptions {
    /// Maximum idle connections retained per host (Python also caps cached host pools).
    pub pool_connections: usize,
    /// Additional attempts after the initial request; the default is three.
    pub retries: usize,
}

impl Default for HeartbeatOptions {
    fn default() -> Self {
        Self {
            pool_connections: 10,
            retries: 3,
        }
    }
}

/// Periodic worker pings with bounded snapshots and an optional independent executor.
///
/// Construction starts no work. [`Self::start_ping`] explicitly owns a native thread
/// and runtime, so blocking user handlers cannot stop heartbeats. Each instance starts
/// at most once; separate instances replace Python's process-global singleton.
#[derive(Debug)]
pub struct Heartbeat {
    config: WorkerConfig,
    options: HeartbeatOptions,
    started: bool,
}

impl Heartbeat {
    /// Configure a heartbeat using the source's default pool and retry counts.
    ///
    /// # Errors
    /// Rejects zero intervals and malformed authorization headers.
    pub fn new(config: WorkerConfig) -> Result<Self> {
        Self::with_options(config, HeartbeatOptions::default())
    }

    /// Configure explicit-loop requests; background startup reconstructs default options.
    ///
    /// Python's child creates a fresh default Heartbeat, discarding parent pool/retry
    /// overrides. This preserves that quirk for [`Self::start_ping`].
    ///
    /// # Errors
    /// Rejects zero intervals and malformed authorization headers.
    pub fn with_options(config: WorkerConfig, options: HeartbeatOptions) -> Result<Self> {
        if config.ping_interval.is_zero() {
            return Err(Error::InvalidInput(
                "heartbeat interval must be positive".into(),
            ));
        }
        header::HeaderValue::from_str(&config.api_key)
            .map_err(|_| Error::InvalidInput("invalid worker authorization header".into()))?;
        Ok(Self {
            config,
            options,
            started: false,
        })
    }

    /// Read the owned configuration; its Debug representation redacts credentials.
    #[must_use]
    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }

    /// Read the explicit-loop connection and retry settings.
    #[must_use]
    pub fn options(&self) -> HeartbeatOptions {
        self.options
    }

    fn enabled(&self) -> bool {
        !self.config.api_key.is_empty()
            && !self.config.worker_id.is_empty()
            && self
                .config
                .ping_url
                .as_deref()
                .is_some_and(|url| !url.is_empty() && url != "PING_NOT_SET")
    }

    fn client(&self) -> Result<Client> {
        let mut token = header::HeaderValue::from_str(&self.config.api_key)
            .map_err(|_| Error::InvalidInput("invalid worker authorization header".into()))?;
        token.set_sensitive(true);
        let mut headers = header::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, token);
        let timeout = self.config.ping_interval.saturating_mul(2);
        Ok(Client::builder()
            .default_headers(headers)
            .pool_max_idle_per_host(self.options.pool_connections)
            .connect_timeout(timeout)
            .read_timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(30))
            .retry(reqwest::retry::never())
            .build()?)
    }

    /// Send one logical ping, including retry attempts with a single job snapshot.
    /// Missing deployment configuration is a no-op; ordinary nonretryable HTTP errors
    /// return their status, matching Python's absence of `raise_for_status`.
    ///
    /// # Errors
    /// Returns request, response-body, retry-header or exhausted-retry errors.
    pub async fn send_ping(&self, mirror: Option<&PingJobMirror>) -> Result<Option<u16>> {
        if !self.enabled() {
            return Ok(None);
        }
        self.send(&self.client()?, mirror.and_then(PingJobMirror::get))
            .await
            .map(Some)
    }

    async fn send(&self, client: &Client, ids: Option<String>) -> Result<u16> {
        let url = self
            .config
            .ping_url
            .as_deref()
            .unwrap_or_default()
            .replace("$RUNPOD_POD_ID", &self.config.worker_id);
        for attempt in 0..=self.options.retries {
            let mut request = client
                .get(&url)
                .query(&[("runpod_version", env!("CARGO_PKG_VERSION"))]);
            if let Some(ids) = &ids {
                request = request.query(&[("job_id", ids)]);
            }
            let delay = match request.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let forced = matches!(status, 429 | 500 | 502 | 503 | 504);
                    let after = response.headers().get(header::RETRY_AFTER);
                    let retry = forced
                        || (attempt < self.options.retries && status == 413 && after.is_some());
                    if !retry {
                        response
                            .bytes()
                            .await
                            .map_err(reqwest::Error::without_url)?;
                        return Ok(status);
                    }
                    if attempt == self.options.retries {
                        response
                            .error_for_status()
                            .map_err(reqwest::Error::without_url)?;
                        return Ok(status);
                    }
                    let delay = after
                        .map(retry_after)
                        .transpose()?
                        .filter(|delay| !delay.is_zero());
                    // requests consumes the response before retrying, releasing the connection.
                    response
                        .bytes()
                        .await
                        .map_err(reqwest::Error::without_url)?;
                    delay.unwrap_or_else(|| backoff(attempt))
                }
                Err(error) => {
                    if attempt == self.options.retries
                        || error.is_builder()
                        || error.is_redirect()
                        || !(error.is_connect() || error.is_timeout() || error.is_request())
                    {
                        return Err(error.without_url().into());
                    }
                    backoff(attempt)
                }
            };
            tokio::time::sleep(delay).await;
        }
        unreachable!("the final retry returns its response or error")
    }

    async fn tick(&self, client: &Client, mirror: Option<&PingJobMirror>) {
        if let Err(error) = self.send(client, mirror.and_then(PingJobMirror::get)).await {
            tracing::warn!(%error, "worker heartbeat failed; continuing ping loop");
        }
        tokio::time::sleep(self.config.ping_interval).await;
    }

    /// Run one ping and the following interval, logging request failures without stopping.
    /// This is the source's single-iteration test mode without a boolean switch.
    ///
    /// # Errors
    /// Returns HTTP client initialization failures.
    pub async fn run_once(&self, mirror: Option<&PingJobMirror>) -> Result<()> {
        if self.enabled() {
            self.tick(&self.client()?, mirror).await;
        }
        Ok(())
    }

    /// Drive periodic pings on the caller's executor until shutdown resolves.
    /// Cancellation interrupts requests, retries and the interval sleep.
    ///
    /// # Errors
    /// Returns HTTP client initialization failures. Request failures are logged and retried
    /// on the next interval after the configured retry budget is exhausted.
    pub async fn ping_loop(
        &self,
        mirror: Option<&PingJobMirror>,
        shutdown: impl Future<Output = ()>,
    ) -> Result<()> {
        if !self.enabled() {
            return Ok(());
        }
        self.drive(&self.client()?, mirror, shutdown).await;
        Ok(())
    }

    async fn drive(
        &self,
        client: &Client,
        mirror: Option<&PingJobMirror>,
        shutdown: impl Future<Output = ()>,
    ) {
        tokio::pin!(shutdown);
        loop {
            tokio::select! { biased;
                () = &mut shutdown => return,
                () = self.tick(client, mirror) => {},
            }
        }
    }

    /// Start at most one independent heartbeat executor, returning an owned stop guard.
    /// No thread is started without a key, worker ID and heartbeat URL. Dropping the
    /// guard signals cancellation without blocking; [`HeartbeatHandle::stop`] also waits
    /// for HTTP resources and the private runtime to be released.
    ///
    /// # Errors
    /// Returns thread, runtime or HTTP client initialization failures.
    pub async fn start_ping(
        &mut self,
        mirror: Option<PingJobMirror>,
    ) -> Result<Option<HeartbeatHandle>> {
        if self.started || !self.enabled() {
            return Ok(None);
        }
        let heartbeat = Self::new(self.config.clone())?;
        let (stop, stopped) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        let (ready, initialized) = oneshot::channel();
        std::thread::Builder::new()
            .name("runpod-heartbeat".into())
            .spawn(move || {
                let setup = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(Error::from)
                    .and_then(|runtime| heartbeat.client().map(|client| (runtime, client)));
                match setup {
                    Ok((runtime, client)) => {
                        if ready.send(Ok(())).is_ok() {
                            runtime.block_on(heartbeat.drive(&client, mirror.as_ref(), async {
                                let _ = stopped.await;
                            }));
                        }
                        drop(client);
                        drop(runtime);
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
                let _ = done.send(());
            })?;
        let handle = HeartbeatHandle {
            stop: Some(stop),
            finished,
        };
        initialized.await.map_err(|_| executor_closed())??;
        self.started = true;
        Ok(Some(handle))
    }
}

/// Owns cancellation of the heartbeat's independent thread and runtime.
#[derive(Debug)]
#[must_use = "dropping the handle stops its heartbeat"]
pub struct HeartbeatHandle {
    stop: Option<oneshot::Sender<()>>,
    finished: oneshot::Receiver<()>,
}

impl HeartbeatHandle {
    /// Cancel pending HTTP/timer work and wait until the executor releases its resources.
    ///
    /// # Errors
    /// Returns a broken-pipe error if the heartbeat thread terminates unexpectedly.
    pub async fn stop(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        (&mut self.finished).await.map_err(|_| executor_closed())
    }
}

impl Drop for HeartbeatHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn executor_closed() -> Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "heartbeat executor exited unexpectedly",
    )
    .into()
}

fn backoff(attempt: usize) -> Duration {
    Duration::from_secs(if attempt == 0 {
        0
    } else {
        (1_u64 << attempt.min(7)).min(120)
    })
}

fn retry_after(value: &header::HeaderValue) -> Result<Duration> {
    let text = value
        .to_str()
        .map_err(|_| Error::InvalidResponse("invalid Retry-After header".into()))?
        .trim();
    let delay = if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
        // Overflowing positive integers exceed the source's six-hour cap as well.
        Duration::from_secs(text.parse::<u64>().unwrap_or(u64::MAX))
    } else {
        let date = httpdate::parse_http_date(text)
            .map_err(|_| Error::InvalidResponse("invalid Retry-After header".into()))?;
        date.duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
    };
    // urllib3 2.8 caps Retry-After at six hours independently of exponential backoff.
    Ok(delay.min(Duration::from_secs(21_600)))
}

pub(super) async fn start_worker(
    config: &WorkerConfig,
    progress: &JobsProgress,
) -> Result<Option<HeartbeatHandle>> {
    let mirror = PingJobMirror::new();
    progress.set_mirror(Some(mirror.clone()));
    Heartbeat::new(config.clone())?
        .start_ping(Some(mirror))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_http_dates_and_clamps_past_and_large_values() {
        for value in [
            "0",
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(
                retry_after(&header::HeaderValue::from_str(value).unwrap()).unwrap(),
                Duration::ZERO
            );
        }
        for value in ["99999999999999999999999999", "21601"] {
            assert_eq!(
                retry_after(&header::HeaderValue::from_str(value).unwrap()).unwrap(),
                Duration::from_secs(21600)
            );
        }
        for value in ["-1", "+1", "1.5", "later", ""] {
            assert!(
                retry_after(&header::HeaderValue::from_str(value).unwrap()).is_err(),
                "{value}"
            );
        }
        let future = std::time::SystemTime::now() + Duration::from_secs(10);
        let header = header::HeaderValue::from_str(&httpdate::fmt_http_date(future)).unwrap();
        let delay = retry_after(&header).unwrap();
        assert!(delay > Duration::from_secs(8) && delay <= Duration::from_secs(10));
    }

    #[test]
    fn retry_backoff_starts_immediately_and_caps_at_two_minutes() {
        let actual: Vec<_> = (0..10).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(actual, [0, 2, 4, 8, 16, 32, 64, 120, 120, 120]);
        assert_eq!(backoff(usize::MAX), Duration::from_secs(120));
    }
}

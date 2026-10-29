//! Serverless endpoint submissions, polling, cancellation, and incremental output.
use crate::{
    Client, Error, Result, Value,
    client::{required_str, segment, truthy},
    json,
};
use futures_util::Stream;
use std::time::Duration;

/// Terminal states used by the Python SDK's stream protocol.
pub const FINAL_STATES: &[&str] = &["COMPLETED", "FAILED", "TIMED_OUT"];

/// Tests whether a status is terminal, including a cancelled job.
#[must_use]
pub fn is_completed(status: &str) -> bool {
    FINAL_STATES.contains(&status) || status == "CANCELLED"
}

/// A serverless endpoint bound to an authenticated client.
#[derive(Debug, Clone)]
pub struct Endpoint {
    endpoint_id: String,
    client: Client,
}

impl Endpoint {
    /// Creates an endpoint using an existing client.
    pub fn new(endpoint_id: impl Into<String>, client: Client) -> Self {
        Self {
            endpoint_id: endpoint_id.into(),
            client,
        }
    }
    /// Returns the configured endpoint identifier.
    pub fn id(&self) -> &str {
        &self.endpoint_id
    }
    fn path(&self, operation: &str) -> String {
        format!("{}/{operation}", segment(&self.endpoint_id))
    }
    async fn request(
        &self,
        method: &str,
        operation: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        self.client
            .request(
                &self.client.config.endpoint_base_url,
                method,
                &self.path(operation),
                body,
                &[],
                timeout,
            )
            .await
    }
    /// Submits input, preserving an existing nonempty `input` envelope as Python's Endpoint does.
    ///
    /// # Errors
    /// Returns request errors or a malformed job response error.
    pub async fn run(&self, input: Value) -> Result<Job> {
        let response = self
            .request(
                "POST",
                "run",
                Some(&wrap_input(input)),
                Duration::from_secs(10),
            )
            .await?;
        Ok(Job::new(
            &self.endpoint_id,
            required_str(&response, "id")?,
            self.client.clone(),
        ))
    }
    /// Submits input using Python AsyncioEndpoint's unconditional `input` envelope.
    ///
    /// # Errors
    /// Returns request errors or a malformed job response error.
    pub async fn run_asyncio(&self, input: Value) -> Result<Job> {
        let response = self
            .request(
                "POST",
                "run",
                Some(&json!({"input":input})),
                Duration::from_secs(10),
            )
            .await?;
        let id = required_str(&response, "id")?;
        let mut client = self.client.clone();
        client.request_id = Some(id.to_owned());
        Ok(Job::new(&self.endpoint_id, id, client))
    }
    /// Calls runsync and polls when the service returns an unfinished job.
    ///
    /// # Errors
    /// Returns request errors, invalid response errors, or a job timeout.
    pub async fn run_sync(&self, input: Value, timeout: Duration) -> Result<Value> {
        let response = self
            .request("POST", "runsync", Some(&wrap_input(input)), timeout)
            .await?;
        if FINAL_STATES.contains(&required_str(&response, "status")?) {
            return Ok(response.get("output").cloned().unwrap_or(Value::Null));
        }
        Job::new(
            &self.endpoint_id,
            required_str(&response, "id")?,
            self.client.clone(),
        )
        .output(timeout)
        .await
    }
    /// Returns the endpoint's worker and queue health.
    ///
    /// # Errors
    /// Returns request or response decoding errors.
    pub async fn health(&self) -> Result<Value> {
        self.request("GET", "health", None, Duration::from_secs(3))
            .await
    }
    /// Purges pending jobs from the endpoint's queue.
    ///
    /// # Errors
    /// Returns request or response decoding errors.
    pub async fn purge_queue(&self) -> Result<Value> {
        self.request("POST", "purge-queue", None, Duration::from_secs(3))
            .await
    }
}

/// Compatibility facade for Python AsyncioEndpoint's distinct input and timeout behavior.
#[derive(Debug, Clone)]
pub struct AsyncioEndpoint {
    inner: Endpoint,
}

impl AsyncioEndpoint {
    /// Binds an endpoint to an existing asynchronous client.
    pub fn new(endpoint_id: impl Into<String>, client: Client) -> Self {
        Self {
            inner: Endpoint::new(endpoint_id, client),
        }
    }
    /// Submits input with an unconditional envelope and attaches X-Request-ID to job calls.
    ///
    /// # Errors
    /// Returns submission or malformed response errors.
    pub async fn run(&self, input: Value) -> Result<AsyncioJob> {
        Ok(AsyncioJob {
            inner: self.inner.run_asyncio(input).await?,
        })
    }
    /// Reads endpoint health.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn health(&self) -> Result<Value> {
        self.inner.health().await
    }
    /// Removes queued jobs.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn purge_queue(&self) -> Result<Value> {
        self.inner.purge_queue().await
    }
}

/// Compatibility job with Python AsyncioJob's zero-timeout and refetch behavior.
#[derive(Debug, Clone)]
pub struct AsyncioJob {
    inner: Job,
}

impl AsyncioJob {
    /// Attaches to an existing job.
    pub fn new(endpoint_id: impl Into<String>, job_id: impl Into<String>, client: Client) -> Self {
        Self {
            inner: Job::new(endpoint_id, job_id, client),
        }
    }
    /// Returns the job identifier.
    pub fn id(&self) -> &str {
        self.inner.id()
    }
    /// Reads and caches terminal status.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn status(&mut self) -> Result<String> {
        self.inner.status().await
    }
    /// Waits for output; a zero timeout fails immediately unless non-null output is cached.
    ///
    /// # Errors
    /// Returns timeout or request errors.
    pub async fn output(&mut self, timeout: Duration) -> Result<Value> {
        if !self.inner.job_output.is_null() {
            return Ok(self.inner.job_output.clone());
        }
        if timeout.is_zero() {
            return Err(Error::Timeout);
        }
        tokio::time::timeout(timeout, async {
            while !is_completed(&self.inner.status().await?) {
                tokio::time::sleep(self.inner.client.config.poll_interval).await;
            }
            Ok::<(), Error>(())
        })
        .await
        .map_err(|_| Error::Timeout)??;
        Ok(self
            .inner
            .fetch("status")
            .await?
            .get("output")
            .cloned()
            .unwrap_or(Value::Null))
    }
    /// Streams partial results until an empty terminal page.
    pub fn stream(&mut self) -> impl Stream<Item = Result<Value>> + '_ {
        self.inner.stream()
    }
    /// Cancels the job.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn cancel(&self) -> Result<Value> {
        self.inner.cancel().await
    }
}

fn wrap_input(input: Value) -> Value {
    if input.get("input").is_some_and(truthy) {
        input
    } else {
        json!({"input":input})
    }
}

/// A submitted job with cached terminal status and output.
#[derive(Debug, Clone)]
pub struct Job {
    endpoint_id: String,
    job_id: String,
    client: Client,
    job_status: Option<String>,
    job_output: Value,
}

impl Job {
    /// Attaches to an existing job without sending a request.
    pub fn new(endpoint_id: impl Into<String>, job_id: impl Into<String>, client: Client) -> Self {
        Self {
            endpoint_id: endpoint_id.into(),
            job_id: job_id.into(),
            client,
            job_status: None,
            job_output: Value::Null,
        }
    }
    /// Returns the job identifier.
    pub fn id(&self) -> &str {
        &self.job_id
    }
    /// Returns the endpoint identifier.
    pub fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }
    async fn fetch(&mut self, source: &str) -> Result<Value> {
        let path = format!(
            "{}/{source}/{}",
            segment(&self.endpoint_id),
            segment(&self.job_id)
        );
        let response = self
            .client
            .request(
                &self.client.config.endpoint_base_url,
                "GET",
                &path,
                None,
                &[],
                Duration::from_secs(10),
            )
            .await?;
        let status = required_str(&response, "status")?;
        if is_completed(status) {
            self.job_status = Some(status.into());
            self.job_output = response.get("output").cloned().unwrap_or(Value::Null);
        }
        Ok(response)
    }
    /// Fetches status until terminal, then returns the cached value.
    ///
    /// # Errors
    /// Returns request or invalid response errors.
    pub async fn status(&mut self) -> Result<String> {
        if let Some(status) = &self.job_status {
            return Ok(status.clone());
        }
        Ok(required_str(&self.fetch("status").await?, "status")?.into())
    }
    /// Returns output immediately for a zero timeout, or waits up to the given deadline.
    ///
    /// # Errors
    /// Returns request errors, invalid response errors, or timeout.
    pub async fn output(&mut self, timeout: Duration) -> Result<Value> {
        if !timeout.is_zero() {
            tokio::time::timeout(timeout, async {
                while !is_completed(&self.status().await?) {
                    tokio::time::sleep(self.client.config.poll_interval).await;
                }
                Ok::<(), Error>(())
            })
            .await
            .map_err(|_| Error::Timeout)??;
        }
        if !self.job_output.is_null() {
            return Ok(self.job_output.clone());
        }
        Ok(self
            .fetch("status")
            .await?
            .get("output")
            .cloned()
            .unwrap_or(Value::Null))
    }
    /// Yields incremental output until a terminal stream page contains no chunks.
    ///
    /// CANCELLED remains outside `FINAL_STATES`, matching Python's stream behavior.
    /// Drop the stream to cancel polling.
    ///
    /// # Errors
    /// Each item may contain a request or invalid response error.
    pub fn stream(&mut self) -> impl Stream<Item = Result<Value>> + '_ {
        async_stream::try_stream! {
            loop {
                tokio::time::sleep(self.client.config.poll_interval).await;
                let page = self.fetch("stream").await?;
                let terminal = FINAL_STATES.contains(&required_str(&page, "status")?);
                let chunks = page.get("stream").and_then(Value::as_array);
                if terminal && chunks.is_none_or(Vec::is_empty) { break; }
                if let Some(chunks) = chunks {
                    for chunk in chunks {
                        yield chunk.get("output").cloned().ok_or_else(|| Error::InvalidResponse("stream chunk has no output".into()))?;
                    }
                }
            }
        }
    }
    /// Requests cancellation of this job.
    ///
    /// # Errors
    /// Returns request or response decoding errors.
    pub async fn cancel(&self) -> Result<Value> {
        let path = format!(
            "{}/cancel/{}",
            segment(&self.endpoint_id),
            segment(&self.job_id)
        );
        self.client
            .request(
                &self.client.config.endpoint_base_url,
                "POST",
                &path,
                None,
                &[],
                Duration::from_secs(3),
            )
            .await
    }
}

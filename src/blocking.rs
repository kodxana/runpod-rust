//! Synchronous facade for callers without an asynchronous runtime.
//!
//! Construct and use these clients outside a Tokio runtime. Async callers should use
//! the crate-root client instead; creating a nested runtime is rejected.
use crate::{ClientConfig, Error, Result, Value};
use std::{sync::Arc, time::Duration};

/// An owned Tokio runtime and shared asynchronous client for blocking applications.
#[derive(Debug, Clone)]
pub struct Client {
    inner: crate::Client,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl Client {
    /// Constructs a blocking client with explicit credentials.
    ///
    /// # Errors
    /// Fails on invalid credentials or when called inside a Tokio runtime.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::with_config(ClientConfig::new(api_key))
    }
    /// Constructs a blocking client with custom service URLs.
    ///
    /// # Errors
    /// Fails on invalid configuration, runtime creation, or nested-runtime use.
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(Error::InvalidInput(
                "blocking client must be created outside a Tokio runtime".into(),
            ));
        }
        Ok(Self {
            inner: crate::Client::with_config(config)?,
            runtime: Arc::new(tokio::runtime::Runtime::new()?),
        })
    }
    /// Sends a synchronous authenticated REST request.
    ///
    /// # Errors
    /// Returns transport, authentication, API, or decoding errors.
    pub fn run_rest_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        params: &[(&str, &str)],
    ) -> Result<Value> {
        self.runtime
            .block_on(self.inner.run_rest_request(method, path, body, params))
    }
    /// Sends a synchronous authenticated GraphQL request.
    ///
    /// # Errors
    /// Returns transport, authentication, API, or decoding errors.
    pub fn run_graphql_query(&self, query: &str, variables: Option<&Value>) -> Result<Value> {
        self.runtime
            .block_on(self.inner.run_graphql_query(query, variables))
    }
    /// Creates a blocking endpoint sharing the connection pool.
    pub fn endpoint(&self, id: impl Into<String>) -> Endpoint {
        Endpoint {
            inner: self.inner.endpoint(id),
            runtime: self.runtime.clone(),
        }
    }
}

/// Synchronous serverless endpoint client.
#[derive(Debug, Clone)]
pub struct Endpoint {
    inner: crate::Endpoint,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl Endpoint {
    /// Submits a job and returns a blocking job handle.
    ///
    /// # Errors
    /// Returns submission or malformed response errors.
    pub fn run(&self, input: Value) -> Result<Job> {
        Ok(Job {
            inner: self.runtime.block_on(self.inner.run(input))?,
            runtime: self.runtime.clone(),
        })
    }
    /// Waits for a job's output through runsync and status polling.
    ///
    /// # Errors
    /// Returns request errors or timeout.
    pub fn run_sync(&self, input: Value, timeout: Duration) -> Result<Value> {
        self.runtime.block_on(self.inner.run_sync(input, timeout))
    }
    /// Gets endpoint health.
    ///
    /// # Errors
    /// Returns request errors.
    pub fn health(&self) -> Result<Value> {
        self.runtime.block_on(self.inner.health())
    }
    /// Purges queued jobs.
    ///
    /// # Errors
    /// Returns request errors.
    pub fn purge_queue(&self) -> Result<Value> {
        self.runtime.block_on(self.inner.purge_queue())
    }
}

/// Synchronous job polling and cancellation handle.
#[derive(Debug, Clone)]
pub struct Job {
    inner: crate::Job,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl Job {
    /// Returns the job ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }
    /// Gets the job status, caching terminal responses.
    ///
    /// # Errors
    /// Returns request errors.
    pub fn status(&mut self) -> Result<String> {
        self.runtime.block_on(self.inner.status())
    }
    /// Reads output, optionally polling until the specified deadline.
    ///
    /// # Errors
    /// Returns request errors or timeout.
    pub fn output(&mut self, timeout: Duration) -> Result<Value> {
        self.runtime.block_on(self.inner.output(timeout))
    }
    /// Cancels a pending or running job.
    ///
    /// # Errors
    /// Returns request errors.
    pub fn cancel(&self) -> Result<Value> {
        self.runtime.block_on(self.inner.cancel())
    }
    /// Returns an iterator that polls incremental output on demand.
    pub fn stream(&mut self) -> impl Iterator<Item = Result<Value>> + '_ {
        use futures_util::StreamExt;
        let mut stream = Box::pin(self.inner.stream());
        let runtime = &self.runtime;
        std::iter::from_fn(move || runtime.block_on(stream.next()))
    }
}

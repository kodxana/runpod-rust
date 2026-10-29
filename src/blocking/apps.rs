//! Synchronous application clients and job handles.
use crate::{Result, Value, apps::targets::JobOptions};
use futures_util::{StreamExt, stream::BoxStream};
use serde_json::Map;
use std::{fmt, sync::Arc, time::Duration};

/// Synchronous client for a deployed application queue.
#[derive(Debug, Clone)]
pub struct Queue {
    pub(super) inner: crate::apps::Queue,
    pub(super) runtime: Arc<tokio::runtime::Runtime>,
}
impl Queue {
    /// Returns a clone with per-job policy and callback options accumulated.
    ///
    /// # Errors
    /// Returns invalid option errors.
    pub fn with_options(&self, options: &JobOptions) -> Result<Self> {
        Ok(Self {
            inner: self.inner.with_options(options)?,
            runtime: self.runtime.clone(),
        })
    }
    /// Invokes a queue function and returns its result.
    ///
    /// # Errors
    /// Returns request, deadline, or remote execution errors.
    pub fn remote(&self, input: &Map<String, Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.remote(input))
    }
    /// Invokes with an explicit wait deadline.
    ///
    /// # Errors
    /// Returns request, deadline, or remote execution errors.
    pub fn remote_with_timeout(
        &self,
        input: &Map<String, Value>,
        timeout: Duration,
    ) -> Result<Value> {
        self.runtime
            .block_on(self.inner.remote_with_timeout(input, timeout))
    }
    /// Submits without waiting for execution.
    ///
    /// # Errors
    /// Returns submission or invalid admission-response errors.
    pub fn spawn(&self, input: &Map<String, Value>) -> Result<Job> {
        Ok(Job {
            inner: self.runtime.block_on(self.inner.spawn(input))?,
            runtime: self.runtime.clone(),
        })
    }
    /// Reconnects to an existing job without an immediate network request.
    pub fn job(&self, id: impl Into<String>) -> Job {
        Job {
            inner: self.inner.job(id),
            runtime: self.runtime.clone(),
        }
    }
    /// Lazily submits and yields partial results on the owned runtime.
    pub fn stream(&self, input: Map<String, Value>) -> OutputIter {
        OutputIter {
            inner: self.inner.stream(input),
            runtime: self.runtime.clone(),
        }
    }
    /// Lazily submits and streams with an explicit streaming deadline.
    pub fn stream_with_timeout(&self, input: Map<String, Value>, timeout: Duration) -> OutputIter {
        OutputIter {
            inner: self.inner.stream_with_timeout(input, timeout),
            runtime: self.runtime.clone(),
        }
    }
}

/// A queue job whose status and results are queried synchronously.
#[derive(Debug, Clone)]
pub struct Job {
    inner: crate::apps::Job,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl Job {
    /// Returns the submitted job ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }
    /// Returns whether the cached state is terminal.
    pub fn done(&self) -> bool {
        self.inner.done()
    }
    /// Reads status, reusing cached terminal data.
    ///
    /// # Errors
    /// Returns request or decoding errors.
    pub fn status(&mut self) -> Result<&str> {
        self.runtime.block_on(self.inner.status())
    }
    /// Waits for output while updating cached job state.
    ///
    /// # Errors
    /// Returns request, deadline, or remote execution errors.
    pub fn result(&mut self, timeout: Duration) -> Result<Value> {
        self.runtime.block_on(self.inner.result(timeout))
    }
    /// Alias of result.
    ///
    /// # Errors
    /// Returns the same errors as result.
    pub fn output(&mut self, timeout: Duration) -> Result<Value> {
        self.result(timeout)
    }
    /// Cancels this job and updates cached status.
    ///
    /// # Errors
    /// Returns request or decoding errors.
    pub fn cancel(&mut self) -> Result<&mut Self> {
        self.runtime.block_on(self.inner.cancel())?;
        Ok(self)
    }
    /// Retries this job and updates cached status.
    ///
    /// # Errors
    /// Returns request or decoding errors.
    pub fn retry(&mut self) -> Result<&mut Self> {
        self.runtime.block_on(self.inner.retry())?;
        Ok(self)
    }
    /// Iterates over partial results until the terminal page.
    pub fn stream(&self, timeout: Duration) -> OutputIter {
        OutputIter {
            inner: self.inner.stream(timeout),
            runtime: self.runtime.clone(),
        }
    }
}

/// Blocking adapter for an application output stream, owning its runtime.
pub struct OutputIter {
    inner: BoxStream<'static, Result<Value>>,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl fmt::Debug for OutputIter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputIter").finish_non_exhaustive()
    }
}
impl Iterator for OutputIter {
    type Item = Result<Value>;
    fn next(&mut self) -> Option<Self::Item> {
        self.runtime.block_on(self.inner.next())
    }
}

/// Synchronous HTTP client for a deployed application API.
#[derive(Debug, Clone)]
pub struct Api {
    pub(super) inner: crate::apps::Api,
    pub(super) runtime: Arc<tokio::runtime::Runtime>,
}
impl Api {
    /// Overrides the HTTP service root for private gateways and local workers.
    ///
    /// # Errors
    /// Returns invalid URL errors.
    pub fn set_load_balancer_url(&mut self, url: impl Into<String>) -> Result<()> {
        self.inner.set_load_balancer_url(url)
    }
    /// Sends an HTTP route call with an explicit method and timeout.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        self.runtime
            .block_on(self.inner.request(method, path, body, timeout))
    }
    /// Sends GET and returns JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub fn get(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.get(path, body))
    }
    /// Sends POST and returns JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub fn post(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.post(path, body))
    }
    /// Sends PUT and returns JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub fn put(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.put(path, body))
    }
    /// Sends DELETE and returns JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub fn delete(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.delete(path, body))
    }
    /// Sends PATCH and returns JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub fn patch(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.patch(path, body))
    }
}

use super::{queue::is_terminal, targets::Target};
use crate::{Error, Result, Value};
use futures_util::stream::BoxStream;
use std::{fmt, time::Duration};

/// A submitted Apps queue job with cached terminal status and reconnectable identity.
#[derive(Clone)]
pub struct Job {
    data: Value,
    target: Target,
}
impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job")
            .field("id", &self.id())
            .field("status", &self.cached_status())
            .finish()
    }
}
impl Job {
    /// Creates a handle from raw admission data and its invocation target.
    ///
    /// # Errors
    /// Rejects non-object admission data.
    pub fn new(data: Value, target: Target) -> Result<Self> {
        if !data.is_object() {
            return Err(Error::InvalidResponse("job data must be an object".into()));
        }
        Ok(Self { data, target })
    }
    /// Returns the submitted job ID, or an empty string when absent upstream.
    pub fn id(&self) -> &str {
        self.data.get("id").and_then(Value::as_str).unwrap_or("")
    }
    /// Returns whether cached state is terminal, including cancellation and timeout.
    pub fn done(&self) -> bool {
        is_terminal(&self.data)
    }
    fn cached_status(&self) -> &str {
        self.data
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
    }
    fn update(&mut self, value: Value) -> Result<()> {
        let object = value
            .as_object()
            .ok_or_else(|| Error::InvalidResponse("job status must be an object".into()))?;
        self.data
            .as_object_mut()
            .expect("job data remains an object")
            .extend(object.clone());
        Ok(())
    }
    /// Fetches status unless terminal state is already cached.
    ///
    /// # Errors
    /// Returns request or malformed response errors.
    pub async fn status(&mut self) -> Result<&str> {
        if !self.done() {
            self.update(self.target.job_status(self.id()).await?)?;
        }
        Ok(self.cached_status())
    }
    /// Waits for output while updating cached state on every observed response.
    ///
    /// # Errors
    /// Returns request, deadline, malformed response, or remote execution errors.
    pub async fn result(&mut self, timeout: Duration) -> Result<Value> {
        let target = self.target.clone();
        let initial = self.data.clone();
        let mut update = |value: &Value| self.update(value.clone());
        target.wait(initial, timeout, Some(&mut update)).await
    }
    /// Alias of result with the same caching and timeout behavior.
    ///
    /// # Errors
    /// Returns the same errors as result.
    pub async fn output(&mut self, timeout: Duration) -> Result<Value> {
        self.result(timeout).await
    }
    /// Cancels the job, updates its state, and returns this handle.
    ///
    /// # Errors
    /// Returns request or malformed response errors.
    pub async fn cancel(&mut self) -> Result<&mut Self> {
        self.update(self.target.cancel_job(self.id()).await?)?;
        Ok(self)
    }
    /// Retries the job, updates its state, and returns this handle.
    ///
    /// # Errors
    /// Returns request or malformed response errors.
    pub async fn retry(&mut self) -> Result<&mut Self> {
        self.update(self.target.retry_job(self.id()).await?)?;
        Ok(self)
    }
    /// Streams partial output using the same live/deployed decoding as result.
    pub fn stream(&self, timeout: Duration) -> BoxStream<'static, Result<Value>> {
        self.target.stream_job(self.id(), timeout)
    }
}

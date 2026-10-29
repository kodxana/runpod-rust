use super::{
    Job,
    targets::{DEFAULT_TIMEOUT, JobOptions, ResourceAddress, Target, merge_job_options},
};
use crate::{Client, Error, Result, Value, json};
use futures_util::{StreamExt, stream::BoxStream};
use serde_json::Map;
use std::{fmt, time::Duration};

/// A resource identified by exactly one of its logical name or backend ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// Logical resource name within an application.
    Name(String),
    /// Backend resource ID; still resolved through the application sentinel.
    Id(String),
}
impl ResourceRef {
    /// Returns the sentinel resource routing key.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Name(name) | Self::Id(name) => name,
        }
    }
}

/// Configuration for a client of a resource deployed from another codebase.
#[derive(Debug, Clone)]
pub struct StubConfig {
    /// Owning application name.
    pub app: String,
    /// Logical resource name or backend ID.
    pub resource: ResourceRef,
    /// Explicit environment, otherwise FLASH_ENVIRONMENT and then "default".
    pub environment: Option<String>,
}
impl StubConfig {
    /// Creates configuration with environment resolution deferred to client construction.
    pub fn new(app: impl Into<String>, resource: ResourceRef) -> Self {
        Self {
            app: app.into(),
            resource,
            environment: None,
        }
    }
    fn address(self) -> ResourceAddress {
        let environment = self
            .environment
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("FLASH_ENVIRONMENT")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "default".into());
        ResourceAddress::new(self.app, environment, self.resource.as_str())
    }
}

/// Client for a deployed Apps queue, including per-job options and reconnectable jobs.
#[derive(Clone)]
pub struct Queue {
    target: Target,
    options: Value,
}
impl fmt::Debug for Queue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue")
            .field("target", &self.target)
            .field("options", &"[REDACTED]")
            .finish()
    }
}
impl Queue {
    /// Binds an application resource, resolving environment defaults once.
    ///
    /// # Errors
    /// Returns invalid service configuration or routing header errors.
    pub fn new(client: Client, config: StubConfig) -> Result<Self> {
        Ok(Self {
            target: Target::sentinel(client, config.address())?,
            options: json!({}),
        })
    }
    /// Returns a new client with options accumulated, leaving this client unchanged.
    ///
    /// # Errors
    /// Returns malformed policy errors.
    pub fn with_options(&self, options: &JobOptions) -> Result<Self> {
        Ok(Self {
            target: self.target.clone(),
            options: merge_job_options(&self.options, &options.to_payload())?,
        })
    }
    fn payload(&self, input: &Map<String, Value>) -> Value {
        let mut payload = self.options.clone();
        payload["input"] = if input.is_empty() {
            json!({"__empty":true})
        } else {
            Value::Object(input.clone())
        };
        payload
    }
    /// Invokes a deployed queue function with the default five-minute timeout.
    ///
    /// # Errors
    /// Returns submission, polling, deadline, or remote execution errors.
    pub async fn remote(&self, input: &Map<String, Value>) -> Result<Value> {
        self.remote_with_timeout(input, DEFAULT_TIMEOUT).await
    }
    /// Invokes a deployed queue function with an explicit wait deadline.
    ///
    /// # Errors
    /// Returns submission, polling, deadline, or remote execution errors.
    pub async fn remote_with_timeout(
        &self,
        input: &Map<String, Value>,
        timeout: Duration,
    ) -> Result<Value> {
        self.target.invoke(&self.payload(input), timeout).await
    }
    /// Submits a function call and returns its job handle immediately.
    ///
    /// # Errors
    /// Returns submission or invalid admission-response errors.
    pub async fn spawn(&self, input: &Map<String, Value>) -> Result<Job> {
        Job::new(
            self.target.submit(&self.payload(input)).await?,
            self.target.clone(),
        )
    }
    /// Reconnects to a known job without making a request until it is queried.
    pub fn job(&self, id: impl Into<String>) -> Job {
        Job::new(
            json!({"id":id.into(),"status":"UNKNOWN"}),
            self.target.clone(),
        )
        .expect("reconnected job data is an object")
    }
    /// Lazily submits and streams a call using the default timeout.
    pub fn stream(&self, input: Map<String, Value>) -> BoxStream<'static, Result<Value>> {
        self.stream_with_timeout(input, DEFAULT_TIMEOUT)
    }
    /// Lazily submits and streams a call with an explicit streaming deadline.
    pub fn stream_with_timeout(
        &self,
        input: Map<String, Value>,
        timeout: Duration,
    ) -> BoxStream<'static, Result<Value>> {
        let queue = self.clone();
        Box::pin(async_stream::try_stream! {
            let data=queue.target.submit(&queue.payload(&input)).await?;
            let id=data.get("id").and_then(Value::as_str)
                .ok_or_else(||Error::InvalidResponse("submitted job response is missing a job id".into()))?;
            let mut chunks=queue.target.stream_job(id,timeout);
            while let Some(chunk)=chunks.next().await {yield chunk?;}
        })
    }
}

/// HTTP route client for an application API deployed from another codebase.
#[derive(Debug, Clone)]
pub struct Api {
    target: Target,
}
impl Api {
    /// Binds an API resource, resolving environment defaults once.
    ///
    /// # Errors
    /// Returns invalid service configuration or routing header errors.
    pub fn new(client: Client, config: StubConfig) -> Result<Self> {
        Ok(Self {
            target: Target::sentinel(client, config.address())?,
        })
    }
    /// Overrides the route service root for local workers or private gateways.
    ///
    /// # Errors
    /// Returns invalid load-balancer URL errors.
    pub fn set_load_balancer_url(&mut self, url: impl Into<String>) -> Result<()> {
        self.target.set_load_balancer_url(url)
    }
    /// Sends an HTTP route call with an explicit method and timeout.
    ///
    /// # Errors
    /// Returns invalid route, routing, or HTTP errors.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        self.target.request(method, path, body, timeout).await
    }
    /// Sends GET with the default timeout, returning JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub async fn get(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("GET", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends POST with the default timeout, returning JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub async fn post(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("POST", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends PUT with the default timeout, returning JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub async fn put(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("PUT", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends DELETE with the default timeout, returning JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub async fn delete(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("DELETE", path, body, DEFAULT_TIMEOUT).await
    }
    /// Sends PATCH with the default timeout, returning JSON or text.
    ///
    /// # Errors
    /// Returns routing or HTTP errors.
    pub async fn patch(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.request("PATCH", path, body, DEFAULT_TIMEOUT).await
    }
}

//! Invocation payloads, result handling, and destinations for application resources.
pub use super::queue::{
    DEFAULT_TIMEOUT, FINAL_STATUSES, QueueClient, ResourceAddress, SENTINEL_ID, StatusObserver,
};
use super::{AppError, protocol::FunctionResponse, serialization};
use super::{
    ResourceSpec,
    events::{EventSink, emit},
    protocol::{FunctionRequest, SerializationFormat},
    serialization::{FunctionSource, get_function_source},
};
use crate::Client;
use crate::{Error, Result, Value, client::truthy, control::python_string, json};
use futures_util::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc, time::Duration};

/// Function metadata supplied explicitly in place of Python signature/source reflection.
#[derive(Debug, Clone)]
pub struct FunctionDefinition {
    /// Function symbol name.
    pub name: String,
    /// Positional parameter names in declaration order.
    pub parameters: Vec<String>,
    /// Module source for live invocations; deployed invocations need no source.
    pub source: Option<FunctionSource>,
}

#[derive(Debug, Clone)]
struct SourceAttachment {
    source: FunctionSource,
    resource: String,
    spec: Option<ResourceSpec>,
}
#[derive(Debug, Clone)]
enum TargetMode {
    Deployed,
    Live {
        source: Option<Arc<SourceAttachment>>,
        synced_hash: Arc<tokio::sync::Mutex<Option<[u8; 32]>>>,
        events: Option<EventSink>,
    },
}

/// A deployed sentinel destination or live endpoint, owning its payload/result contract.
#[derive(Clone)]
pub struct Target {
    queue: QueueClient,
    mode: TargetMode,
    resource_name: String,
}
impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target")
            .field("endpoint_id", &self.queue.endpoint_id())
            .field("resource_name", &self.resource_name)
            .field(
                "mode",
                &match self.mode {
                    TargetMode::Deployed => "deployed",
                    TargetMode::Live { .. } => "live",
                },
            )
            .finish_non_exhaustive()
    }
}
impl Target {
    /// Routes a deployed resource through the server's flash sentinel endpoint.
    ///
    /// # Errors
    /// Returns malformed connection configuration or routing header errors.
    pub fn sentinel(client: Client, address: ResourceAddress) -> Result<Self> {
        let resource_name = address.resource.clone();
        let mut queue = QueueClient::new(client, SENTINEL_ID)?;
        queue.set_address(address)?;
        Ok(Self {
            queue,
            mode: TargetMode::Deployed,
            resource_name,
        })
    }
    /// Binds a live endpoint whose requests carry source and function metadata.
    ///
    /// # Errors
    /// Returns malformed connection configuration errors.
    pub fn live(
        client: Client,
        endpoint_id: impl Into<String>,
        resource_name: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            queue: QueueClient::new(client, endpoint_id)?,
            mode: TargetMode::Live {
                source: None,
                synced_hash: Arc::new(tokio::sync::Mutex::new(None)),
                events: None,
            },
            resource_name: resource_name.into(),
        })
    }
    /// Returns the actual queue endpoint or flash sentinel identifier.
    pub fn endpoint_id(&self) -> &str {
        self.queue.endpoint_id()
    }
    /// Overrides HTTP-route addressing for private gateways or local testing.
    ///
    /// # Errors
    /// Returns invalid load-balancer URL errors.
    pub fn set_load_balancer_url(&mut self, url: impl Into<String>) -> Result<()> {
        self.queue.set_load_balancer_url(url)
    }
    /// Attaches module source for lazy, coordinated live HTTP route synchronization.
    ///
    /// # Errors
    /// Returns invalid input if called on a deployed target.
    pub fn attach_source(
        &mut self,
        source: FunctionSource,
        resource: impl Into<String>,
        spec: Option<ResourceSpec>,
    ) -> Result<()> {
        let TargetMode::Live {
            source: attachment, ..
        } = &mut self.mode
        else {
            return Err(Error::InvalidInput(
                "source synchronization requires a live target".into(),
            ));
        };
        *attachment = Some(Arc::new(SourceAttachment {
            source,
            resource: resource.into(),
            spec,
        }));
        Ok(())
    }
    /// Attaches dispatch and completion observation to a live target.
    ///
    /// # Errors
    /// Returns invalid input for deployed targets.
    pub fn set_events(&mut self, sink: EventSink) -> Result<()> {
        let TargetMode::Live { events, .. } = &mut self.mode else {
            return Err(Error::InvalidInput(
                "live events require a live target".into(),
            ));
        };
        *events = Some(sink);
        Ok(())
    }
    fn events(&self) -> Option<&EventSink> {
        match &self.mode {
            TargetMode::Deployed => None,
            TargetMode::Live { events, .. } => events.as_ref(),
        }
    }

    /// Builds named deployed input or a source-bearing live FunctionRequest.
    ///
    /// # Errors
    /// Returns argument-binding, missing-source, file, or serialization errors.
    pub async fn build_payload(
        &self,
        function: &FunctionDefinition,
        spec: &ResourceSpec,
        args: &[Value],
        kwargs: &Map<String, Value>,
    ) -> Result<Value> {
        if matches!(self.mode, TargetMode::Deployed) {
            return Ok(
                json!({"input":args_to_input(&function.name,&function.parameters,args,kwargs)?}),
            );
        }
        let source = function.source.as_ref().ok_or_else(|| {
            Error::InvalidInput(format!(
                "live function '{}' requires module source",
                function.name
            ))
        })?;
        let request = FunctionRequest {
            function_name: function.name.clone(),
            function_code: Some(get_function_source(source).await?),
            args: args.to_vec(),
            kwargs: kwargs.clone(),
            dependencies: spec.dependencies.clone(),
            system_dependencies: spec.system_dependencies.clone(),
            accelerate_downloads: spec.accelerate_downloads,
            serialization_format: SerializationFormat::Json,
            ..FunctionRequest::default()
        };
        Ok(json!({"input":request.to_input()?}))
    }

    /// Extracts a result according to the target's deployed or live response format.
    ///
    /// # Errors
    /// Returns remote execution and serialized result errors.
    pub fn unwrap(&self, data: &Value) -> Result<Value> {
        match self.mode {
            TargetMode::Deployed => unwrap_job_output(data),
            TargetMode::Live { .. } => unwrap_live_output(data),
        }
    }

    /// Submits once through runsync and waits for the same job to finish.
    ///
    /// # Errors
    /// Returns request, deadline, or remote execution errors.
    pub async fn invoke(&self, payload: &Value, timeout: Duration) -> Result<Value> {
        emit(self.events(), "dispatch", &[json!(self.resource_name)]);
        let start = tokio::time::Instant::now();
        let result = async {
            let mut observe = |_: &Value| Ok(());
            let observer = if self.events().is_some() {
                Some(&mut observe as &mut StatusObserver<'_>)
            } else {
                None
            };
            let data = self.queue.invoke(payload, timeout, observer).await?;
            self.unwrap(&data)
        }
        .await;
        emit(
            self.events(),
            if result.is_ok() {
                "request_completed"
            } else {
                "request_failed"
            },
            &[
                json!(self.resource_name),
                json!(start.elapsed().as_secs_f64()),
            ],
        );
        result
    }
    /// Submits without waiting for output.
    ///
    /// # Errors
    /// Returns request or malformed admission response errors.
    pub async fn submit(&self, payload: &Value) -> Result<Value> {
        self.queue.run(payload, DEFAULT_TIMEOUT).await
    }
    /// Reads a submitted job's status.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn job_status(&self, id: &str) -> Result<Value> {
        self.queue.status(id).await
    }
    /// Cancels a submitted job.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn cancel_job(&self, id: &str) -> Result<Value> {
        self.queue.cancel(id).await
    }
    /// Retries a failed or timed-out job.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn retry_job(&self, id: &str) -> Result<Value> {
        self.queue.retry(id).await
    }
    /// Streams partial outputs, decoding live function envelopes when necessary.
    pub fn stream_job(&self, id: &str, timeout: Duration) -> BoxStream<'static, Result<Value>> {
        let live = matches!(self.mode, TargetMode::Live { .. });
        Box::pin(self.queue.stream(id, timeout).map(move |chunk| {
            if live {
                chunk.and_then(|v| unwrap_function_response(&v))
            } else {
                chunk
            }
        }))
    }
    /// Waits for a submitted job, observing raw status data and returning its decoded result.
    ///
    /// # Errors
    /// Returns request, observation, deadline, or remote execution errors.
    pub async fn wait(
        &self,
        data: Value,
        timeout: Duration,
        on_status: Option<&mut StatusObserver<'_>>,
    ) -> Result<Value> {
        self.unwrap(&self.queue.wait(data, timeout, on_status).await?)
    }

    async fn sync_source(&self, timeout: Duration) -> Result<()> {
        if let TargetMode::Live {
            source: Some(source),
            synced_hash,
            ..
        } = &self.mode
        {
            let mut synced = synced_hash.lock().await;
            let text = get_function_source(&source.source).await?;
            let hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
            if *synced == Some(hash) {
                return Ok(());
            }
            let mut payload = json!({"source":text,"resource":source.resource});
            if let Some(spec) = &source.spec {
                payload["dependencies"] = json!(spec.dependencies);
                payload["system_dependencies"] = json!(spec.system_dependencies);
            }
            self.queue.sync_source(&payload, timeout).await?;
            *synced = Some(hash);
        }
        Ok(())
    }

    /// Calls a load-balanced HTTP resource after synchronizing changed live source.
    ///
    /// # Errors
    /// Returns source synchronization, routing, or HTTP errors without executing after a failed sync.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        emit(
            self.events(),
            "dispatch",
            &[json!(self.resource_name), json!(format!("{method} {path}"))],
        );
        let start = tokio::time::Instant::now();
        let result = async {
            self.sync_source(timeout).await?;
            self.queue.http(method, path, body, timeout).await
        }
        .await;
        emit(
            self.events(),
            if result.is_ok() {
                "request_completed"
            } else {
                "request_failed"
            },
            &[
                json!(self.resource_name),
                json!(start.elapsed().as_secs_f64()),
            ],
        );
        result
    }
}

/// Maps positional arguments to declared parameter names, excluding self and cls.
///
/// Rust callers provide parameter names explicitly because compiled functions have no
/// runtime signature reflection. An empty call preserves the platform's __empty marker.
///
/// # Errors
/// Rejects excess positional arguments and duplicate positional/keyword bindings.
pub fn args_to_input(
    name: &str,
    parameters: &[String],
    args: &[Value],
    kwargs: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let parameters: Vec<_> = parameters
        .iter()
        .filter(|p| *p != "self" && *p != "cls")
        .collect();
    let mut body = Map::new();
    for (index, arg) in args.iter().enumerate() {
        let Some(parameter) = parameters.get(index) else {
            return Err(Error::InvalidInput(format!(
                "{name}() got {} positional args, expected at most {}",
                args.len(),
                parameters.len()
            )));
        };
        if kwargs.contains_key(*parameter) {
            return Err(Error::InvalidInput(format!(
                "{name}() got multiple values for argument '{parameter}'"
            )));
        }
        body.insert((*parameter).clone(), arg.clone());
    }
    body.extend(kwargs.clone());
    if body.is_empty() {
        body.insert("__empty".into(), Value::Bool(true));
    }
    Ok(body)
}

/// Per-invocation queue options, including the nested policy fields.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JobOptions {
    /// Completion callback URL.
    pub webhook: Option<String>,
    /// Execution timeout in milliseconds.
    pub execution_timeout: Option<i64>,
    /// Job time-to-live in milliseconds.
    pub ttl: Option<i64>,
    /// Whether to enqueue with low priority.
    pub low_priority: Option<bool>,
    /// Optional S3 output configuration.
    pub s3_config: Option<Value>,
}
impl fmt::Debug for JobOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobOptions")
            .field("webhook", &self.webhook.as_ref().map(|_| "[REDACTED]"))
            .field("execution_timeout", &self.execution_timeout)
            .field("ttl", &self.ttl)
            .field("low_priority", &self.low_priority)
            .field("s3_config", &self.s3_config.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}
impl JobOptions {
    /// Converts options into fields alongside the input object, including explicit zero/false.
    pub fn to_payload(&self) -> Value {
        let mut body = Map::new();
        if let Some(v) = &self.webhook {
            body.insert("webhook".into(), json!(v));
        }
        let mut policy = Map::new();
        if let Some(v) = self.execution_timeout {
            policy.insert("executionTimeout".into(), json!(v));
        }
        if let Some(v) = self.ttl {
            policy.insert("ttl".into(), json!(v));
        }
        if let Some(v) = self.low_priority {
            policy.insert("lowPriority".into(), json!(v));
        }
        if !policy.is_empty() {
            body.insert("policy".into(), Value::Object(policy));
        }
        if let Some(v) = &self.s3_config {
            body.insert("s3Config".into(), v.clone());
        }
        Value::Object(body)
    }
}

/// Merges queue options while accumulating policy fields across chained calls.
///
/// # Errors
/// Returns invalid input when options or merged policy values are not objects.
pub fn merge_job_options(base: &Value, new: &Value) -> Result<Value> {
    let mut merged = base
        .as_object()
        .cloned()
        .ok_or_else(|| Error::InvalidInput("job options must be objects".into()))?;
    for (key, value) in new
        .as_object()
        .ok_or_else(|| Error::InvalidInput("job options must be objects".into()))?
    {
        if key == "policy"
            && let Some(policy) = merged.get_mut(key).and_then(Value::as_object_mut)
        {
            policy.extend(
                value
                    .as_object()
                    .ok_or_else(|| Error::InvalidInput("policy must be an object".into()))?
                    .clone(),
            );
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(merged))
}

/// Extracts queue output and retains nested or streamed error details.
///
/// # Errors
/// Returns RemoteExecution for failed jobs or error-bearing output objects.
pub fn unwrap_job_output(data: &Value) -> Result<Value> {
    let output = data.get("output").unwrap_or(data);
    if data["status"] == "FAILED" || data.get("error").is_some_and(truthy) {
        let error = data.get("error").filter(|v| truthy(v)).or_else(|| {
            if output.is_object() {
                output.get("error").filter(|v| truthy(v))
            } else {
                output.as_array().and_then(|a| {
                    a.iter()
                        .rev()
                        .find_map(|v| v.get("error").filter(|v| truthy(v)))
                })
            }
        });
        return Err(AppError::RemoteExecution(format!(
            "remote execution failed: {}",
            error.map_or_else(|| "unknown".into(), python_string)
        ))
        .into());
    }
    if let Some(error) = output.get("error") {
        return Err(AppError::RemoteExecution(format!(
            "remote execution failed: {}",
            python_string(error)
        ))
        .into());
    }
    Ok(output.clone())
}

/// Decodes a live worker's function response or aggregated generator output.
///
/// # Errors
/// Returns remote execution or serialized result errors.
pub fn unwrap_live_output(data: &Value) -> Result<Value> {
    let output = unwrap_job_output(data)?;
    if let Value::Array(values) = output {
        if values.len() == 1
            && values[0].is_object()
            && !values[0].get("__stream__").is_some_and(truthy)
        {
            return unwrap_function_response(&values[0]);
        }
        return values
            .iter()
            .map(unwrap_function_response)
            .collect::<Result<Vec<_>>>()
            .map(Value::Array);
    }
    unwrap_function_response(&output)
}

pub(crate) fn unwrap_function_response(output: &Value) -> Result<Value> {
    if output.get("success").is_some() {
        let response = FunctionResponse::from_output(output)?;
        if !response.success {
            return Err(AppError::RemoteExecution(format!(
                "remote execution failed: {}",
                response
                    .error
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("unknown")
            ))
            .into());
        }
        if let Some(result) = response.result {
            return serialization::deserialize_result(&result);
        }
        return Ok(response.json_result);
    }
    Ok(output.clone())
}

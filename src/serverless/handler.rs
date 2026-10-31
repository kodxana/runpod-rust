use super::{
    WorkerJob,
    capture::{Capture, CaptureMode},
};
use crate::{Result, Value, client::truthy, json};
use futures_util::{Stream, StreamExt, stream::BoxStream};
use std::{fmt, future::Future, sync::Arc};

/// Determines whether handler values form a final result or a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    /// One result per invocation.
    Single,
    /// Incremental output followed by a completion envelope.
    Streaming,
}

/// A cloneable synchronous, asynchronous, or streaming Rust handler.
#[derive(Clone)]
pub struct Handler {
    kind: HandlerKind,
    call: Arc<dyn Fn(WorkerJob) -> BoxStream<'static, Result<Value>> + Send + Sync>,
}
impl fmt::Debug for Handler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handler")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}
impl Handler {
    /// Wraps an asynchronous single-result function.
    pub fn new<F, Fut>(handler: F) -> Self
    where
        F: Fn(WorkerJob) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self {
            kind: HandlerKind::Single,
            call: Arc::new(move |job| futures_util::stream::once(handler(job)).boxed()),
        }
    }
    /// Wraps a synchronous function; CPU-intensive work should use the caller's executor.
    pub fn sync<F>(handler: F) -> Self
    where
        F: Fn(WorkerJob) -> Result<Value> + Send + Sync + 'static,
    {
        Self {
            kind: HandlerKind::Single,
            call: Arc::new(move |job| {
                let value = handler(job);
                futures_util::stream::once(async move { value }).boxed()
            }),
        }
    }
    /// Wraps a function producing an asynchronous stream of partial results.
    pub fn streaming<F, S>(handler: F) -> Self
    where
        F: Fn(WorkerJob) -> S + Send + Sync + 'static,
        S: Stream<Item = Result<Value>> + Send + 'static,
    {
        Self {
            kind: HandlerKind::Streaming,
            call: Arc::new(move |job| handler(job).boxed()),
        }
    }
    /// Returns the handler's result protocol.
    pub fn kind(&self) -> HandlerKind {
        self.kind
    }
    /// Invokes the handler and returns its result stream.
    pub fn run(&self, job: WorkerJob) -> BoxStream<'static, Result<Value>> {
        (self.call)(job)
    }

    pub(crate) fn captured(
        self,
        job: WorkerJob,
        mode: CaptureMode,
    ) -> (BoxStream<'static, Result<Value>>, Capture) {
        let captured = Capture::new();
        let context = captured.clone();
        let handler = self;
        let stream = async_stream::stream! {
            let enabled = mode.handler_capture_enabled();
            let mut stream = if enabled {
                context.scope_sync(|| handler.run(job))
            } else {handler.run(job)};
            loop {
                let next = if enabled {context.scope(stream.next()).await} else {stream.next().await};
                match next {Some(value) => yield value, None => break}
            }
        }.boxed();
        (stream, captured)
    }
}

/// Moves reserved error and refresh fields into the Runpod result envelope.
pub fn normalize_output(mut output: Value) -> Value {
    let mut result = json!({});
    if let Some(object) = output.as_object_mut() {
        if let Some(error) = object.remove("error").filter(truthy) {
            result["error"] = error;
        }
        if object.remove("refresh_worker").is_some_and(|v| truthy(&v)) {
            result["stopPod"] = json!(true);
        }
    }
    if !output.as_object().is_some_and(serde_json::Map::is_empty) {
        result["output"] = output;
    }
    result
}

/// Runs an async handler, returning the upstream-compatible JSON result envelope.
///
/// Handler errors become structured error JSON. Panics retain Rust's panic semantics.
pub async fn run_job<F, Fut>(job: WorkerJob, handler: F) -> Value
where
    F: FnOnce(WorkerJob) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    run_job_with_capture(job, CaptureMode::from_env(), handler).await
}

/// Run a handler with an explicit log-capture policy, without reading capture environment settings.
/// Handler errors retain their full message and native traceback; only captured logs are bounded.
pub async fn run_job_with_capture<F, Fut>(job: WorkerJob, mode: CaptureMode, handler: F) -> Value
where
    F: FnOnce(WorkerJob) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let captured = Capture::new();
    let invocation = async { handler(job).await };
    let result = if mode.handler_capture_enabled() {
        captured.scope(invocation).await
    } else {
        invocation.await
    };
    match result {
        Ok(value) => normalize_output(value),
        Err(error) => handler_error(&error, &captured.text()),
    }
}

/// Run a handler stream as output/error envelopes, reading capture policy when creating the stream.
/// The first handler error ends the stream; panics retain Rust's panic semantics.
pub fn run_job_generator(job: WorkerJob, handler: Handler) -> BoxStream<'static, Value> {
    run_job_generator_with_capture(job, CaptureMode::from_env(), handler)
}

/// Stream output envelopes with an explicit capture policy and one terminal error envelope.
/// Partial values are preserved, including caller-owned objects containing an `error` key.
pub fn run_job_generator_with_capture(
    job: WorkerJob,
    mode: CaptureMode,
    handler: Handler,
) -> BoxStream<'static, Value> {
    let job_id = job.id.clone();
    let (mut stream, captured) = handler.captured(job, mode);
    async_stream::stream! {
        let completion = GeneratorCompletion(job_id);
        while let Some(item) = stream.next().await {
            match item {
                Ok(value) => yield json!({"output":value}),
                Err(error) => {
                    tracing::error!(%error, job_id=%completion.0, "generator handler failed");
                    yield stream_error(&error, &captured.text());
                    break;
                }
            }
        }
    }
    .boxed()
}

struct GeneratorCompletion(String);
impl Drop for GeneratorCompletion {
    fn drop(&mut self) {
        tracing::info!(job_id=%self.0, "Finished running generator.");
    }
}

pub(crate) fn handler_error(error: &crate::Error, logs: &str) -> Value {
    let mut payload = json!({
        "error_type":"runpod::Error", "error_message":error.to_string(),
        "error_traceback":native_traceback(error),
        "hostname":std::env::var("RUNPOD_POD_HOSTNAME").unwrap_or_else(|_|"unknown".into()),
        "worker_id":std::env::var("RUNPOD_POD_ID").unwrap_or_else(|_|"unknown".into()),
        "runpod_version":env!("CARGO_PKG_VERSION")
    });
    if !logs.is_empty() {
        payload["logs"] = json!(logs);
    }
    json!({"error":payload.to_string()})
}

pub(crate) fn stream_error(error: &crate::Error, logs: &str) -> Value {
    let mut message = format!("handler: {error} \ntraceback: {}", native_traceback(error));
    if !logs.is_empty() {
        message.push_str("\nlogs:\n");
        message.push_str(logs);
    }
    json!({"error":message})
}

fn native_traceback(error: &crate::Error) -> String {
    format!(
        "runpod::Error: {error}\n{}",
        std::backtrace::Backtrace::force_capture()
    )
}

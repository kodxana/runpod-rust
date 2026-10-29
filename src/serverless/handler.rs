use super::WorkerJob;
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
    match handler(job).await {
        Ok(value) => normalize_output(value),
        Err(error) => handler_error(&error),
    }
}

pub(crate) fn handler_error(error: &crate::Error) -> Value {
    json!({"error":json!({
        "error_type":"runpod::Error", "error_message":error.to_string(),
        "error_traceback":std::backtrace::Backtrace::capture().to_string(),
        "hostname":std::env::var("RUNPOD_POD_HOSTNAME").unwrap_or_else(|_|"unknown".into()),
        "worker_id":std::env::var("RUNPOD_POD_ID").unwrap_or_else(|_|"unknown".into()),
        "runpod_version":env!("CARGO_PKG_VERSION")
    }).to_string()})
}

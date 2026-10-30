use crate::{Result, Value};
use futures_util::{Stream, StreamExt, TryStreamExt, future::BoxFuture, stream::BoxStream};
use serde_json::Map;
use std::{fmt, future::Future, sync::Arc};

/// Positional and named JSON arguments passed to a registered native function.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FunctionArgs {
    /// Positional arguments retain caller order.
    pub args: Vec<Value>,
    /// Named arguments, independent of positional binding for local execution.
    pub kwargs: Map<String, Value>,
}

#[derive(Clone)]
enum FunctionBody {
    Sync(Arc<dyn Fn(FunctionArgs) -> Result<Value> + Send + Sync>),
    Async(Arc<dyn Fn(FunctionArgs) -> BoxFuture<'static, Result<Value>> + Send + Sync>),
    Stream(Arc<dyn Fn(FunctionArgs) -> BoxStream<'static, Result<Value>> + Send + Sync>),
}

/// Owned, cloneable Rust function body with explicit synchronous, asynchronous, or streaming behavior.
#[derive(Clone)]
pub struct NativeFunction {
    body: FunctionBody,
}
impl fmt::Debug for NativeFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeFunction")
            .field("streaming", &self.is_streaming())
            .finish_non_exhaustive()
    }
}
impl NativeFunction {
    /// Registers an asynchronous function without executing it.
    pub fn new<F, Fut>(function: F) -> Self
    where
        F: Fn(FunctionArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self {
            body: FunctionBody::Async(Arc::new(move |args| Box::pin(function(args)))),
        }
    }

    /// Registers a synchronous function, offloaded to Tokio's blocking pool on async invocation.
    pub fn sync<F>(function: F) -> Self
    where
        F: Fn(FunctionArgs) -> Result<Value> + Send + Sync + 'static,
    {
        Self {
            body: FunctionBody::Sync(Arc::new(function)),
        }
    }

    /// Registers a lazy stream factory; use an iterator stream for synchronous generators.
    pub fn streaming<F, S>(function: F) -> Self
    where
        F: Fn(FunctionArgs) -> S + Send + Sync + 'static,
        S: Stream<Item = Result<Value>> + Send + 'static,
    {
        Self {
            body: FunctionBody::Stream(Arc::new(move |args| function(args).boxed())),
        }
    }

    /// Reports whether calls produce incremental output.
    #[must_use]
    pub fn is_streaming(&self) -> bool {
        matches!(self.body, FunctionBody::Stream(_))
    }

    pub(crate) fn stream(&self, args: FunctionArgs) -> BoxStream<'static, Result<Value>> {
        match &self.body {
            FunctionBody::Sync(function) => {
                let function = function.clone();
                Box::pin(futures_util::stream::once(async move {
                    tokio::task::spawn_blocking(move || function(args)).await?
                }))
            }
            FunctionBody::Async(function) => Box::pin(futures_util::stream::once(function(args))),
            FunctionBody::Stream(function) => function(args),
        }
    }

    pub(crate) async fn call(&self, args: FunctionArgs) -> Result<Value> {
        let mut stream = self.stream(args);
        if self.is_streaming() {
            Ok(Value::Array(stream.try_collect().await?))
        } else {
            // Single-result bodies always construct a one-item stream, including JSON null.
            stream
                .next()
                .await
                .expect("single-result function produces exactly one item")
        }
    }
}

use super::FunctionArgs;
use crate::Result;
use axum::response::Response;
use futures_util::future::BoxFuture;
use std::{fmt, future::Future, sync::Arc};

#[derive(Clone)]
enum Body {
    Sync(Arc<dyn Fn(FunctionArgs) -> Result<Response> + Send + Sync>),
    Async(Arc<dyn Fn(FunctionArgs) -> BoxFuture<'static, Result<Response>> + Send + Sync>),
}

/// Native API method returning an owned Axum HTTP response, including streaming bodies.
///
/// Arguments follow the class API's JSON-body convention. Status, headers (including
/// repeated headers), and body pass through unchanged. Use `axum::body::Body::from_stream`
/// for incremental output; no buffering or JSON conversion is applied to responses.
#[derive(Clone)]
pub struct NativeHttpFunction {
    body: Body,
}

impl fmt::Debug for NativeHttpFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeHttpFunction").finish_non_exhaustive()
    }
}

impl NativeHttpFunction {
    /// Registers an asynchronous handler without executing it.
    pub fn new<F, Fut>(function: F) -> Self
    where
        F: Fn(FunctionArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response>> + Send + 'static,
    {
        Self {
            body: Body::Async(Arc::new(move |args| Box::pin(function(args)))),
        }
    }

    /// Registers a synchronous handler, offloaded to Tokio's blocking pool on invocation.
    pub fn sync<F>(function: F) -> Self
    where
        F: Fn(FunctionArgs) -> Result<Response> + Send + Sync + 'static,
    {
        Self {
            body: Body::Sync(Arc::new(function)),
        }
    }

    pub(super) async fn call(&self, args: FunctionArgs) -> Result<Response> {
        match &self.body {
            Body::Sync(function) => {
                let function = function.clone();
                tokio::task::spawn_blocking(move || function(args)).await?
            }
            Body::Async(function) => function(args).await,
        }
    }
}

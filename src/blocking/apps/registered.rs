use super::{Job, OutputIter};
use crate::{
    Error, Result, Value,
    apps::{
        AppConfig, FunctionArgs, NativeApi, NativeFunction, ResourceSpec,
        deploy::AppDefinition,
        targets::{FunctionDefinition, JobOptions},
    },
};
use serde_json::Map;
use std::{sync::Arc, time::Duration};

/// Synchronous application registration and invocation, owning a shared Tokio runtime.
///
/// Construct and drop blocking handles outside a Tokio runtime. Call blocking methods from
/// synchronous code, never while polling an async task; use `as_async` there. Jobs and
/// iterators keep the runtime alive after this app is dropped.
#[derive(Debug, Clone)]
pub struct App {
    inner: crate::apps::App,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl App {
    /// Creates a blocking app with lazy credentials and default routing.
    ///
    /// # Errors
    /// Rejects an empty name, runtime creation failure, or construction inside a Tokio runtime.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        Self::from_app(crate::apps::App::new(name)?)
    }
    /// Creates a blocking app with explicit invocation and task configuration.
    ///
    /// # Errors
    /// Returns name, runtime creation, or nested-runtime errors.
    pub fn with_config(name: impl Into<String>, config: AppConfig) -> Result<Self> {
        Self::from_app(crate::apps::App::with_config(name, config)?)
    }
    /// Wraps existing native declarations, retaining their shared registry and routing state.
    ///
    /// # Errors
    /// Returns runtime creation or nested-runtime errors.
    pub fn from_app(inner: crate::apps::App) -> Result<Self> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(Error::InvalidInput(
                "blocking application must be created outside a Tokio runtime".into(),
            ));
        }
        Ok(Self {
            inner,
            runtime: Arc::new(tokio::runtime::Runtime::new()?),
        })
    }
    /// Borrows native app access for metadata, target bindings, and event configuration.
    #[must_use]
    pub fn as_async(&self) -> &crate::apps::App {
        &self.inner
    }
    /// Returns an owned metadata snapshot in registration order.
    ///
    /// # Errors
    /// Returns poisoned registration errors.
    pub fn definition(&self) -> Result<AppDefinition> {
        self.inner.definition()
    }
    /// Registers a queue body and returns a handle sharing this runtime.
    ///
    /// # Errors
    /// Returns resource validation, duplicate registration, or poisoned-state errors.
    pub fn queue(
        &self,
        spec: ResourceSpec,
        function: FunctionDefinition,
        body: NativeFunction,
    ) -> Result<FunctionHandle> {
        Ok(FunctionHandle {
            inner: self.inner.queue(spec, function, body)?,
            runtime: self.runtime.clone(),
        })
    }
    /// Registers a dedicated-pod body and returns a handle sharing this runtime.
    ///
    /// # Errors
    /// Returns resource validation, duplicate registration, or poisoned-state errors.
    pub fn task(
        &self,
        spec: ResourceSpec,
        function: FunctionDefinition,
        body: NativeFunction,
    ) -> Result<FunctionHandle> {
        Ok(FunctionHandle {
            inner: self.inner.task(spec, function, body)?,
            runtime: self.runtime.clone(),
        })
    }
    /// Registers native API declarations and returns a synchronous HTTP handle.
    ///
    /// # Errors
    /// Returns invalid routes/resources, duplicate registration, or poisoned-state errors.
    pub fn api(&self, spec: ResourceSpec, body: NativeApi) -> Result<ApiHandle> {
        Ok(ApiHandle {
            inner: self.inner.api(spec, body)?,
            runtime: self.runtime.clone(),
        })
    }
    /// Returns mixed function/API handles in registration order.
    ///
    /// # Errors
    /// Returns poisoned registration errors.
    pub fn resources(&self) -> Result<Vec<ResourceHandle>> {
        Ok(self
            .inner
            .resources()?
            .into_iter()
            .map(|resource| match resource {
                crate::apps::ResourceHandle::Function(inner) => {
                    ResourceHandle::Function(FunctionHandle {
                        inner,
                        runtime: self.runtime.clone(),
                    })
                }
                crate::apps::ResourceHandle::Api(inner) => ResourceHandle::Api(ApiHandle {
                    inner,
                    runtime: self.runtime.clone(),
                }),
            })
            .collect())
    }
}

/// One registered blocking function or API handle.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ResourceHandle {
    /// Queue or dedicated-pod function.
    Function(FunctionHandle),
    /// Load-balanced HTTP API.
    Api(ApiHandle),
}

/// Synchronous local/remote function calls, job submission, and streaming on an owned runtime.
#[derive(Debug, Clone)]
pub struct FunctionHandle {
    inner: crate::apps::FunctionHandle,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl FunctionHandle {
    /// Borrows native access for async invocation, hook/secret metadata, and scheduling intent.
    #[must_use]
    pub fn as_async(&self) -> &crate::apps::FunctionHandle {
        &self.inner
    }
    /// Returns the registered resource metadata snapshot.
    ///
    /// # Errors
    /// Returns poisoned registration errors.
    pub fn spec(&self) -> Result<ResourceSpec> {
        self.inner.spec()
    }
    /// Returns an independent option binding while retaining the same resource and runtime.
    ///
    /// # Errors
    /// Rejects task options or invalid policy fields.
    pub fn with_options(&self, options: &JobOptions) -> Result<Self> {
        Ok(Self {
            inner: self.inner.with_options(options)?,
            runtime: self.runtime.clone(),
        })
    }
    /// Executes the native startup hook explicitly.
    ///
    /// # Errors
    /// Returns hook or registration errors.
    pub fn initialize(&self) -> Result<()> {
        self.runtime.block_on(self.inner.initialize())
    }
    /// Executes the native body locally, collecting generator output.
    ///
    /// # Errors
    /// Returns handler, background task, or registration errors.
    pub fn local(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.local(args, kwargs))
    }
    /// Invokes the owning worker locally or resolves and calls a remote resource.
    ///
    /// # Errors
    /// Returns discovery, validation, resolution, handler, or execution errors.
    pub fn remote(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.remote(args, kwargs))
    }
    /// Submits once and returns a queue/task job retaining this runtime.
    ///
    /// # Errors
    /// Returns discovery, validation, resolution, or submission errors.
    pub fn spawn(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> Result<FunctionJob> {
        Ok(FunctionJob {
            inner: self.runtime.block_on(self.inner.spawn(args, kwargs))?,
            runtime: self.runtime.clone(),
        })
    }
    /// Reconnects a queue job without submitting a new request.
    ///
    /// # Errors
    /// Rejects dedicated-pod task reconnection or returns resolution errors.
    pub fn job(&self, id: impl Into<String>) -> Result<Job> {
        Ok(Job {
            inner: self.runtime.block_on(self.inner.job(id))?,
            runtime: self.runtime.clone(),
        })
    }
    /// Lazily invokes a queue generator and drives each chunk on the owned runtime.
    #[must_use]
    pub fn stream(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> OutputIter {
        OutputIter {
            inner: self.inner.stream(args, kwargs),
            runtime: self.runtime.clone(),
        }
    }
    /// Lazily drives local generator output without submitting remote work.
    #[must_use]
    pub fn local_stream(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> OutputIter {
        OutputIter {
            inner: self.inner.local_stream(args, kwargs),
            runtime: self.runtime.clone(),
        }
    }
}

/// Submitted queue/task work whose blocking operations keep their runtime alive.
#[derive(Debug)]
pub struct FunctionJob {
    inner: crate::apps::FunctionJob,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl FunctionJob {
    /// Borrows native job metadata, including queue ID or owned pod ID.
    #[must_use]
    pub fn as_async(&self) -> &crate::apps::FunctionJob {
        &self.inner
    }
    /// Waits for output with the queue/task timeout defaults of the native job.
    ///
    /// # Errors
    /// Returns request, execution, timeout, or cleanup errors.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<Value> {
        self.runtime.block_on(self.inner.wait(timeout))
    }
    /// Cancels a queue job or terminates its dedicated pod.
    ///
    /// # Errors
    /// Returns request or cleanup errors.
    pub fn cancel(&mut self) -> Result<()> {
        self.runtime.block_on(self.inner.cancel())
    }
}

/// Synchronous HTTP calls and explicit native method execution for a registered API.
#[derive(Debug, Clone)]
pub struct ApiHandle {
    inner: crate::apps::ApiHandle,
    runtime: Arc<tokio::runtime::Runtime>,
}
impl ApiHandle {
    /// Borrows native access for metadata, environment values, and async calls.
    #[must_use]
    pub fn as_async(&self) -> &crate::apps::ApiHandle {
        &self.inner
    }
    /// Returns the registered resource metadata snapshot.
    ///
    /// # Errors
    /// Returns poisoned registration errors.
    pub fn spec(&self) -> Result<ResourceSpec> {
        self.inner.spec()
    }
    /// Executes the native startup hook explicitly.
    ///
    /// # Errors
    /// Returns initialization, registration, or incompatible Python factory errors.
    pub fn initialize(&self) -> Result<()> {
        self.runtime.block_on(self.inner.initialize())
    }
    /// Calls a declared native method by handler name without making an HTTP request.
    ///
    /// # Errors
    /// Returns handler or registration errors.
    pub fn call_handler(&self, name: &str, args: FunctionArgs) -> Result<Value> {
        self.runtime.block_on(self.inner.call_handler(name, args))
    }
    /// Resolves and calls an HTTP route with an explicit timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, source synchronization, or HTTP errors.
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
    /// Sends GET using the default timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub fn get(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.get(path, body))
    }
    /// Sends POST using the default timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub fn post(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.post(path, body))
    }
    /// Sends PUT using the default timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub fn put(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.put(path, body))
    }
    /// Sends DELETE using the default timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub fn delete(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.delete(path, body))
    }
    /// Sends PATCH using the default timeout.
    ///
    /// # Errors
    /// Returns discovery, routing, synchronization, or HTTP errors.
    pub fn patch(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.runtime.block_on(self.inner.patch(path, body))
    }
}

use super::{
    App, AppError, Context, FunctionArgs, Job, NativeFunction, ResourceKind, ResourceSpec,
    app::{RegisteredFunction, ResolvedTarget},
    deploy::SymbolReference,
    targets::{DEFAULT_TIMEOUT, JobOptions, merge_job_options},
    tasks::TaskJob,
};
use crate::{Error, Result, Value, json};
use futures_util::{StreamExt, stream::BoxStream};
use serde_json::Map;
use std::fmt;

/// A registered native queue or task function with immutable per-call job options.
#[derive(Clone)]
pub struct FunctionHandle {
    app: App,
    index: usize,
    options: Value,
}
impl fmt::Debug for FunctionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionHandle")
            .field("app", &self.app)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}
impl FunctionHandle {
    pub(crate) fn new(app: App, index: usize) -> Self {
        Self {
            app,
            index,
            options: json!({}),
        }
    }
    fn registration(&self) -> Result<RegisteredFunction> {
        self.app.function(self.index)
    }

    /// Returns a resource snapshot; editing it never mutates registered routing state.
    ///
    /// # Errors
    /// Returns a poisoned registration error.
    pub fn spec(&self) -> Result<ResourceSpec> {
        Ok(self.registration()?.definition.spec)
    }

    /// Borrows this handle's per-invocation payload options.
    #[must_use]
    pub fn options(&self) -> &Value {
        &self.options
    }

    /// Returns a handle with accumulated options, deep-merging policy fields without changing this one.
    ///
    /// # Errors
    /// Rejects dedicated-pod tasks, poisoned registration, or invalid policy objects.
    pub fn with_options(&self, options: &JobOptions) -> Result<Self> {
        if self.spec()?.kind == ResourceKind::Task {
            return Err(AppError::InvalidResource(
                "per-job options apply only to @app.queue functions; tasks run on a dedicated pod"
                    .into(),
            )
            .into());
        }
        Ok(Self {
            app: self.app.clone(),
            index: self.index,
            options: merge_job_options(&self.options, &options.to_payload())?,
        })
    }

    /// Replaces the worker initialization hook; local calls never execute this hook automatically.
    ///
    /// # Errors
    /// Returns a poisoned registration error.
    pub fn init(&self, hook: NativeFunction) -> Result<()> {
        // User captures may have destructors; release the registry lock before dropping them.
        let _previous = self
            .app
            .update_function(self.index, |r| r.init.replace(hook))?;
        Ok(())
    }

    /// Executes the registered startup hook; worker runtimes call this before accepting jobs.
    ///
    /// # Errors
    /// Returns initialization or poisoned registration errors.
    pub async fn initialize(&self) -> Result<()> {
        if let Some(hook) = self.registration()?.init {
            hook.call(FunctionArgs::default()).await?;
        }
        Ok(())
    }

    /// Records a nonempty cron expression; deployment remains gated by backend schedule support.
    ///
    /// # Errors
    /// Rejects empty expressions or poisoned registration state.
    pub fn schedule(&self, cron: impl Into<String>) -> Result<()> {
        let cron = cron.into();
        if cron.is_empty() {
            return Err(Error::InvalidInput(
                "cron must be a non-empty string".into(),
            ));
        }
        self.app
            .update_function(self.index, |r| r.definition.spec.schedule = Some(cron))
    }

    /// Attaches explicit source symbol metadata for interoperable artifact manifests.
    ///
    /// # Errors
    /// Returns a poisoned registration error.
    pub fn set_symbol(&self, symbol: SymbolReference) -> Result<()> {
        self.app
            .update_function(self.index, |r| r.definition.symbol = Some(symbol))
    }

    /// Retains typed secret references while rendering worker environment values.
    ///
    /// # Errors
    /// Returns a poisoned registration error.
    pub fn set_environment(&self, env: &[(String, super::EnvValue)]) -> Result<()> {
        self.app
            .update_function(self.index, |r| r.definition.set_environment(env))
    }

    /// Executes the local Rust body, collecting generator output into an array.
    ///
    /// Use [`Self::local_stream`] for incremental consumption. Synchronous bodies run in Tokio's
    /// blocking pool, whose already-running calls finish even if this future is canceled.
    ///
    /// # Errors
    /// Returns local handler, background task, or poisoned registration errors.
    pub async fn local(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> Result<Value> {
        self.registration()?
            .body
            .call(FunctionArgs { args, kwargs })
            .await
    }

    /// Runs the local generator lazily, without initialization or remote resolution.
    pub fn local_stream(
        &self,
        args: Vec<Value>,
        kwargs: Map<String, Value>,
    ) -> BoxStream<'static, Result<Value>> {
        let handle = self.clone();
        Box::pin(async_stream::try_stream! {
            let function = handle.registration()?;
            let mut output = function.body.stream(FunctionArgs { args, kwargs });
            while let Some(value) = output.next().await { yield value?; }
        })
    }

    fn apply_options(&self, mut payload: Value) -> Result<Value> {
        let object = payload
            .as_object_mut()
            .ok_or_else(|| Error::InvalidInput("function payload must be an object".into()))?;
        if let Some(options) = self.options.as_object() {
            object.extend(options.clone());
        }
        Ok(payload)
    }

    /// Invokes the owning worker locally or resolves the resource's remote destination.
    ///
    /// # Errors
    /// Returns discovery, validation, resolution, handler, request, or execution errors.
    pub async fn remote(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> Result<Value> {
        let function = self.registration()?;
        let spec = &function.definition.spec;
        let context = self.app.context();
        context.guard_discovery(&spec.name)?;
        spec.validate()?;
        if context.kind == Context::Worker && context.is_current_worker(self.app.name(), &spec.name)
        {
            return function.body.call(FunctionArgs { args, kwargs }).await;
        }
        match self.app.resolve(&function, &context).await? {
            ResolvedTarget::Queue(target) => {
                let payload = target
                    .build_payload(&function.function, spec, &args, &kwargs)
                    .await?;
                target
                    .invoke(&self.apply_options(payload)?, DEFAULT_TIMEOUT)
                    .await
            }
            ResolvedTarget::Pod(target) => {
                target
                    .invoke(&target.build_payload(&args, &kwargs).await?, None)
                    .await
            }
        }
    }

    /// Submits once and returns either a reconnectable queue job or an owned task job.
    ///
    /// # Errors
    /// Returns discovery, validation, resolution, or submission errors.
    pub async fn spawn(&self, args: Vec<Value>, kwargs: Map<String, Value>) -> Result<FunctionJob> {
        let function = self.registration()?;
        let context = self.app.context();
        context.guard_discovery(&function.definition.spec.name)?;
        match self.app.resolve(&function, &context).await? {
            ResolvedTarget::Queue(target) => {
                let payload = target
                    .build_payload(
                        &function.function,
                        &function.definition.spec,
                        &args,
                        &kwargs,
                    )
                    .await?;
                Ok(FunctionJob {
                    inner: JobInner::Queue(Box::new(Job::new(
                        target.submit(&self.apply_options(payload)?).await?,
                        *target,
                    )?)),
                })
            }
            ResolvedTarget::Pod(target) => Ok(FunctionJob {
                inner: JobInner::Task(
                    target
                        .submit(&target.build_payload(&args, &kwargs).await?)
                        .await?,
                ),
            }),
        }
    }

    /// Reconnects a queue job by ID; dedicated-pod jobs must be retained from spawn.
    ///
    /// # Errors
    /// Returns invalid task usage or target-resolution errors.
    pub async fn job(&self, id: impl Into<String>) -> Result<Job> {
        let function = self.registration()?;
        if function.definition.spec.kind == ResourceKind::Task {
            return Err(AppError::InvalidResource(
                "task jobs cannot be reconnected by id; retain the job returned by .spawn()".into(),
            )
            .into());
        }
        match self.app.resolve(&function, &self.app.context()).await? {
            ResolvedTarget::Queue(target) => {
                Job::new(json!({"id":id.into(),"status":"UNKNOWN"}), *target)
            }
            ResolvedTarget::Pod(_) => Err(AppError::InvalidResource(
                "task jobs cannot be reconnected by id".into(),
            )
            .into()),
        }
    }

    /// Lazily invokes a queue generator, preserving local or remote chunk order.
    pub fn stream(
        &self,
        args: Vec<Value>,
        kwargs: Map<String, Value>,
    ) -> BoxStream<'static, Result<Value>> {
        let handle = self.clone();
        Box::pin(async_stream::try_stream! {
            let function = handle.registration()?;
            let spec = &function.definition.spec;
            let context = handle.app.context();
            context.guard_discovery(&spec.name)?;
            spec.validate()?;
            if spec.kind == ResourceKind::Task { Err(AppError::InvalidResource("tasks do not stream; use @app.queue for generator functions".into()))?; }
            if !function.body.is_streaming() { Err(AppError::InvalidResource(format!("'{}' is not a generator function; use .remote(...) instead", function.function.name)))?; }
            let mut chunks = if context.kind == Context::Worker && context.is_current_worker(handle.app.name(), &spec.name) {
                function.body.stream(FunctionArgs { args, kwargs })
            } else {
                match handle.app.resolve(&function, &context).await? {
                    ResolvedTarget::Queue(target) => {
                        let payload = target.build_payload(&function.function, spec, &args, &kwargs).await?;
                        let data = target.submit(&handle.apply_options(payload)?).await?;
                        let id = data.get("id").and_then(Value::as_str).ok_or_else(|| Error::InvalidResponse("submitted job response is missing a job id".into()))?;
                        target.stream_job(id, DEFAULT_TIMEOUT)
                    },
                    ResolvedTarget::Pod(_) => Err(AppError::InvalidResource("tasks do not stream".into()))?,
                }
            };
            while let Some(chunk) = chunks.next().await { yield chunk?; }
        })
    }
}

#[derive(Debug)]
enum JobInner {
    Queue(Box<Job>),
    Task(TaskJob),
}

/// Submission result owning either a reconnectable queue job or a dedicated pod job.
#[derive(Debug)]
pub struct FunctionJob {
    inner: JobInner,
}
impl FunctionJob {
    /// Borrows the queue job, if this submission used a queue resource.
    #[must_use]
    pub fn queue(&self) -> Option<&Job> {
        match &self.inner {
            JobInner::Queue(job) => Some(job),
            JobInner::Task(_) => None,
        }
    }
    /// Mutably borrows the queue job for status, retry, or output operations.
    pub fn queue_mut(&mut self) -> Option<&mut Job> {
        match &mut self.inner {
            JobInner::Queue(job) => Some(job),
            JobInner::Task(_) => None,
        }
    }
    /// Borrows the task job, if this submission owns a dedicated pod.
    #[must_use]
    pub fn task(&self) -> Option<&TaskJob> {
        match &self.inner {
            JobInner::Task(job) => Some(job),
            JobInner::Queue(_) => None,
        }
    }
    /// Mutably borrows the dedicated-pod job for wait or cancellation operations.
    pub fn task_mut(&mut self) -> Option<&mut TaskJob> {
        match &mut self.inner {
            JobInner::Task(job) => Some(job),
            JobInner::Queue(_) => None,
        }
    }
    /// Waits for output; absent timeout uses five minutes for queues and no limit for tasks.
    ///
    /// # Errors
    /// Returns remote execution, deadline, transport, or cleanup errors.
    pub async fn wait(&mut self, timeout: Option<std::time::Duration>) -> Result<Value> {
        match &mut self.inner {
            JobInner::Queue(job) => job.result(timeout.unwrap_or(DEFAULT_TIMEOUT)).await,
            JobInner::Task(job) => job.wait(timeout).await,
        }
    }
    /// Cancels a queue job or terminates the pod owned by a task job.
    ///
    /// # Errors
    /// Returns transport or cleanup errors.
    pub async fn cancel(&mut self) -> Result<()> {
        match &mut self.inner {
            JobInner::Queue(job) => {
                job.cancel().await?;
                Ok(())
            }
            JobInner::Task(job) => job.cancel().await,
        }
    }
}

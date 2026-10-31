use super::capture::{Capture, MAX_CAPTURED_CHARS, RingBuffer, clip, paused_sync};
use crate::{Error, Result, Value, json};
use futures_util::{FutureExt, future::BoxFuture};
use std::{fmt, future::Future, sync::Arc, time::Duration};

/// Named failure or deadline expiry in the startup hook sequence.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PrestartError {
    /// The complete phase exhausted its deadline while running this hook.
    #[error("prestart hook '{hook}' exceeded prestart_timeout of {timeout:?}")]
    Timeout {
        /// Name of the active hook.
        hook: String,
        /// Shared deadline for the complete phase.
        timeout: Duration,
    },
    /// A hook returned an error; later hooks were not run.
    #[error("prestart hook '{hook}' failed: {source}")]
    Failed {
        /// Name of the failing hook.
        hook: String,
        /// Original SDK or handler failure.
        #[source]
        source: Box<Error>,
    },
}

/// Build a bounded startup failure report with the original error, hook, and captured log tail.
/// Rust reports its native error type and a backtrace at the reporting boundary, not a Python traceback.
pub fn build_prestart_failed_payload(error: &PrestartError, logs: &str, worker_id: &str) -> Value {
    let (kind, message, hook) = match error {
        PrestartError::Failed { hook, source } => ("runpod::Error", source.to_string(), hook),
        PrestartError::Timeout { hook, .. } => ("PrestartTimeout", error.to_string(), hook),
    };
    let traceback = format!(
        "{kind}: {message}\n{}",
        std::backtrace::Backtrace::force_capture()
    );
    let mut payload = json!({
        "event":"prestart_failed", "error_type":kind,
        "error_message":clip(&message, MAX_CAPTURED_CHARS),
        "error_traceback":clip(&traceback, MAX_CAPTURED_CHARS),
        "worker_id":worker_id, "runpod_version":env!("CARGO_PKG_VERSION")
    });
    if !hook.is_empty() {
        payload["hook"] = json!(hook);
    }
    if !logs.is_empty() {
        let mut tail = RingBuffer::default();
        tail.write(logs);
        payload["logs"] = json!(tail.as_str());
    }
    payload
}

/// Build the reason returned to requests held when shutdown cancels startup.
pub fn build_prestart_cancelled_payload(worker_id: &str) -> Value {
    json!({"event":"prestart_cancelled", "error_type":"PrestartCancelled",
        "error_message":"worker shutdown began before prestart completed",
        "worker_id":worker_id, "runpod_version":env!("CARGO_PKG_VERSION")})
}

struct Hook {
    name: String,
    call: Box<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>,
}
/// Startup hooks owned by one worker, invoked in registration order.
#[derive(Default)]
pub struct PrestartHooks {
    hooks: Vec<Hook>,
}
impl fmt::Debug for PrestartHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrestartHooks")
            .field("names", &self.get_prestart_hooks())
            .finish()
    }
}
impl PrestartHooks {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }
    /// Registers a named asynchronous hook.
    pub fn register_prestart_hook<F, Fut>(&mut self, name: impl Into<String>, hook: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.hooks.push(Hook {
            name: name.into(),
            call: Box::new(move || hook().boxed()),
        });
    }
    /// Register blocking startup work on a dedicated thread, outside Tokio's blocking pool.
    /// A timeout or canceled phase stops waiting; the detached hook may continue until process exit.
    /// The thread does not delay runtime shutdown, matching Python's daemon-thread behavior.
    pub fn register_sync_prestart_hook(
        &mut self,
        name: impl Into<String>,
        hook: impl Fn() -> Result<()> + Send + Sync + 'static,
    ) {
        let hook = Arc::new(hook);
        self.register_prestart_hook(name, move || {
            let hook = hook.clone();
            async move {
                let (completed, result) = tokio::sync::oneshot::channel();
                let captured = Capture::current();
                let dispatch = tracing::dispatcher::get_default(Clone::clone);
                // A dedicated detached thread avoids Tokio joining a blocked hook during shutdown.
                std::thread::Builder::new()
                    .name("rp-prestart".into())
                    .spawn(move || {
                        let value =
                            tracing::dispatcher::with_default(&dispatch, || match captured {
                                Some(captured) => captured.scope_sync(|| hook()),
                                None => hook(),
                            });
                        let _ = completed.send(value);
                    })?;
                result.await.map_err(|_| {
                    Error::Io(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "prestart thread ended without returning its result",
                    ))
                })?
            }
        });
    }
    /// Returns names in registration order.
    pub fn get_prestart_hooks(&self) -> Vec<&str> {
        self.hooks.iter().map(|h| h.name.as_str()).collect()
    }
    /// Tests whether startup work is registered.
    pub fn has_prestart_hooks(&self) -> bool {
        !self.hooks.is_empty()
    }
    /// Clears registered hooks, normally for test isolation.
    pub fn clear_prestart_hooks(&mut self) {
        self.hooks.clear();
    }
    /// Runs all hooks within a shared optional deadline.
    ///
    /// # Errors
    /// Returns the first named hook failure or timeout. Dropping the future cancels it.
    pub async fn run_prestart_phase(
        &self,
        timeout: Option<Duration>,
    ) -> std::result::Result<(), PrestartError> {
        if self.hooks.is_empty() {
            return Ok(());
        }
        self.run_prestart_hooks_async(timeout).await
    }
    /// Execute registered hooks in order, emitting progress under one optional phase deadline.
    /// Unlike an empty phase, this explicit execution rejects a zero timeout even with no hooks.
    ///
    /// # Errors
    /// Returns a named hook failure or phase timeout. Dropping the future cancels pending async work.
    pub async fn run_prestart_hooks_async(
        &self,
        timeout: Option<Duration>,
    ) -> std::result::Result<(), PrestartError> {
        let started = tokio::time::Instant::now();
        paused_sync(|| tracing::info!("Prestart | phase started"));
        if timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(PrestartError::Timeout {
                hook: "unknown".into(),
                timeout: Duration::ZERO,
            });
        }
        for hook in &self.hooks {
            paused_sync(|| tracing::info!("Prestart | running hook: {}", hook.name));
            let invocation = async { (hook.call)().await };
            let result = if let Some(timeout) = timeout {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(PrestartError::Timeout {
                        hook: hook.name.clone(),
                        timeout,
                    });
                }
                tokio::time::timeout(remaining, invocation)
                    .await
                    .map_err(|_| PrestartError::Timeout {
                        hook: hook.name.clone(),
                        timeout,
                    })?
            } else {
                invocation.await
            };
            result.map_err(|source| PrestartError::Failed {
                hook: hook.name.clone(),
                source: Box::new(source),
            })?;
        }
        paused_sync(|| tracing::info!("Prestart | ready"));
        Ok(())
    }
}

use crate::{Error, Result};
use futures_util::{FutureExt, future::BoxFuture};
use std::{fmt, future::Future, time::Duration};

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
        let started = tokio::time::Instant::now();
        for hook in &self.hooks {
            let invocation = (hook.call)();
            let result = if let Some(timeout) = timeout {
                let remaining = timeout.saturating_sub(started.elapsed());
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
        Ok(())
    }
}

//! Execution context and discovery guards without mutable process-global SDK state.
use super::AppError;
use crate::Result;
use std::collections::BTreeMap;

/// Location in which an application invocation originates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Context {
    /// Ordinary application code on a developer machine.
    #[default]
    Local,
    /// A development session with explicitly provisioned targets.
    Dev,
    /// A serverless worker or dedicated pod.
    Worker,
}

/// Captured routing and discovery state; contains no credentials or arbitrary environment values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionContext {
    /// Worker detection takes precedence over a development session.
    pub kind: Context,
    /// Deployed environment override from `FLASH_ENVIRONMENT`.
    pub environment: Option<String>,
    /// Owning app when executing inside a development worker.
    pub dev_app: Option<String>,
    /// Resource executed by that development worker.
    pub dev_resource: Option<String>,
    /// `FLASH_RESOURCE_NAME`, falling back to `RUNPOD_RESOURCE_NAME`.
    pub resource_name: Option<String>,
    /// Whether declaration discovery forbids remote execution.
    pub discovery: bool,
}
impl ExecutionContext {
    /// Reads the relevant environment variables without modifying process state.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_values(
            [
                "RUNPOD_ENDPOINT_ID",
                "RUNPOD_POD_ID",
                "RUNPOD_DEV_SESSION",
                "FLASH_ENVIRONMENT",
                "RUNPOD_DEV_APP",
                "RUNPOD_DEV_RESOURCE",
                "FLASH_RESOURCE_NAME",
                "RUNPOD_RESOURCE_NAME",
                "RUNPOD_DISCOVERY_SCAN",
            ]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key, value))),
        )
    }

    /// Detects context from an explicit environment snapshot, treating empty values as absent.
    #[must_use]
    pub fn from_values<K: Into<String>, V: Into<String>>(
        values: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let values: BTreeMap<String, String> = values
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        let get = |key: &str| values.get(key).filter(|v| !v.is_empty()).cloned();
        Self {
            kind: if get("RUNPOD_ENDPOINT_ID")
                .or_else(|| get("RUNPOD_POD_ID"))
                .is_some()
            {
                Context::Worker
            } else if get("RUNPOD_DEV_SESSION").is_some() {
                Context::Dev
            } else {
                Context::Local
            },
            environment: get("FLASH_ENVIRONMENT"),
            dev_app: get("RUNPOD_DEV_APP"),
            dev_resource: get("RUNPOD_DEV_RESOURCE"),
            resource_name: get("FLASH_RESOURCE_NAME").or_else(|| get("RUNPOD_RESOURCE_NAME")),
            discovery: get("RUNPOD_DISCOVERY_SCAN").is_some(),
        }
    }

    /// Returns true for both ordinary local code and a development session.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.kind != Context::Worker
    }

    /// Determines whether a handle identifies the current worker's own function.
    #[must_use]
    pub fn is_current_worker(&self, app: &str, resource: &str) -> bool {
        if let Some(dev_app) = self.dev_app.as_deref().filter(|s| !s.is_empty()) {
            dev_app == app && self.dev_resource.as_deref() == Some(resource)
        } else {
            self.resource_name.as_deref() == Some(resource)
        }
    }

    pub(crate) fn guard_discovery(&self, resource: &str) -> Result<()> {
        if self.discovery {
            return Err(AppError::DiscoveryInvocation(resource.into()).into());
        }
        Ok(())
    }
}

/// Detects the current process context, giving workers precedence over development sessions.
#[must_use]
pub fn current_context() -> Context {
    ExecutionContext::from_env().kind
}

/// Returns whether this process is outside a Runpod worker or pod.
#[must_use]
pub fn is_local() -> bool {
    ExecutionContext::from_env().is_local()
}

/// Returns whether this process is collecting declarations under a discovery scan.
#[must_use]
pub fn in_discovery() -> bool {
    ExecutionContext::from_env().discovery
}

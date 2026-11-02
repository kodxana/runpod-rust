//! Selects deployed native resources using the worker manifest and mounted-volume bindings.
use super::{
    ApiHandle, AppRegistry, FunctionHandle, ResourceHandle, ResourceKind,
    bootstrap::{MANIFEST_NAME, PhaseError, read_manifest},
    volume::MountBindings,
};
use crate::{Error, Result, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const TASK_IDENTITY: [&str; 4] = [
    "FLASH_APP_NAME",
    "FLASH_ENVIRONMENT",
    "FLASH_RESOURCE_NAME",
    "RUNPOD_BUILD_ID",
];

/// Worker settings captured before native application declarations are constructed.
///
/// Supply a callback that maps a manifest module to compiled registrations. The callback
/// receives validated mounts and runs once per load. Return only resources exposed by
/// that module, in declaration order. Closures own bindings needed after loading.
#[derive(Debug, Clone)]
pub struct DeployedContext {
    app_dir: PathBuf,
    resource_name: String,
    mounts: MountBindings,
    runtime_kind: String,
    task_identity: [String; 4],
}

impl DeployedContext {
    /// Captures process worker settings and validates mount bindings before application loading.
    ///
    /// # Errors
    /// Returns malformed mount JSON or invalid binding errors.
    pub fn from_env() -> Result<Self> {
        let values = [
            "RUNPOD_APP_DIR",
            "RUNPOD_RESOURCE_NAME",
            "RUNPOD_MOUNTS",
            "RUNPOD_RUNTIME_KIND",
        ]
        .into_iter()
        .chain(TASK_IDENTITY)
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
        .collect();
        Self::from_values(&values)
    }

    /// Captures explicit worker settings without modifying process environment or global state.
    ///
    /// An absent `RUNPOD_MOUNTS` means no mounts; an empty value is invalid JSON.
    /// Task identity is validated by [`Self::load_task`] before reading the manifest.
    ///
    /// # Errors
    /// Returns malformed mount JSON with its original cause, or invalid binding errors.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self> {
        let get =
            |key: &str, default: &str| values.get(key).cloned().unwrap_or_else(|| default.into());
        let raw = values.get("RUNPOD_MOUNTS").map_or("[]", String::as_str);
        let mounts = serde_json::from_str(raw).map_err(|error| Error::Context {
            message: "RUNPOD_MOUNTS must contain a valid JSON array".into(),
            source: Box::new(Error::Json(error)),
        })?;
        Ok(Self {
            app_dir: get("RUNPOD_APP_DIR", "/app").into(),
            resource_name: values
                .get("FLASH_RESOURCE_NAME")
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| get("RUNPOD_RESOURCE_NAME", "")),
            mounts: MountBindings::from_value(&mounts)?,
            runtime_kind: get("RUNPOD_RUNTIME_KIND", "queue"),
            task_identity: TASK_IDENTITY.map(|key| get(key, "")),
        })
    }

    /// Returns the directory containing the deployed manifest.
    #[must_use]
    pub fn app_dir(&self) -> &Path {
        &self.app_dir
    }

    /// Returns the Flash resource name, falling back to the Runpod resource name when empty.
    #[must_use]
    pub fn resource_name(&self) -> &str {
        &self.resource_name
    }

    /// Returns the validated bindings available to application declarations and their handlers.
    #[must_use]
    pub fn mounts(&self) -> &MountBindings {
        &self.mounts
    }

    /// Detects deployed queue/API mode from a nonempty resource name and a manifest file.
    #[must_use]
    pub fn is_deployed(&self) -> bool {
        !self.resource_name.is_empty() && self.app_dir.join(MANIFEST_NAME).is_file()
    }

    /// Loads the first matching queue or task function without running its initialization hook.
    ///
    /// Pass the result to [`FunctionHandle::queue_worker`] or
    /// [`FunctionHandle::start_queue_worker`] to initialize it and serve jobs.
    /// As in the Python queue runtime, manifest kind, owner and qualified name are ignored.
    ///
    /// # Errors
    /// Returns manifest, module registration, missing handle, or poisoned registration errors.
    pub fn load_queue(
        &self,
        loader: impl FnOnce(&str, &MountBindings) -> Result<AppRegistry>,
    ) -> Result<FunctionHandle> {
        let (module, registry) = self.load_registry(loader)?;
        for app in registry.apps() {
            for resource in app.resources()? {
                if let ResourceHandle::Function(handle) = resource
                    && handle.spec()?.name == self.resource_name
                {
                    return Ok(handle);
                }
            }
        }
        Err(Error::InvalidInput(format!(
            "no @app.queue/@app.task handle named '{}' found in module '{module}'",
            self.resource_name
        )))
    }

    /// Loads the first matching API without running initialization or constructing its HTTP factory.
    ///
    /// Pass the result to [`ApiHandle::serve`] or [`ApiHandle::start`] to start serving.
    ///
    /// # Errors
    /// Returns manifest, module registration, missing handle, or poisoned registration errors.
    pub fn load_api(
        &self,
        loader: impl FnOnce(&str, &MountBindings) -> Result<AppRegistry>,
    ) -> Result<ApiHandle> {
        let (module, registry) = self.load_registry(loader)?;
        for app in registry.apps() {
            for resource in app.resources()? {
                if let ResourceHandle::Api(handle) = resource
                    && handle.spec()?.name == self.resource_name
                {
                    return Ok(handle);
                }
            }
        }
        Err(Error::InvalidInput(format!(
            "no @app.api handle named '{}' found in module '{module}'",
            self.resource_name
        )))
    }

    fn load_registry(
        &self,
        loader: impl FnOnce(&str, &MountBindings) -> Result<AppRegistry>,
    ) -> Result<(String, AppRegistry)> {
        let manifest = read_manifest(&self.app_dir)?;
        let resources = manifest
            .get("resources")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let entry = resources
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(&self.resource_name))
            .ok_or_else(|| {
                let names: Vec<_> = resources.iter().map(|entry| &entry["name"]).collect();
                Error::InvalidInput(format!(
                    "resource '{}' not in manifest (has: {})",
                    self.resource_name,
                    crate::json!(names)
                ))
            })?;
        let module = entry.get("module").and_then(Value::as_str).ok_or_else(|| {
            Error::InvalidInput("manifest resource requires a module string".into())
        })?;
        Ok((module.into(), loader(module, &self.mounts)?))
    }

    /// Validates deployed task identity, selects exactly one task, and awaits its initialization.
    ///
    /// The manifest app, resource kind and explicit qualified-name metadata must match.
    /// Aliases of the same application instance are counted once. Initialization uses the
    /// caller's Tokio runtime, so initialized async state remains usable for local execution.
    /// This loads a task handle; it does not start a task HTTP server.
    ///
    /// # Errors
    /// Returns identity/attachment phase errors, registration errors, an absent or ambiguous
    /// task error, or the original initialization failure. No hook runs on selection failure.
    pub async fn load_task(
        &self,
        loader: impl FnOnce(&str, &MountBindings) -> Result<AppRegistry>,
    ) -> Result<FunctionHandle> {
        if self.runtime_kind != "task" {
            return Err(PhaseError::new("identity", "RUNPOD_RUNTIME_KIND must be task").into());
        }
        for (key, value) in TASK_IDENTITY.iter().zip(&self.task_identity) {
            if value.trim().is_empty() {
                return Err(PhaseError::new(
                    "identity",
                    format!("{key} is required for deployed tasks"),
                )
                .into());
            }
        }
        let manifest = read_manifest(&self.app_dir)?;
        let (module, qualname) = self.task_symbol(&manifest)?;
        let registry = loader(module, &self.mounts)?;
        let mut selected = None;
        for (index, app) in registry.apps().iter().enumerate() {
            if app.name() != self.task_identity[0]
                || registry.apps()[..index]
                    .iter()
                    .any(|other| app.same_instance(other))
            {
                continue;
            }
            for resource in app.resources()? {
                let ResourceHandle::Function(handle) = resource else {
                    continue;
                };
                let definition = handle.registration()?.definition;
                if definition.spec.name == self.resource_name
                    && definition.spec.kind == ResourceKind::Task
                    && definition
                        .symbol
                        .as_ref()
                        .is_some_and(|symbol| symbol.qualname == qualname)
                {
                    if selected.is_some() {
                        return Err(task_handle_error());
                    }
                    selected = Some(handle);
                }
            }
        }
        let handle = selected.ok_or_else(task_handle_error)?;
        handle.initialize().await?;
        Ok(handle)
    }

    fn task_symbol<'a>(&self, manifest: &'a Value) -> Result<(&'a str, &'a str)> {
        let attach = |detail| PhaseError::new("attach", detail);
        if !manifest.is_object() {
            return Err(attach("manifest must be an object").into());
        }
        if manifest.get("app").and_then(Value::as_str) != Some(&self.task_identity[0]) {
            return Err(attach("manifest app does not match FLASH_APP_NAME").into());
        }
        let resources = manifest
            .get("resources")
            .and_then(Value::as_array)
            .filter(|resources| resources.iter().all(Value::is_object))
            .ok_or_else(|| attach("manifest resources must be an array of objects"))?;
        let mut matches = resources
            .iter()
            .filter(|entry| entry.get("name").and_then(Value::as_str) == Some(&self.resource_name));
        let entry = matches
            .next()
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("task"))
            .filter(|_| matches.next().is_none())
            .ok_or_else(|| attach("manifest must contain exactly one matching task"))?;
        let symbol = |key| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| attach("task module and qualname are required"))
        };
        Ok((symbol("module")?, symbol("qualname")?))
    }
}

fn task_handle_error() -> Error {
    Error::InvalidInput("module must expose exactly one matching @app.task handle".into())
}

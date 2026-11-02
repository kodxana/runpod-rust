use super::{BootstrapConfig, PhaseError, SystemPackageInstaller, read_manifest, report_error};
use crate::{
    Error, Result,
    apps::{
        AppRegistry,
        runtime::{self, DeployedContext, TASK_IDENTITY},
        task_runner,
    },
};
use std::{collections::BTreeMap, path::PathBuf};

fn settings() -> BTreeMap<String, String> {
    [
        "FLASH_BUILD_ARTIFACT_PATH",
        "RUNPOD_APP_DIR",
        "RUNPOD_PREBUILT_APP_DIR",
        "RUNPOD_ARTIFACT_WAIT",
        "RUNPOD_RESOURCE_NAME",
        "RUNPOD_MOUNTS",
        "RUNPOD_RUNTIME_KIND",
        "PORT",
    ]
    .into_iter()
    .chain(TASK_IDENTITY)
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
    .collect()
}
fn get<'a>(values: &'a BTreeMap<String, String>, key: &str, default: &'a str) -> &'a str {
    values.get(key).map_or(default, String::as_str)
}
fn resource_name(values: &BTreeMap<String, String>) -> &str {
    values
        .get("FLASH_RESOURCE_NAME")
        .filter(|name| !name.is_empty())
        .map_or_else(|| get(values, "RUNPOD_RESOURCE_NAME", ""), String::as_str)
}

async fn prepare(
    values: &BTreeMap<String, String>,
    installer: &SystemPackageInstaller,
) -> Result<PathBuf> {
    let task = get(values, "RUNPOD_RUNTIME_KIND", "queue") == "task";
    if task {
        runtime::validate_task_identity("task", TASK_IDENTITY.map(|key| get(values, key, "")))?;
    }
    let name = resource_name(values);
    if name.is_empty() {
        return Err(Error::InvalidInput(
            "deployed bootstrap requires FLASH_RESOURCE_NAME or RUNPOD_RESOURCE_NAME".into(),
        ));
    }
    let config = BootstrapConfig::from_values(values)?;
    let (app_dir, manifest) = tokio::task::spawn_blocking(move || -> Result<_> {
        let app_dir = config.locate()?;
        let manifest = read_manifest(&app_dir)?;
        Ok((app_dir, manifest))
    })
    .await??;
    if task {
        runtime::task_symbol(&manifest, get(values, "FLASH_APP_NAME", ""), name)?;
    }
    // Native crate dependencies are resolved when this worker binary is built. Runtime
    // Python import probes and pip repair belong to the Python-compatible artifact path.
    installer.install(&manifest, name).await?;
    Ok(app_dir)
}

/// Prepares a deployed native artifact and starts its registered queue, API or task worker.
///
/// Captures artifact/identity settings, validates task identity before extraction, locates the
/// app tree, reads its manifest and installs this resource's system dependencies. Compiled
/// dependencies require no Python environment or import probe. `loader` maps the manifest module
/// to native declarations using the resolved [`DeployedContext`], including its app directory and
/// validated mounts. Clone paths/bindings needed by handlers; the selected worker owns initialization.
/// Located paths are carried in an owned context without changing environment or current directory.
///
/// Bootstrap phase failures enter [`report_error`] before application loading. Other preparation
/// failures use the task bootstrap-error surface in task mode and propagate otherwise. Queue/API
/// load and initialization failures retain their existing worker behavior; task failures use its
/// authenticated diagnostics. Task deletion can exit the process after acknowledgement.
///
/// This entry point requires a deployed resource name. Live-source execution is a separate path.
/// Location/extraction runs on the blocking pool: cancellation prevents worker startup but cannot
/// stop already-running filesystem work. Cancellation kills an active package-manager child.
///
/// # Errors
/// Returns undeployed/unknown-kind configuration, package-manager, load, initialization, port,
/// network or serving errors. Phase failures are normally reported until the responder stops.
pub async fn start_deployed<F>(loader: F) -> Result<()>
where
    F: FnOnce(&str, &DeployedContext) -> Result<AppRegistry> + Send,
{
    let values = settings();
    let kind = get(&values, "RUNPOD_RUNTIME_KIND", "queue");
    let app_dir = match prepare(&values, &SystemPackageInstaller::default()).await {
        Ok(path) => path,
        Err(Error::Bootstrap(error)) => return report_error(&error).await,
        Err(error) if kind == "task" => {
            return report_error(&PhaseError::new("bootstrap", error.to_string())).await;
        }
        Err(error) => return Err(error),
    };
    match kind {
        "task" => {
            task_runner::start(async move {
                let context = DeployedContext::from_values(&values)?.with_app_dir(app_dir);
                context
                    .load_task(|module, _| loader(module, &context))
                    .await
            })
            .await
        }
        "api" => {
            let port = get(&values, "PORT", "80")
                .trim()
                .parse::<u16>()
                .map_err(|_| Error::InvalidInput("PORT must be a port number".into()))?;
            let context = DeployedContext::from_values(&values)?.with_app_dir(app_dir);
            context
                .load_api(|module, _| loader(module, &context))?
                .start("0.0.0.0", port)
                .await
        }
        "queue" => {
            let context = DeployedContext::from_values(&values)?.with_app_dir(app_dir);
            context
                .load_queue(|module, _| loader(module, &context))?
                .start_queue_worker()
                .await
        }
        _ => Err(Error::InvalidInput(format!(
            "unsupported RUNPOD_RUNTIME_KIND: {kind}"
        ))),
    }
}

#[cfg(test)]
mod tests;

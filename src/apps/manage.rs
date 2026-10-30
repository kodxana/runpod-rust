//! Application inspection and ordered endpoint/environment teardown.
use super::{
    AppError, AppsApiClient,
    events::{EventSink, emit},
};
use crate::{
    Error, Result, Value,
    client::{required_str, truthy},
    control::python_string,
    json,
};
use serde::{Deserialize, Serialize};

/// Determines whether endpoint teardown also removes its environment record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EnvironmentDisposition {
    /// Delete the environment only after every endpoint deletion succeeds.
    #[default]
    Delete,
    /// Retain the environment even after all endpoints have been removed.
    Keep,
}

/// Outcome of best-effort endpoint teardown and conditional parent deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndeployResult {
    /// Number of endpoint deletion calls that completed without an error.
    pub endpoints_deleted: usize,
    /// Whether this operation completed an environment deletion call.
    pub environment_deleted: bool,
    /// Whether this operation completed an application deletion call.
    pub app_deleted: bool,
    /// Ordered operation-qualified failure messages, matching the upstream result contract.
    pub failures: Vec<String>,
}

/// Lists applications by name, retaining server order for equal or absent names.
///
/// # Errors
/// Returns request errors or malformed non-string application names.
pub async fn list_apps(api: &AppsApiClient) -> Result<Vec<Value>> {
    let mut apps = api
        .list_apps()
        .await?
        .into_iter()
        .map(|app| {
            let name = match app.get("name").filter(|v| truthy(v)) {
                Some(name) => name.as_str().ok_or_else(|| {
                    Error::InvalidResponse("application name must be a string".into())
                })?,
                None => "",
            }
            .to_owned();
            Ok((name, app))
        })
        .collect::<Result<Vec<_>>>()?;
    apps.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(apps.into_iter().map(|(_, app)| app).collect())
}

/// Looks up a named application, distinguishing an absent app from transport failure.
///
/// # Errors
/// Returns [`AppError::AppNotFound`] or the underlying request/response error.
pub async fn get_app(app_name: &str, api: &AppsApiClient) -> Result<Value> {
    api.get_app_by_name(app_name)
        .await?
        .ok_or_else(|| AppError::AppNotFound(app_name.into()).into())
}

/// Looks up an environment, reporting absence even when its application is missing.
///
/// # Errors
/// Returns [`AppError::EnvironmentNotFound`] or the underlying request/response error.
pub async fn get_environment(app_name: &str, env_name: &str, api: &AppsApiClient) -> Result<Value> {
    api.get_environment_by_name(app_name, env_name)
        .await?
        .ok_or_else(|| {
            AppError::EnvironmentNotFound {
                app_name: app_name.into(),
                environment_name: env_name.into(),
            }
            .into()
        })
}

fn records<'a>(owner: &'a Value, key: &str) -> Result<&'a [Value]> {
    match owner.get(key).filter(|v| truthy(v)) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| Error::InvalidResponse(format!("{key} must be an array"))),
    }
}

/// Deletes every endpoint before optionally deleting its environment record.
///
/// Endpoint failures are collected while later endpoints are still attempted. Any endpoint
/// failure retains the environment. Completed mutations are not rolled back; canceling this
/// future stops subsequent requests. Observer errors follow [`EventSink`]'s suppression contract.
///
/// # Errors
/// Returns lookup or malformed endpoint-list errors; deletion errors are collected in the result.
pub async fn undeploy_environment(
    app_name: &str,
    env_name: &str,
    disposition: EnvironmentDisposition,
    api: &AppsApiClient,
    events: Option<&EventSink>,
) -> Result<UndeployResult> {
    let environment = get_environment(app_name, env_name, api).await?;
    let endpoints = records(&environment, "endpoints")?;
    let mut result = UndeployResult::default();
    emit(events, "cleanup_started", &[json!(endpoints.len())]);
    for endpoint in endpoints {
        let name = endpoint
            .get("name")
            .filter(|v| truthy(v))
            .or_else(|| endpoint.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
        emit(events, "deleting", std::slice::from_ref(&name));
        let deletion = async { api.delete_endpoint(required_str(endpoint, "id")?).await }.await;
        match deletion {
            Ok(_) => {
                result.endpoints_deleted += 1;
                emit(events, "deleted", std::slice::from_ref(&name));
                tracing::info!(endpoint=%python_string(&name), "deleted application endpoint");
            }
            Err(error) => {
                result
                    .failures
                    .push(format!("endpoint {}: {error}", python_string(&name)));
                emit(events, "delete_failed", std::slice::from_ref(&name));
            }
        }
    }
    if disposition == EnvironmentDisposition::Delete && result.failures.is_empty() {
        let deletion = async {
            api.delete_environment(required_str(&environment, "id")?)
                .await
        }
        .await;
        match deletion {
            Ok(_) => {
                result.environment_deleted = true;
                tracing::info!(environment = env_name, "deleted application environment");
            }
            Err(error) => result
                .failures
                .push(format!("environment {env_name}: {error}")),
        }
    }
    Ok(result)
}

/// Undeploys every environment before deleting the application if no child operation failed.
///
/// The result aggregates endpoint counts and failures; `environment_deleted` remains false
/// for this app-level operation, preserving the upstream aggregate result. Non-raising false
/// deletion confirmations count as completed calls, matching upstream behavior.
///
/// # Errors
/// Returns application/environment lookup errors or malformed environment metadata.
/// Individual deletion errors are collected in the result while later environments continue.
pub async fn delete_app(
    app_name: &str,
    api: &AppsApiClient,
    events: Option<&EventSink>,
) -> Result<UndeployResult> {
    let app = get_app(app_name, api).await?;
    let mut result = UndeployResult::default();
    for environment in records(&app, "flashEnvironments")? {
        let env_result = undeploy_environment(
            app_name,
            required_str(environment, "name")?,
            EnvironmentDisposition::Delete,
            api,
            events,
        )
        .await?;
        result.endpoints_deleted += env_result.endpoints_deleted;
        result.failures.extend(env_result.failures);
    }
    if result.failures.is_empty() {
        let deletion = async { api.delete_app(required_str(&app, "id")?).await }.await;
        match deletion {
            Ok(_) => {
                result.app_deleted = true;
                tracing::info!(app = app_name, "deleted application");
            }
            Err(error) => result.failures.push(format!("app {app_name}: {error}")),
        }
    }
    Ok(result)
}

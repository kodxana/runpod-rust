//! Deployment manifests and reconciliation of application endpoints.
use super::{
    AppError, AppsApiClient, ResourceKind, ResourceSpec, Selection,
    gpu::{PoolMembers, gpu_ids_value, needs_gpu_catalog, resolve_gpu_string},
    images::{DEFAULT_PYTHON_VERSION, RuntimeImages},
    spec::normalize_workers,
    volume::{VolumeResolver, attach_endpoint_volumes},
};
use crate::{Client, Error, Result, Value, client::required_str, control::python_string, json};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

/// Version understood by the pinned coordinator and application runtime.
pub const MANIFEST_VERSION: u32 = 1;

/// Explicit symbol metadata replacing runtime module and qualified-name reflection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolReference {
    /// Module/import path expected by the selected remote runtime.
    pub module: String,
    /// Qualified function, class, or factory name inside that module.
    pub qualname: String,
}

/// One ordered application resource and its optional runtime symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDefinition {
    /// Validated resource configuration consumed by the worker and control plane.
    pub spec: ResourceSpec,
    /// Symbol metadata is absent only for resources without an attached callable.
    pub symbol: Option<SymbolReference>,
}

/// An owned snapshot of the application metadata required for deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDefinition {
    /// Application routing name.
    pub name: String,
    /// Default environment for nested calls and deployment.
    pub environment: String,
    /// Resources in registration order, retained in the manifest.
    pub resources: Vec<ResourceDefinition>,
}
impl AppDefinition {
    /// Creates an empty application definition with an explicit default environment.
    ///
    /// # Errors
    /// Rejects an empty application name.
    pub fn new(name: impl Into<String>, environment: impl Into<String>) -> Result<Self> {
        let app = Self {
            name: name.into(),
            environment: environment.into(),
            resources: Vec::new(),
        };
        app.validate()?;
        Ok(app)
    }
    /// Validates every resource before any deployment operation starts.
    ///
    /// # Errors
    /// Returns invalid application or resource configuration.
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(
                AppError::InvalidResource("App name must be a non-empty string".into()).into(),
            );
        }
        for resource in &self.resources {
            resource.spec.validate()?;
        }
        Ok(())
    }
}

/// Runtime channels and worker credentials captured for one deployment.
#[derive(Clone)]
pub struct DeploymentConfig {
    /// Remote interpreter version used by the manifest and built-in runtime images.
    pub python_version: String,
    /// Optional `RUNPOD_RUNTIME_TAG`, `RUNPOD_RUNTIME_PACKAGE_SPEC`, and `RUNPOD_PACKAGE_SPEC`.
    /// Unknown keys are ignored; an empty tag still selects an empty image suffix.
    pub runtime_overrides: BTreeMap<String, String>,
    worker_api_key: String,
}
impl fmt::Debug for DeploymentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeploymentConfig")
            .field("python_version", &self.python_version)
            .field(
                "runtime_override_keys",
                &self.runtime_overrides.keys().collect::<Vec<_>>(),
            )
            .field("has_worker_api_key", &!self.worker_api_key.is_empty())
            .finish()
    }
}
const RUNTIME_OVERRIDES: &[&str] = &[
    "RUNPOD_RUNTIME_TAG",
    "RUNPOD_RUNTIME_PACKAGE_SPEC",
    "RUNPOD_PACKAGE_SPEC",
];
impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            python_version: DEFAULT_PYTHON_VERSION.into(),
            runtime_overrides: RUNTIME_OVERRIDES
                .iter()
                .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).into(), value)))
                .collect(),
            worker_api_key: String::new(),
        }
    }
}
impl DeploymentConfig {
    /// Captures runtime overrides and credentials injected for nested worker calls.
    #[must_use]
    pub fn new(worker_api_key: impl Into<String>) -> Self {
        Self {
            worker_api_key: worker_api_key.into(),
            ..Self::default()
        }
    }
    /// Uses the same account credentials as the control-plane client.
    #[must_use]
    pub fn from_client(client: &Client) -> Self {
        Self::new(client.config.api_key.clone())
    }
    fn images(&self) -> RuntimeImages {
        RuntimeImages {
            python_version: self.python_version.clone(),
            tag: self
                .runtime_overrides
                .get("RUNPOD_RUNTIME_TAG")
                .cloned()
                .unwrap_or_else(|| "latest".into()),
        }
    }
}

/// Identifiers and endpoint mappings produced by a completed application deployment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeployResult {
    /// Application routing name.
    pub app_name: String,
    /// Activated build identifier.
    pub build_id: String,
    /// Target environment identifier.
    pub environment_id: String,
    /// Resource names in manifest order, including tasks.
    pub resources: Vec<String>,
    /// Provisioned queue/API resource names mapped to their endpoint IDs.
    #[serde(default)]
    pub endpoints: BTreeMap<String, String>,
}

/// Constructs a coordinator manifest from ordered resource and symbol metadata.
///
/// # Errors
/// Returns validation errors, unsupported schedules, or queue/API names shorter than three characters.
pub fn build_manifest(
    app: &AppDefinition,
    python_version: &str,
    excluded_packages: &[String],
) -> Result<Value> {
    // Resource validation remains at serialization time so schedule/name errors retain precedence.
    if app.name.is_empty() {
        return Err(AppError::InvalidResource("App name must be a non-empty string".into()).into());
    }
    let mut resources = Vec::new();
    for resource in &app.resources {
        let spec = &resource.spec;
        if spec.schedule.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(AppError::ScheduleNotSupported.into());
        }
        if matches!(spec.kind, ResourceKind::Queue | ResourceKind::Api)
            && spec.name.chars().count() < 3
        {
            return Err(AppError::InvalidResource(format!("resource name '{}' is too short to deploy; queue and api names must be at least 3 characters",spec.name)).into());
        }
        let mut entry = spec.to_manifest()?;
        if let Some(symbol) = &resource.symbol {
            entry["module"] = json!(symbol.module);
            entry["qualname"] = json!(symbol.qualname);
        }
        resources.push(entry);
    }
    let mut manifest = json!({"version":MANIFEST_VERSION,"app":app.name,"pythonVersion":python_version,"resources":resources});
    if !excluded_packages.is_empty() {
        manifest["excludedPackages"] = json!(excluded_packages);
    }
    Ok(manifest)
}

fn normalized_gpu(spec: &ResourceSpec) -> Result<Vec<String>> {
    spec.gpu
        .as_ref()
        .map(Selection::values)
        .unwrap_or_default()
        .iter()
        .map(|s| resolve_gpu_string(s))
        .collect::<Result<Vec<_>>>()
        .map(|values| values.into_iter().flatten().collect())
}

/// Constructs the saveEndpoint payload for a deployed queue/API resource.
///
/// This is an offline formatter: missing live GPU membership uses the static catalog.
///
/// # Errors
/// Returns resource, location, or runtime image validation errors.
pub fn deployed_endpoint_input(
    app: &AppDefinition,
    spec: &ResourceSpec,
    environment_id: &str,
    build_id: &str,
    environment_name: Option<&str>,
    config: &DeploymentConfig,
    pool_members: Option<&PoolMembers>,
) -> Result<Value> {
    spec.validate()?;
    let mut env = serde_json::Map::new();
    env.insert("FLASH_RESOURCE_NAME".into(), json!(spec.name));
    env.insert("RUNPOD_BUILD_ID".into(), json!(build_id));
    if let Some(user_env) = &spec.env {
        env.extend(
            user_env
                .iter()
                .map(|(key, value)| (key.clone(), json!(python_string(value)))),
        );
    }
    env.insert(
        "FLASH_ENVIRONMENT".into(),
        json!(
            environment_name
                .filter(|s| !s.is_empty())
                .unwrap_or(&app.environment)
        ),
    );
    if !config.worker_api_key.is_empty() && !env.contains_key("RUNPOD_API_KEY") {
        env.insert("RUNPOD_API_KEY".into(), json!(config.worker_api_key));
    }
    for key in RUNTIME_OVERRIDES {
        if let Some(value) = config.runtime_overrides.get(*key).filter(|s| !s.is_empty()) {
            env.entry((*key).to_owned()).or_insert_with(|| json!(value));
        }
    }
    if spec.max_concurrency > 1 {
        env.insert(
            "RUNPOD_MAX_CONCURRENCY".into(),
            json!(spec.max_concurrency.to_string()),
        );
    }
    if spec.kind == ResourceKind::Api {
        env.insert("PORT".into(), json!("80"));
        env.insert("PORT_HEALTH".into(), json!("80"));
    }
    let workers = normalize_workers(spec.workers)?;
    let mut env = env
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect::<Vec<_>>();
    let command = if spec.image.as_ref().is_some_and(|s| !s.is_empty()) {
        env.push(json!({"key":"RUNPOD_RUNTIME_KIND","value":spec.kind}));
        super::shim::runtime_launcher(spec.kind)
    } else {
        String::new()
    };
    let mut payload = json!({"name":spec.name,"flashEnvironmentId":environment_id,"workersMin":workers[0],"workersMax":workers[1],
        "idleTimeout":spec.idle_timeout,"scalerType":spec.effective_scaler_type(),"scalerValue":spec.scaler_value,"executionTimeoutMs":spec.execution_timeout_ms,
        "template":{"name":format!("{}-{}-template",app.name,spec.name),"imageName":config.images().image_for_spec(spec)?,
        "containerDiskInGb":spec.container_disk_gb.unwrap_or(if spec.is_cpu(){10}else{30}),"dockerArgs":command,"env":env}});
    if spec.flashboot {
        payload["flashBootType"] = json!("FLASHBOOT");
    }
    if spec.kind == ResourceKind::Api {
        payload["type"] = json!("LB");
        payload["template"]["ports"] = json!("80/http");
    }
    if let Some(dc) = spec.datacenter.as_ref().filter(|s| !s.values().is_empty()) {
        payload["locations"] = json!(
            dc.values()
                .iter()
                .map(|s| s.parse::<super::DataCenter>().map(|dc| dc.to_string()))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        );
    }
    if spec.is_cpu() {
        let instances = spec.cpu.as_ref().map(Selection::values).unwrap_or_default();
        payload["instanceIds"] = json!(instances);
        if payload.get("locations").is_none() {
            payload["locations"] = json!(if instances.iter().any(|s| s.starts_with("cpu5")) {
                "EU-RO-1"
            } else {
                "EU-CZ-1,EU-RO-1,US-CA-2,US-IL-1,US-KS-2,US-MO-1,US-MO-2,US-NC-2,US-NE-1,US-WA-1"
            });
        }
    } else {
        payload["gpuIds"] = json!(gpu_ids_value(&normalized_gpu(spec)?, pool_members));
        payload["gpuCount"] = json!(spec.gpu_count);
        if let Some(version) = &spec.min_cuda_version {
            payload["minCudaVersion"] = json!(version.trim());
        }
        if payload.get("locations").is_none() {
            payload["locations"] = json!(
                super::DataCenter::all()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    Ok(payload)
}

/// Updates/creates queue and API endpoints, then deletes removed resources in environment order.
///
/// Tasks are excluded from endpoint provisioning. A failed save stops reconciliation before
/// obsolete endpoints are deleted; already completed saves are not rolled back.
///
/// # Errors
/// Returns validation, malformed environment, credential/storage, or control-plane failures.
pub async fn reconcile_endpoints(
    client: &AppsApiClient,
    app: &AppDefinition,
    environment: &Value,
    build_id: &str,
    config: &DeploymentConfig,
) -> Result<BTreeMap<String, String>> {
    app.validate()?;
    let environment_id = required_str(environment, "id")?;
    let environment_name = required_str(environment, "name")?;
    let mut existing: Vec<(&str, &str)> = Vec::new();
    if let Some(endpoints) = environment
        .get("endpoints")
        .filter(|v| crate::client::truthy(v))
    {
        for endpoint in endpoints.as_array().ok_or_else(|| {
            Error::InvalidResponse("environment endpoints must be an array".into())
        })? {
            let name = required_str(endpoint, "name")?;
            let id = required_str(endpoint, "id")?;
            if let Some((_, previous)) = existing.iter_mut().find(|(n, _)| *n == name) {
                *previous = id;
            } else {
                existing.push((name, id));
            }
        }
    }
    let provisionable: BTreeMap<_, _> = app
        .resources
        .iter()
        .filter(|r| matches!(r.spec.kind, ResourceKind::Queue | ResourceKind::Api))
        .map(|r| (r.spec.name.as_str(), &r.spec))
        .collect();
    let specs = app
        .resources
        .iter()
        .map(|r| r.spec.clone())
        .collect::<Vec<_>>();
    let mut resolver = VolumeResolver::new(client.clone());
    let mut endpoints = BTreeMap::new();
    for (name, spec) in &provisionable {
        let members = if needs_gpu_catalog(&normalized_gpu(spec)?) {
            client.gpu_pool_members().await
        } else {
            None
        };
        let mut payload = deployed_endpoint_input(
            app,
            spec,
            environment_id,
            build_id,
            Some(environment_name),
            config,
            members.as_ref(),
        )?;
        attach_endpoint_volumes(&mut payload, spec, &mut resolver, &specs).await?;
        if let Some(id) =
            super::registry::resolve_registry_auth(spec.registry_auth.as_deref(), client).await?
        {
            payload["template"]["containerRegistryAuthId"] = json!(id);
        }
        if let Some(model) = spec.model.as_ref().filter(|s| !s.is_empty()) {
            payload["modelReferences"] = json!([super::Model::new(model)?.reference()]);
        }
        if let Some((_, id)) = existing.iter().find(|(n, _)| n == name) {
            payload["id"] = json!(id);
        }
        let response = client.save_endpoint(&payload).await?;
        let id = required_str(&response, "id")?;
        tracing::info!(resource=%name,endpoint_id=%id,updated=payload.get("id").is_some(),"reconciled endpoint");
        endpoints.insert((*name).to_owned(), id.to_owned());
    }
    for (name, id) in existing {
        if !provisionable.contains_key(name) {
            client.delete_endpoint(id).await?;
            tracing::info!(resource=%name,endpoint_id=%id,"deleted removed endpoint");
        }
    }
    Ok(endpoints)
}

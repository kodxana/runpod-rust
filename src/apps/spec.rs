use super::{DataCenter, Model, gpu::resolve_gpu_string};
use crate::{Error, Result, Value, json};
use serde::{Deserialize, Serialize};

/// Platform execution mode of an application resource.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    /// A queue-based serverless function.
    #[default]
    Queue,
    /// A load-balanced HTTP service.
    Api,
    /// A pod provisioned per invocation.
    Task,
}
/// One selection or an ordered preference list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Selection {
    /// A single string.
    One(String),
    /// Multiple strings.
    Many(Vec<String>),
}
impl Selection {
    /// Borrows the selected values as a slice.
    pub fn values(&self) -> &[String] {
        match self {
            Self::One(v) => std::slice::from_ref(v),
            Self::Many(v) => v,
        }
    }
}
impl From<String> for Selection {
    fn from(value: String) -> Self {
        Self::One(value)
    }
}
impl From<&str> for Selection {
    fn from(value: &str) -> Self {
        Self::One(value.into())
    }
}
impl From<Vec<String>> for Selection {
    fn from(value: Vec<String>) -> Self {
        Self::Many(value)
    }
}

/// Worker capacity expressed as a maximum or minimum/maximum range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Workers {
    /// Scale between zero and this maximum.
    Max(i64),
    /// Explicit minimum and maximum.
    Range([i64; 2]),
}
/// Normalizes worker counts and validates the requested range.
///
/// # Errors
/// Returns invalid input for negative minima, zero maxima, or inverted ranges.
pub fn normalize_workers(workers: Option<Workers>) -> Result<[i64; 2]> {
    let [min, max] = match workers {
        None => [0, 3],
        Some(Workers::Max(max)) => [0, max],
        Some(Workers::Range(range)) => range,
    };
    if min < 0 || max < 1 {
        return Err(Error::InvalidInput(format!(
            "invalid worker range ({min}, {max})"
        )));
    }
    if min > max {
        return Err(Error::InvalidInput(format!(
            "workers min ({min}) cannot exceed max ({max})"
        )));
    }
    Ok([min, max])
}

/// HTTP route metadata attached to an API resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSpec {
    /// Uppercase HTTP method.
    pub method: String,
    /// Absolute route path.
    pub path: String,
    /// Name used to identify the handler in the resource.
    pub handler_name: String,
}

/// Validates user routes against the runtime's reserved method/path space.
///
/// # Errors
/// Returns invalid input for unsupported methods, relative paths, or reserved paths.
pub fn validate_route(method: &str, path: &str) -> Result<()> {
    if !["GET", "POST", "PUT", "DELETE", "PATCH"].contains(&method) {
        return Err(Error::InvalidInput(
            "method must be one of ['DELETE', 'GET', 'PATCH', 'POST', 'PUT']".into(),
        ));
    }
    if !path.starts_with('/') {
        return Err(Error::InvalidInput(format!(
            "path must start with '/', got: '{path}'"
        )));
    }
    if ["/execute", "/ping", "/_runpod/sync"].contains(&path) {
        return Err(Error::InvalidInput(format!(
            "path '{path}' is reserved by the worker runtime (reserved: /_runpod/sync, /execute, /ping)"
        )));
    }
    Ok(())
}

/// Declarative resource settings serialized into an application deployment manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceSpec {
    /// Explicit worker mount paths and volume references.
    pub mounts: Vec<super::volume::Mount>,
    /// Execution mode.
    pub kind: ResourceKind,
    /// Nonempty resource name.
    pub name: String,
    /// GPU pool or device selection.
    pub gpu: Option<Selection>,
    /// CPU instance selection, mutually exclusive with GPUs.
    pub cpu: Option<Selection>,
    /// Number of GPUs per worker.
    pub gpu_count: i64,
    /// Worker range; absent defaults to (0, 3).
    pub workers: Option<Workers>,
    /// Worker idle timeout in seconds.
    pub idle_timeout: i64,
    /// Application dependencies retained in the manifest.
    pub dependencies: Option<Vec<String>>,
    /// Operating-system packages.
    pub system_dependencies: Option<Vec<String>>,
    /// Environment values, including pre-rendered secret references.
    pub env: Option<serde_json::Map<String, Value>>,
    /// Datacenter selection.
    pub datacenter: Option<Selection>,
    /// Custom container image.
    pub image: Option<String>,
    /// Container registry credential reference.
    pub registry_auth: Option<String>,
    /// Cached model reference.
    pub model: Option<String>,
    /// Optional cron expression; backend execution is separately gated.
    pub schedule: Option<String>,
    /// Maximum concurrent requests per worker.
    pub max_concurrency: i64,
    /// Job execution timeout in milliseconds.
    pub execution_timeout_ms: i64,
    /// Enable FlashBoot.
    pub flashboot: bool,
    /// Explicit scaling mode or resource-kind default.
    pub scaler_type: Option<String>,
    /// Scaling threshold.
    pub scaler_value: i64,
    /// Minimum CUDA version supported by the host.
    pub min_cuda_version: Option<String>,
    /// Accelerate model downloads.
    pub accelerate_downloads: bool,
    /// Container disk size in GB.
    pub container_disk_gb: Option<i64>,
    /// HTTP routes on API resources.
    pub routes: Vec<RouteSpec>,
    /// Optional ASGI factory metadata for interoperating with Python manifests.
    pub asgi_factory: Option<String>,
}
impl Default for ResourceSpec {
    fn default() -> Self {
        Self {
            mounts: Vec::new(),
            kind: ResourceKind::Queue,
            name: String::new(),
            gpu: None,
            cpu: None,
            gpu_count: 1,
            workers: None,
            idle_timeout: 60,
            dependencies: None,
            system_dependencies: None,
            env: None,
            datacenter: None,
            image: None,
            registry_auth: None,
            model: None,
            schedule: None,
            max_concurrency: 1,
            execution_timeout_ms: 0,
            flashboot: true,
            scaler_type: None,
            scaler_value: 4,
            min_cuda_version: None,
            accelerate_downloads: true,
            container_disk_gb: None,
            routes: Vec::new(),
            asgi_factory: None,
        }
    }
}
impl ResourceSpec {
    /// Creates a resource with upstream defaults.
    pub fn new(kind: ResourceKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            ..Self::default()
        }
    }
    /// Returns whether this resource uses CPU instances.
    pub fn is_cpu(&self) -> bool {
        self.cpu.is_some()
    }
    /// Returns the selected scaler or the resource-kind default.
    pub fn effective_scaler_type(&self) -> String {
        self.scaler_type
            .as_deref()
            .map(|s| s.trim().to_uppercase())
            .unwrap_or_else(|| {
                if self.kind == ResourceKind::Api {
                    "REQUEST_COUNT".into()
                } else {
                    "QUEUE_DELAY".into()
                }
            })
    }
    /// Validates mutable resource configuration without provisioning anything.
    ///
    /// # Errors
    /// Returns invalid input for conflicting or unsupported configuration.
    pub fn validate(&self) -> Result<()> {
        super::volume::validate_mounts(
            &self.mounts,
            match self.kind {
                ResourceKind::Queue => "queue",
                ResourceKind::Api => "api",
                ResourceKind::Task => "task",
            },
            if self.is_cpu() {
                super::volume::ComputeType::Cpu
            } else {
                super::volume::ComputeType::Gpu
            },
        )?;
        normalize_workers(self.workers)?;
        if let Some(gpu) = &self.gpu {
            for value in gpu.values() {
                resolve_gpu_string(value)?;
            }
        }
        if let Some(cpu) = &self.cpu
            && (cpu.values().is_empty() || cpu.values().iter().any(|s| s.trim().is_empty()))
        {
            return Err(Error::InvalidInput(
                "cpu must contain non-empty instance ids".into(),
            ));
        }
        if !["QUEUE_DELAY", "REQUEST_COUNT"].contains(&self.effective_scaler_type().as_str()) {
            return Err(Error::InvalidInput(
                "scaler_type must be QUEUE_DELAY or REQUEST_COUNT".into(),
            ));
        }
        if let Some(version) = &self.min_cuda_version
            && ![
                "11.8", "12.0", "12.1", "12.2", "12.3", "12.4", "12.5", "12.6", "12.7", "12.8",
                "12.9", "13.0",
            ]
            .contains(&version.trim())
        {
            return Err(Error::InvalidInput(format!(
                "invalid CUDA version {version}"
            )));
        }
        if let Some(dc) = &self.datacenter {
            for value in dc.values() {
                value.parse::<DataCenter>()?;
            }
        }
        if self.gpu.is_some() && self.cpu.is_some() {
            return Err(Error::InvalidInput(format!(
                "resource '{}': gpu and cpu are mutually exclusive",
                self.name
            )));
        }
        if self.name.is_empty() {
            return Err(Error::InvalidInput(
                "resource name must be a non-empty string".into(),
            ));
        }
        let invalid =
            |message: &str| Error::InvalidInput(format!("resource '{}': {message}", self.name));
        if self.gpu_count < 1 {
            return Err(invalid("gpu_count must be >= 1"));
        }
        if self.max_concurrency < 1 {
            return Err(invalid(&format!(
                "max_concurrency must be >= 1, got {}",
                self.max_concurrency
            )));
        }
        if self.execution_timeout_ms < 0 {
            return Err(invalid("execution_timeout_ms must be >= 0"));
        }
        if self.scaler_value < 1 {
            return Err(invalid("scaler_value must be >= 1"));
        }
        if self.container_disk_gb.is_some_and(|v| v < 1) {
            return Err(invalid("container_disk_gb must be >= 1"));
        }
        if self.min_cuda_version.is_some() && self.is_cpu() {
            return Err(invalid("min_cuda_version has no effect on cpu resources"));
        }
        if self.model.is_some() && self.kind == ResourceKind::Task {
            return Err(invalid(
                "platform-cached models are only available on queue and api resources; tasks download weights themselves",
            ));
        }
        if self.schedule.as_ref().is_some_and(String::is_empty) {
            return Err(Error::InvalidInput(
                "cron must be a non-empty string".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for route in &self.routes {
            validate_route(&route.method, &route.path)?;
            if !seen.insert((&route.method, &route.path)) {
                return Err(Error::InvalidInput(format!(
                    "duplicate route {} {} on {}",
                    route.method, route.path, self.name
                )));
            }
        }
        Ok(())
    }
    /// Serializes normalized settings using the Python deployment manifest field names.
    ///
    /// # Errors
    /// Returns validation or model-reference errors.
    pub fn to_manifest(&self) -> Result<Value> {
        self.validate()?;
        let workers = normalize_workers(self.workers)?;
        let mut data = json!({"kind":self.kind,"name":self.name,"gpuCount":self.gpu_count,"workersMin":workers[0],"workersMax":workers[1],"idleTimeout":self.idle_timeout});
        if !self.mounts.is_empty() {
            data["mounts"]=json!(super::volume::normalize_mounts(&self.mounts)?.iter().map(|m|json!({"kind":m.volume.kind(),"reference":m.volume.reference(),"path":m.path})).collect::<Vec<_>>());
        }
        if let Some(gpu) = &self.gpu {
            let mut values = Vec::new();
            for value in gpu.values() {
                values.extend(resolve_gpu_string(value)?);
            }
            data["gpus"] = json!(values);
        }
        if let Some(cpu) = &self.cpu {
            data["instanceIds"] = json!(cpu.values());
        }
        for (key, value) in [
            ("dependencies", &self.dependencies),
            ("systemDependencies", &self.system_dependencies),
        ] {
            if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
                data[key] = json!(value);
            }
        }
        if let Some(env) = self.env.as_ref().filter(|v| !v.is_empty()) {
            data["env"] = super::super::control::environment(env);
        }
        if let Some(dc) = self.datacenter.as_ref().filter(|v| !v.values().is_empty()) {
            data["locations"] = json!(
                dc.values()
                    .iter()
                    .map(|v| v.parse::<DataCenter>().map(|d| d.to_string()))
                    .collect::<Result<Vec<_>>>()?
                    .join(",")
            );
        }
        for (key, value) in [
            ("imageName", &self.image),
            ("registryAuth", &self.registry_auth),
            ("schedule", &self.schedule),
            ("asgiFactory", &self.asgi_factory),
        ] {
            if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
                data[key] = json!(value);
            }
        }
        if let Some(model) = self.model.as_ref().filter(|m| !m.is_empty()) {
            data["model"] = json!(Model::new(model)?.reference());
        }
        if self.max_concurrency != 1 {
            data["maxConcurrency"] = json!(self.max_concurrency);
        }
        if self.execution_timeout_ms != 0 {
            data["executionTimeoutMs"] = json!(self.execution_timeout_ms);
        }
        if !self.flashboot {
            data["flashboot"] = json!(false);
        }
        if self.scaler_type.is_some() {
            data["scalerType"] = json!(self.effective_scaler_type());
        }
        if self.scaler_value != 4 {
            data["scalerValue"] = json!(self.scaler_value);
        }
        if let Some(version) = &self.min_cuda_version {
            data["minCudaVersion"] = json!(version.trim());
        }
        if !self.accelerate_downloads {
            data["accelerateDownloads"] = json!(false);
        }
        if let Some(size) = self.container_disk_gb {
            data["containerDiskGb"] = json!(size);
        }
        if !self.routes.is_empty() {
            data["routes"] = json!(
                self.routes
                    .iter()
                    .map(|r| json!({"method":r.method,"path":r.path,"handler":r.handler_name}))
                    .collect::<Vec<_>>()
            );
        }
        Ok(data)
    }
}

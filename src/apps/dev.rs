//! Development endpoint configuration and owned refresh/cleanup sessions.
mod session;
use super::{
    ResourceKind, ResourceSpec, Selection,
    gpu::{PoolMembers, gpu_ids_value, resolve_gpu_string},
    images::RuntimeImages,
};
use crate::{Client, Error, Result, Value, client::truthy, control::python_string, json};
pub use session::DevSession;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// Changing this worker environment value recreates workers after a source refresh.
pub const GENERATION_ENV: &str = "RUNPOD_DEV_GENERATION";

/// Captured remote runtime configuration; formatting payloads does not need credentials.
#[derive(Clone)]
pub struct DevConfig {
    /// Explicit interpreter/channel selection for Python-compatible live worker images.
    pub images: RuntimeImages,
    /// Optional `RUNPOD_DEV_IMAGE`; custom resource images take precedence.
    pub image_override: Option<String>,
    /// Custom-image package overrides. Only the two upstream package-spec keys are used.
    pub runtime_overrides: BTreeMap<String, String>,
    worker_api_key: String,
}
impl fmt::Debug for DevConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevConfig")
            .field("images", &self.images)
            .field("has_image_override", &self.image_override.is_some())
            .field(
                "runtime_override_keys",
                &self.runtime_overrides.keys().collect::<Vec<_>>(),
            )
            .field("has_worker_api_key", &!self.worker_api_key.is_empty())
            .finish()
    }
}
const PACKAGE_KEYS: &[&str] = &["RUNPOD_RUNTIME_PACKAGE_SPEC", "RUNPOD_PACKAGE_SPEC"];
impl Default for DevConfig {
    fn default() -> Self {
        Self {
            images: RuntimeImages::default(),
            image_override: std::env::var("RUNPOD_DEV_IMAGE").ok(),
            runtime_overrides: PACKAGE_KEYS
                .iter()
                .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).into(), value)))
                .collect(),
            worker_api_key: String::new(),
        }
    }
}
impl DevConfig {
    /// Captures runtime environment overrides and a key for sibling worker invocations.
    #[must_use]
    pub fn new(worker_api_key: impl Into<String>) -> Self {
        Self {
            worker_api_key: worker_api_key.into(),
            ..Self::default()
        }
    }
    /// Uses the control-plane client's account key for nested remote calls.
    #[must_use]
    pub fn from_client(client: &Client) -> Self {
        Self::new(client.config.api_key.clone())
    }
    fn image_for(&self, spec: &ResourceSpec) -> Result<String> {
        if let Some(image) = spec.image.as_ref().filter(|s| !s.is_empty()) {
            return Ok(image.clone());
        }
        if let Some(image) = self.image_override.as_ref().filter(|s| !s.is_empty()) {
            return Ok(image.clone());
        }
        self.images.image_for_spec(spec)
    }
}

fn gpu_selection(spec: &ResourceSpec) -> Result<Vec<String>> {
    spec.gpu
        .as_ref()
        .map(Selection::values)
        .unwrap_or_default()
        .iter()
        .map(|s| resolve_gpu_string(s))
        .collect::<Result<Vec<_>>>()
        .map(|v| v.into_iter().flatten().collect())
}

/// Builds a live endpoint's atomic container configuration without network calls.
///
/// Reserved environment entries retain upstream ordering and duplicate-key precedence.
/// Images execute explicit Python source metadata; native Rust binary deployment is separate.
///
/// # Errors
/// Returns invalid resource, GPU, datacenter, or runtime version errors.
pub fn endpoint_input(
    app_name: &str,
    spec: &ResourceSpec,
    generation: u64,
    config: &DevConfig,
    pool_members: Option<&PoolMembers>,
) -> Result<Value> {
    spec.validate()?;
    let mut resource_env = serde_json::Map::new();
    if let Some(env) = &spec.env {
        resource_env.extend(
            env.iter()
                .map(|(k, v)| (k.clone(), json!(python_string(v)))),
        );
    }
    resource_env.insert("RUNPOD_DEV_RESOURCE".into(), json!(spec.name));
    if spec.kind == ResourceKind::Api {
        resource_env.insert("PORT".into(), json!("80"));
        resource_env.insert("PORT_HEALTH".into(), json!("80"));
    }
    let mut env = vec![json!({"key":GENERATION_ENV,"value":generation.to_string()})];
    if !config.worker_api_key.is_empty() {
        env.push(json!({"key":"RUNPOD_API_KEY","value":config.worker_api_key}));
    }
    env.push(json!({"key":"RUNPOD_DEV_APP","value":app_name}));
    if spec.max_concurrency > 1 {
        env.push(json!({"key":"RUNPOD_MAX_CONCURRENCY","value":spec.max_concurrency.to_string()}));
    }
    env.extend(
        resource_env
            .into_iter()
            .map(|(key, value)| json!({"key":key,"value":value})),
    );
    let command = if spec.image.as_ref().is_some_and(|s| !s.is_empty()) {
        env.push(json!({"key":"RUNPOD_RUNTIME_KIND","value":spec.kind}));
        for key in PACKAGE_KEYS {
            if let Some(value) = config.runtime_overrides.get(*key).filter(|s| !s.is_empty()) {
                env.push(json!({"key":key,"value":value}));
            }
        }
        super::shim::runtime_launcher(spec.kind)
    } else {
        String::new()
    };
    let workers = super::spec::normalize_workers(spec.workers)?;
    let disk = spec
        .container_disk_gb
        .filter(|v| *v != 0)
        .unwrap_or(if spec.is_cpu() { 10 } else { 30 });
    let mut payload = json!({"name":super::dev_endpoint_name(app_name,&spec.name),"workersMin":workers[0],"workersMax":workers[1],
        "idleTimeout":spec.idle_timeout,"scalerType":spec.effective_scaler_type(),"scalerValue":spec.scaler_value,"executionTimeoutMs":spec.execution_timeout_ms,
        "template":{"imageName":config.image_for(spec)?,"containerDiskInGb":disk,"dockerArgs":command,"env":env}});
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
        payload["gpuIds"] = json!(gpu_ids_value(&gpu_selection(spec)?, pool_members));
        payload["gpuCount"] = json!(spec.gpu_count);
        if let Some(version) = spec.min_cuda_version.as_ref().filter(|s| !s.is_empty()) {
            payload["minCudaVersion"] = json!(version.trim());
        }
    }
    Ok(payload)
}

/// Clones a payload, omitting identity and generation changes from configuration comparisons.
///
/// # Errors
/// Returns invalid input for malformed payload, template, or environment structures.
pub fn comparable_payload(payload: &Value) -> Result<Value> {
    let mut payload = payload.clone();
    let object = payload
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("endpoint payload must be an object".into()))?;
    object.remove("id");
    if let Some(template) = object.get_mut("template").filter(|v| truthy(v)) {
        let template = template
            .as_object_mut()
            .ok_or_else(|| Error::InvalidInput("template must be an object".into()))?;
        let env = match template.get("env").filter(|v| truthy(v)) {
            Some(env) => env
                .as_array()
                .ok_or_else(|| Error::InvalidInput("template env must be an array".into()))?
                .iter()
                .filter(|v| v["key"] != GENERATION_ENV)
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        template.insert("env".into(), json!(env));
    }
    Ok(payload)
}

// Python compares booleans and integers/floats numerically, including nested JSON values.
// The guarded float-to-i128 conversions are exact for integral values in JSON's i64/u64
// range. Out-of-range floats saturate outside that range and cannot compare equal.
#[allow(clippy::cast_possible_truncation)]
fn equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let integer = |v: &serde_json::Number| {
                v.as_i64()
                    .map(i128::from)
                    .or_else(|| v.as_u64().map(i128::from))
            };
            match (integer(a), integer(b)) {
                (Some(a), Some(b)) => a == b,
                (Some(i), None) => b
                    .as_f64()
                    .is_some_and(|f| f.fract() == 0.0 && f as i128 == i),
                (None, Some(i)) => a
                    .as_f64()
                    .is_some_and(|f| f.fract() == 0.0 && f as i128 == i),
                (None, None) => a == b,
            }
        }
        (Value::Bool(a), Value::Number(b)) | (Value::Number(b), Value::Bool(a)) => {
            b.as_f64() == Some(f64::from(u8::from(*a)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| equivalent(a, b)))
        }
        _ => a == b,
    }
}

/// Lists changed fields in sorted order, flattening template keys and naming `imageName` image.
///
/// Missing and null fields compare equally, as do Python-compatible numeric JSON values.
/// # Errors
/// Returns invalid input for non-object payloads or truthy non-object templates.
pub fn changed_fields(old: &Value, new: &Value) -> Result<Vec<String>> {
    fn objects(value: &Value) -> Result<&serde_json::Map<String, Value>> {
        value
            .as_object()
            .ok_or_else(|| Error::InvalidInput("configuration must be an object".into()))
    }
    let (old, new) = (objects(old)?, objects(new)?);
    let mut fields = Vec::new();
    for key in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
        let a = old.get(key).unwrap_or(&Value::Null);
        let b = new.get(key).unwrap_or(&Value::Null);
        if equivalent(a, b) {
            continue;
        }
        if key == "template" {
            let empty = serde_json::Map::new();
            let a = if truthy(a) { objects(a)? } else { &empty };
            let b = if truthy(b) { objects(b)? } else { &empty };
            for sub in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                if !equivalent(
                    a.get(sub).unwrap_or(&Value::Null),
                    b.get(sub).unwrap_or(&Value::Null),
                ) {
                    fields.push(if sub == "imageName" {
                        "image".into()
                    } else {
                        sub.clone()
                    });
                }
            }
        } else {
            fields.push(key.clone());
        }
    }
    Ok(fields)
}

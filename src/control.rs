//! Control-plane operations corresponding to every Python `ctl_commands` function.
use crate::{Client, Error, Result, Value, client::segment, json};
use serde::{Deserialize, Serialize};

/// GPU or CPU pod creation parameters, retaining the Python SDK's names and defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PodOptions {
    /// Display name.
    pub name: String,
    /// Container image; may be empty when a template is supplied.
    pub image_name: String,
    /// GPU device ID; absence selects a CPU pod.
    pub gpu_type_id: Option<String>,
    /// ALL, COMMUNITY, or SECURE.
    pub cloud_type: String,
    /// Legacy option; true is rejected because REST v2 cannot represent it.
    pub support_public_ip: bool,
    /// Enable SSH access.
    pub start_ssh: bool,
    /// Datacenter identifier.
    pub data_center_id: Option<String>,
    /// Legacy country constraint, rejected by REST v2.
    pub country_code: Option<String>,
    /// Number of GPUs.
    pub gpu_count: u64,
    /// Persistent disk size in GB, zero to inherit or omit.
    pub volume_in_gb: u64,
    /// Container disk size; absence inherits a template or defaults to 10 GB.
    pub container_disk_in_gb: Option<u64>,
    /// Minimum virtual CPUs per GPU.
    pub min_vcpu_count: u64,
    /// Minimum memory in GB per GPU.
    pub min_memory_in_gb: u64,
    /// Container command override; an empty string clears an inherited command.
    pub docker_args: Option<String>,
    /// Comma-separated ports.
    pub ports: Option<String>,
    /// Storage mount override.
    pub volume_mount_path: Option<String>,
    /// Environment values converted to Python-compatible strings.
    pub env: Option<serde_json::Map<String, Value>>,
    /// Template to inherit from.
    pub template_id: Option<String>,
    /// Existing network volume to mount.
    pub network_volume_id: Option<String>,
    /// Accepted CUDA versions.
    pub allowed_cuda_versions: Option<Vec<String>>,
    /// Legacy bandwidth constraint, rejected by REST v2.
    pub min_download: Option<f64>,
    /// Legacy bandwidth constraint, rejected by REST v2.
    pub min_upload: Option<f64>,
    /// CPU instance in flavor-vcpu-memory form.
    pub instance_id: Option<String>,
}
impl Default for PodOptions {
    fn default() -> Self {
        Self {
            name: String::new(),
            image_name: String::new(),
            gpu_type_id: None,
            cloud_type: "ALL".into(),
            support_public_ip: false,
            start_ssh: true,
            data_center_id: None,
            country_code: None,
            gpu_count: 1,
            volume_in_gb: 0,
            container_disk_in_gb: None,
            min_vcpu_count: 1,
            min_memory_in_gb: 1,
            docker_args: None,
            ports: None,
            volume_mount_path: None,
            env: None,
            template_id: None,
            network_volume_id: None,
            allowed_cuda_versions: None,
            min_download: None,
            min_upload: None,
            instance_id: None,
        }
    }
}

/// Pod or serverless template creation parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateOptions {
    /// Display name.
    pub name: String,
    /// Container image.
    pub image_name: String,
    /// Optional command override.
    pub docker_start_cmd: Option<String>,
    /// Container disk in GB.
    pub container_disk_in_gb: u64,
    /// Optional persistent disk in GB.
    pub volume_in_gb: Option<u64>,
    /// Persistent mount point, default `/workspace`.
    pub volume_mount_path: Option<String>,
    /// Comma-separated port specifications.
    pub ports: Option<String>,
    /// Container environment.
    pub env: Option<serde_json::Map<String, Value>>,
    /// Whether this template targets serverless workers.
    pub is_serverless: bool,
    /// Registry credential ID.
    pub registry_auth_id: Option<String>,
}
impl Default for TemplateOptions {
    fn default() -> Self {
        Self {
            name: String::new(),
            image_name: String::new(),
            docker_start_cmd: None,
            container_disk_in_gb: 10,
            volume_in_gb: None,
            volume_mount_path: None,
            ports: None,
            env: None,
            is_serverless: false,
            registry_auth_id: None,
        }
    }
}

/// Queue-based serverless endpoint configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EndpointOptions {
    /// Display name.
    pub name: String,
    /// Worker template ID.
    pub template_id: String,
    /// Comma-separated pools; prefix device IDs with `-` to exclude them.
    pub gpu_ids: String,
    /// Existing network volume.
    pub network_volume_id: Option<String>,
    /// Comma-separated datacenter IDs.
    pub locations: Option<String>,
    /// Idle timeout for QUEUE_DELAY scaling.
    pub idle_timeout: Option<u64>,
    /// QUEUE_DELAY or REQUEST_COUNT.
    pub scaler_type: String,
    /// Queue delay or requests per worker.
    pub scaler_value: u64,
    /// Minimum active workers.
    pub workers_min: u64,
    /// Maximum active workers.
    pub workers_max: u64,
    /// Enable FlashBoot.
    pub flashboot: bool,
    /// Comma-separated CUDA versions.
    pub allowed_cuda_versions: Option<String>,
    /// GPUs allocated to each worker.
    pub gpu_count: u64,
}
impl Default for EndpointOptions {
    fn default() -> Self {
        Self {
            name: String::new(),
            template_id: String::new(),
            gpu_ids: "AMPERE_16".into(),
            network_volume_id: None,
            locations: None,
            idle_timeout: None,
            scaler_type: "QUEUE_DELAY".into(),
            scaler_value: 4,
            workers_min: 0,
            workers_max: 3,
            flashboot: false,
            allowed_cuda_versions: None,
            gpu_count: 1,
        }
    }
}

/// User query retained verbatim to support captured Python wire fixtures.
pub const QUERY_USER: &str = "\nquery myself {\n    myself {\n        id\n        pubKey\n        networkVolumes {\n            id\n            name\n            size\n            dataCenterId\n        }\n    }\n}\n";

fn split_values(value: &str) -> Vec<&str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect()
}
fn nonempty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}
pub(crate) fn environment(env: &serde_json::Map<String, Value>) -> Value {
    Value::Object(
        env.iter()
            .map(|(k, v)| (k.clone(), Value::String(python_string(v))))
            .collect(),
    )
}
pub(crate) fn python_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        _ => v.to_string(),
    }
}
fn list_field(response: Value, key: &str) -> Result<Vec<Value>> {
    response
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| Error::InvalidResponse(format!("missing array field {key}")))
}

impl Client {
    /// Gets account information and network volumes.
    ///
    /// # Errors
    /// Returns request errors or a malformed GraphQL response.
    pub async fn get_user(&self) -> Result<Value> {
        self.run_graphql_query(QUERY_USER, None)
            .await?
            .pointer("/data/myself")
            .cloned()
            .ok_or_else(|| Error::InvalidResponse("missing data.myself".into()))
    }
    /// Replaces SSH public keys and returns refreshed account information.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn update_user_settings(&self, pubkey: &str) -> Result<Value> {
        let keys: Vec<_> = pubkey
            .lines()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .collect();
        self.run_rest_request(
            "PUT",
            "/v2/account/ssh-keys",
            Some(&json!({"keys":keys})),
            &[],
        )
        .await?;
        self.get_user().await
    }
    /// Lists available GPU types.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn get_gpus(&self) -> Result<Vec<Value>> {
        list_field(
            self.run_rest_request("GET", "/v2/catalog/gpus", None, &[])
                .await?,
            "gpus",
        )
    }
    /// Gets one GPU type and its availability for a specified quantity.
    ///
    /// # Errors
    /// Returns invalid input for unknown GPU IDs, or request errors.
    pub async fn get_gpu(&self, gpu_id: &str, gpu_quantity: u64) -> Result<Value> {
        let count = gpu_quantity.to_string();
        match self.run_rest_request("GET",&format!("/v2/catalog/gpus/{}",segment(gpu_id)),None,&[("include","AVAILABILITY"),("product","POD"),("count",&count)]).await {
            Err(e) if e.status_code()==Some(404)=>Err(Error::InvalidInput("No GPU found with the specified ID, run runpod.get_gpus() to get a list of all GPUs".into())), other=>other
        }
    }
    /// Lists standalone pods.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn get_pods(&self) -> Result<Vec<Value>> {
        list_field(
            self.run_rest_request("GET", "/v2/pods", None, &[]).await?,
            "pods",
        )
    }
    /// Gets a pod, returning None when it does not exist.
    ///
    /// # Errors
    /// Returns request errors other than not-found.
    pub async fn get_pod(&self, pod_id: &str) -> Result<Option<Value>> {
        match self
            .run_rest_request("GET", &format!("/v2/pods/{}", segment(pod_id)), None, &[])
            .await
        {
            Err(e) if e.status_code() == Some(404) => Ok(None),
            other => other.map(Some),
        }
    }
    /// Creates a GPU or CPU pod while preserving template override semantics.
    ///
    /// # Errors
    /// Rejects unsupported legacy constraints or invalid CPU configuration; propagates API errors.
    pub async fn create_pod(&self, o: &PodOptions) -> Result<Value> {
        if o.image_name.is_empty() && nonempty(&o.template_id).is_none() {
            return Err(Error::InvalidInput(
                "Either image_name or template_id must be provided".into(),
            ));
        }
        if !["ALL", "COMMUNITY", "SECURE"].contains(&o.cloud_type.as_str()) {
            return Err(Error::InvalidInput(
                "cloud_type must be one of ALL, COMMUNITY or SECURE".into(),
            ));
        }
        let gpu = nonempty(&o.gpu_type_id);
        let mut unsupported = Vec::new();
        if o.support_public_ip {
            unsupported.push("support_public_ip");
        }
        if o.country_code.is_some() {
            unsupported.push("country_code");
        }
        if gpu.is_none() && o.min_memory_in_gb != 1 {
            unsupported.push("min_memory_in_gb");
        }
        if gpu.is_none()
            && o.volume_mount_path.is_some()
            && nonempty(&o.network_volume_id).is_none()
        {
            unsupported.push("volume_mount_path on CPU pods without network_volume_id");
        }
        if o.min_download.is_some() {
            unsupported.push("min_download");
        }
        if o.min_upload.is_some() {
            unsupported.push("min_upload");
        }
        if !unsupported.is_empty() {
            return Err(Error::InvalidInput(format!(
                "REST API v2 does not support: {}",
                unsupported.join(", ")
            )));
        }
        let mut body = json!({"name":o.name,"startSsh":o.start_ssh});
        if let Some(v) = &o.docker_args {
            body["args"] = json!(v);
        }
        if !o.image_name.is_empty() {
            body["image"] = json!(o.image_name);
        }
        if let Some(v) = nonempty(&o.template_id) {
            body["templateId"] = json!(v);
        }
        if o.cloud_type != "ALL" {
            body["cloud"] = json!(o.cloud_type);
        }
        if let Some(v) = nonempty(&o.data_center_id) {
            body["dataCenterIds"] = json!([v]);
        }
        if let Some(v) = o.container_disk_in_gb {
            body["disk"] = json!(v);
        } else if nonempty(&o.template_id).is_none() {
            body["disk"] = json!(10);
        }
        if let Some(v) = &o.ports {
            body["ports"] = json!(split_values(v));
        }
        if let Some(v) = &o.env {
            body["env"] = environment(v);
        }
        let mount = o.volume_mount_path.as_deref().unwrap_or("/runpod-volume");
        if let Some(v) = nonempty(&o.network_volume_id) {
            body["mounts"] = json!({"network":[{"volumeId":v,"path":mount}]});
        } else if o.volume_in_gb != 0 {
            body["mounts"] = json!({"persistent":{"size":o.volume_in_gb,"path":mount}});
        } else if let (Some(template), Some(_), Some(path)) =
            (nonempty(&o.template_id), gpu, &o.volume_mount_path)
        {
            let template = self
                .run_rest_request(
                    "GET",
                    &format!("/v2/templates/{}", segment(template)),
                    None,
                    &[],
                )
                .await?;
            if let Some(persistent) = template
                .pointer("/mounts/persistent")
                .filter(|v| !v.is_null())
            {
                let size = persistent
                    .get("size")
                    .ok_or_else(|| Error::InvalidResponse("persistent mount lacks size".into()))?;
                body["mounts"] = json!({"persistent":{"size":size,"path":path}});
            }
        }
        if let Some(id) = gpu {
            let mut config = json!({"id":id,"count":o.gpu_count});
            if o.min_memory_in_gb != 1 {
                config["minRamPerGpu"] = json!(o.min_memory_in_gb);
            }
            if o.min_vcpu_count != 1 {
                config["minVcpuCountPerGpu"] = json!(o.min_vcpu_count);
            }
            if let Some(v) = &o.allowed_cuda_versions {
                config["allowedCudaVersions"] = json!(
                    v.iter()
                        .map(|v| v.trim())
                        .filter(|v| !v.is_empty())
                        .collect::<Vec<_>>()
                );
            }
            body["gpu"] = config;
        } else {
            let id = nonempty(&o.instance_id).ok_or_else(|| {
                Error::InvalidInput("instance_id must be provided for CPU pods".into())
            })?;
            body["cpu"] = cpu_config(id)?;
        }
        self.run_rest_request("POST", "/v2/pods", Some(&body), &[])
            .await
    }
    /// Stops a pod without deleting its storage.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn stop_pod(&self, pod_id: &str) -> Result<Value> {
        self.run_rest_request(
            "POST",
            &format!("/v2/pods/{}/action", segment(pod_id)),
            Some(&json!({"action":"stop"})),
            &[],
        )
        .await
    }
    /// Restarts a stopped pod; REST v2 does not support changing GPU count here.
    ///
    /// # Errors
    /// Returns invalid input if gpu_count is supplied, or request errors.
    pub async fn resume_pod(&self, pod_id: &str, gpu_count: Option<u64>) -> Result<Value> {
        if gpu_count.is_some() {
            return Err(Error::InvalidInput(
                "REST API v2 does not support gpu_count when resuming a pod".into(),
            ));
        }
        self.run_rest_request(
            "POST",
            &format!("/v2/pods/{}/action", segment(pod_id)),
            Some(&json!({"action":"start"})),
            &[],
        )
        .await
    }
    /// Terminates a pod.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn terminate_pod(&self, pod_id: &str) -> Result<()> {
        self.run_rest_request(
            "DELETE",
            &format!("/v2/pods/{}", segment(pod_id)),
            None,
            &[],
        )
        .await?;
        Ok(())
    }
    /// Creates a container template.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn create_template(&self, o: &TemplateOptions) -> Result<Value> {
        let mut body = json!({"name":o.name,"image":o.image_name,"disk":o.container_disk_in_gb,"serverless":o.is_serverless});
        if let Some(v) = &o.docker_start_cmd {
            body["args"] = json!(v);
        }
        if let Some(size) = o.volume_in_gb.filter(|v| *v != 0) {
            body["mounts"] = json!({"persistent":{"size":size,"path":nonempty(&o.volume_mount_path).unwrap_or("/workspace")}});
        }
        if let Some(v) = &o.ports {
            body["ports"] = json!(split_values(v));
        }
        if let Some(v) = &o.env {
            body["env"] = environment(v);
        }
        if let Some(v) = &o.registry_auth_id {
            body["registry"] = json!(v);
        }
        self.run_rest_request("POST", "/v2/templates", Some(&body), &[])
            .await
    }
    /// Lists serverless endpoints.
    ///
    /// # Errors
    /// Returns request or response errors.
    pub async fn get_endpoints(&self) -> Result<Vec<Value>> {
        list_field(
            self.run_rest_request("GET", "/v2/serverless", None, &[])
                .await?,
            "endpoints",
        )
    }
    /// Creates a serverless queue with GPU pool and exclusion filters.
    ///
    /// # Errors
    /// Returns invalid scaling configuration or request errors.
    pub async fn create_endpoint(&self, o: &EndpointOptions) -> Result<Value> {
        let scaling = match o.scaler_type.as_str() {
            "QUEUE_DELAY" => json!({"type":"QUEUE_DELAY","queueDelay":o.scaler_value}),
            "REQUEST_COUNT" => {
                if o.idle_timeout.is_some() {
                    return Err(Error::InvalidInput(
                        "idle_timeout is only supported with QUEUE_DELAY scaling".into(),
                    ));
                }
                json!({"type":"REQUEST_COUNT","requestCount":o.scaler_value})
            }
            _ => {
                return Err(Error::InvalidInput(
                    "scaler_type must be QUEUE_DELAY or REQUEST_COUNT".into(),
                ));
            }
        };
        let mut pools = Vec::new();
        let mut excluded = Vec::new();
        for id in split_values(&o.gpu_ids) {
            if let Some(exclusion) = id.strip_prefix('-') {
                let exclusion = exclusion.trim();
                if !exclusion.is_empty() && !excluded.contains(&exclusion) {
                    excluded.push(exclusion);
                }
            } else {
                pools.push(id);
            }
        }
        let mut gpu = json!({"pools":pools,"count":o.gpu_count});
        if !excluded.is_empty() {
            gpu["excludedTypes"] = json!(excluded);
        }
        if let Some(v) = &o.allowed_cuda_versions {
            gpu["allowedCudaVersions"] = json!(split_values(v));
        }
        let mut workers = json!({"min":o.workers_min,"max":o.workers_max});
        if o.scaler_type == "QUEUE_DELAY" {
            workers["idleTimeout"] = json!(o.idle_timeout.unwrap_or(5));
        }
        let mut body = json!({"name":o.name,"templateId":o.template_id,"type":"QUEUE","gpu":gpu,"workers":workers,"scaling":scaling,"flashboot":if o.flashboot {"FLASHBOOT"} else {"OFF"}});
        if let Some(v) = nonempty(&o.network_volume_id) {
            body["networkVolumes"] = json!([v]);
        }
        if let Some(v) = nonempty(&o.locations) {
            body["dataCenterIds"] = json!(split_values(v));
        }
        self.run_rest_request("POST", "/v2/serverless", Some(&body), &[])
            .await
    }
    /// Applies a different template to an existing endpoint.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn update_endpoint_template(
        &self,
        endpoint_id: &str,
        template_id: &str,
    ) -> Result<Value> {
        self.run_rest_request(
            "PATCH",
            &format!("/v2/serverless/{}", segment(endpoint_id)),
            Some(&json!({"templateId":template_id})),
            &[],
        )
        .await
    }
    /// Creates container registry credentials.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn create_container_registry_auth(
        &self,
        name: &str,
        username: &str,
        password: &str,
    ) -> Result<Value> {
        self.run_rest_request(
            "POST",
            "/v2/registries",
            Some(&json!({"name":name,"username":username,"password":password})),
            &[],
        )
        .await
    }
    /// Updates registry credentials through the upstream GraphQL operation.
    ///
    /// # Errors
    /// Returns request or malformed response errors.
    pub async fn update_container_registry_auth(
        &self,
        registry_auth_id: &str,
        username: &str,
        password: &str,
    ) -> Result<Value> {
        let id = serde_json::to_string(registry_auth_id)?;
        let username = serde_json::to_string(username)?;
        let password = serde_json::to_string(password)?;
        let query = format!(
            "\n    mutation UpdateRegistryAuth {{\n        updateRegistryAuth(input: {{id: {id}, username: {username}, password: {password}}}) {{\n            id\n            name\n        }}\n    }}\n    "
        );
        self.run_graphql_query(&query, None)
            .await?
            .pointer("/data/updateRegistryAuth")
            .cloned()
            .ok_or_else(|| Error::InvalidResponse("missing data.updateRegistryAuth".into()))
    }
    /// Deletes container registry credentials and returns true on success.
    ///
    /// # Errors
    /// Returns request errors.
    pub async fn delete_container_registry_auth(&self, registry_auth_id: &str) -> Result<bool> {
        self.run_rest_request(
            "DELETE",
            &format!("/v2/registries/{}", segment(registry_auth_id)),
            None,
            &[],
        )
        .await?;
        Ok(true)
    }
}

pub(crate) fn cpu_config(id: &str) -> Result<Value> {
    let parts: Vec<_> = id.split('-').collect();
    let positive =
        |v: &str| !v.starts_with('0') && !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit());
    if parts.len() != 3
        || parts[0].is_empty()
        || parts[0].chars().any(char::is_whitespace)
        || !positive(parts[1])
        || !positive(parts[2])
    {
        return Err(Error::InvalidInput(
            "instance_id must use the format <cpu-flavor>-<vcpu-count>-<memory>".into(),
        ));
    }
    let count = parts[1]
        .parse::<u64>()
        .map_err(|_| Error::InvalidInput("CPU count exceeds u64".into()))?;
    Ok(json!({"id":parts[0],"vcpuCount":count}))
}

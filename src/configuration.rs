//! Endpoint payload construction and editable configuration documents.
//!
//! Builders resolve catalog and registry selections but do not create or modify resources.
//! Omitted options remain omitted; updates merge environment, worker, and mount maps.
use crate::{Client, Error, Result, Value, client::truthy, compute, json};
use serde_json::Map;
use std::{fmt, str::FromStr};

/// Container settings shared by pod and endpoint configuration builders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerOptions {
    /// Container image, including an explicit empty string when supplied.
    pub image: Option<String>,
    /// Template ID, serialized as `templateId`.
    pub template: Option<String>,
    /// Raw container argument string.
    pub args: Option<String>,
    /// Container disk size in GB; no CLI range check is applied here.
    pub disk: Option<u64>,
    /// Whether to enable global networking, retaining an explicit false.
    pub global_networking: Option<bool>,
    /// Shell-quoted CMD arguments; parsed without executing a shell.
    pub cmd: Option<String>,
    /// Shell-quoted ENTRYPOINT arguments; empty clears the argument list.
    pub entrypoint: Option<String>,
    /// Exposed port/protocol strings; an empty list omits the field.
    pub ports: Vec<String>,
    /// Registry credential name or ID; an empty string clears it without lookup.
    pub registry: Option<String>,
    /// Repeated `KEY=VALUE` assignments, applied in order.
    pub env: Vec<String>,
}
/// Worker and autoscaler changes; absent options preserve the current values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScaleOptions {
    /// Minimum workers, including zero.
    pub min_workers: Option<u64>,
    /// Maximum workers, including zero.
    pub max_workers: Option<u64>,
    /// Idle-worker timeout in seconds.
    pub idle_timeout_seconds: Option<u64>,
    /// Queue-delay threshold in seconds; mutually exclusive with request count.
    pub queue_delay_seconds: Option<f64>,
    /// In-flight requests per worker.
    pub requests_per_worker: Option<u64>,
}
/// Serverless request routing used when constructing a new endpoint.
/// Parses and displays the CLI spellings `queue` and `load-balancer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointType {
    /// Queued jobs.
    Queue,
    /// Load-balanced HTTP requests.
    LoadBalancer,
}
impl EndpointType {
    fn api(self) -> &'static str {
        match self {
            Self::Queue => "QUEUE",
            Self::LoadBalancer => "LOAD_BALANCER",
        }
    }
}
impl fmt::Display for EndpointType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Queue => "queue",
            Self::LoadBalancer => "load-balancer",
        })
    }
}
impl FromStr for EndpointType {
    type Err = Error;
    /// # Errors
    /// Returns `InvalidInput` for anything other than an exact CLI spelling.
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "queue" => Ok(Self::Queue),
            "load-balancer" => Ok(Self::LoadBalancer),
            _ => Err(invalid("endpoint type must be queue or load-balancer")),
        }
    }
}
/// Endpoint cold-start optimization mode.
/// Parses `off`, `standard`, or `priority` without regard to ASCII case; displays lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flashboot {
    /// Disable `FlashBoot`.
    Off,
    /// Standard `FlashBoot`.
    Standard,
    /// Priority `FlashBoot`.
    Priority,
}
impl Flashboot {
    fn api(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Standard => "FLASHBOOT",
            Self::Priority => "PRIORITY_FLASHBOOT",
        }
    }
}
impl fmt::Display for Flashboot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Standard => "standard",
            Self::Priority => "priority",
        })
    }
}
impl FromStr for Flashboot {
    type Err = Error;
    /// # Errors
    /// Returns `InvalidInput` for an unknown mode.
    fn from_str(value: &str) -> Result<Self> {
        if value.eq_ignore_ascii_case("off") {
            Ok(Self::Off)
        } else if value.eq_ignore_ascii_case("standard") {
            Ok(Self::Standard)
        } else if value.eq_ignore_ascii_case("priority") {
            Ok(Self::Priority)
        } else {
            Err(invalid("flashboot must be off, standard, or priority"))
        }
    }
}
/// Endpoint configuration options, independent of CLI parsing and formatting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EndpointOptions {
    /// Container configuration; `ports` corresponds to the endpoint CLI's `--port`.
    pub container: ContainerOptions,
    /// Endpoint display name.
    pub name: Option<String>,
    /// `FlashBoot` mode.
    pub flashboot: Option<Flashboot>,
    /// Execution timeout in seconds, converted to milliseconds with overflow checking.
    pub request_timeout_seconds: Option<u64>,
    /// GPU names/pools, optionally suffixed with a per-worker count.
    pub gpu: Vec<String>,
    /// GPUs per worker; must agree with any count in GPU selectors.
    pub gpu_count: Option<u64>,
    /// CUDA floors/exact versions or a single `any` selector.
    pub cuda: Vec<String>,
    /// CPU flavors, optionally suffixed with `:VCPUS`.
    pub cpu: Vec<String>,
    /// vCPU count for selected or existing CPU flavors.
    pub vcpu_count: Option<u64>,
    /// Worker and autoscaler changes.
    pub scale: ScaleOptions,
    /// Explicit data-center selectors, resolved before region selectors.
    pub data_centers: Vec<String>,
    /// Region aliases or ID prefixes.
    pub regions: Vec<String>,
    /// Network volume IDs, replacing the existing network mount list when nonempty.
    pub network_volumes: Vec<String>,
    /// Global volume IDs, replacing the existing global mount list when nonempty.
    pub global_volumes: Vec<String>,
    /// Environment keys removed after assignments have been merged.
    pub unset_env: Vec<String>,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidInput(message.into())
}
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| Error::InvalidResponse("configuration must be an object".into()))
}
fn map_or_empty(value: &Value) -> Result<Map<String, Value>> {
    if truthy(value) {
        Ok(object(value)?.clone())
    } else {
        Ok(Map::new())
    }
}

/// Parses assignments without echoing invalid values in error messages.
///
/// # Errors
/// Returns `InvalidInput` for a missing key or equals sign. Later duplicate keys win.
pub fn parse_env<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<Map<String, Value>> {
    let mut env = Map::new();
    for value in values {
        let (key, content) = value
            .split_once('=')
            .filter(|(key, _)| !key.is_empty())
            .ok_or_else(|| invalid("invalid --env value; use KEY=VALUE"))?;
        env.insert(key.into(), content.into());
    }
    Ok(env)
}

// The CLI accepts shlex-style quoting, with comments and expansion disabled. A shell
// parser would instead remove # comments, line continuations, and some dollar escapes.
fn command_words(input: &str, option: &str) -> Result<Vec<String>> {
    #[derive(PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }
    let mut quote = Quote::None;
    let mut active = false;
    let mut word = String::new();
    let mut words = Vec::new();
    let mut chars = input.chars();
    let error = |message| invalid(format!("Invalid value for --{option}: {message}"));
    while let Some(ch) = chars.next() {
        if quote == Quote::Single {
            if ch == '\'' {
                quote = Quote::None;
            } else {
                word.push(ch);
            }
        } else if ch == '\\' {
            let next = chars.next().ok_or_else(|| error("No escaped character"))?;
            if quote == Quote::Double && !matches!(next, '\\' | '"') {
                word.push('\\');
            }
            word.push(next);
            active = true;
        } else if quote == Quote::Double {
            if ch == '"' {
                quote = Quote::None;
            } else {
                word.push(ch);
            }
        } else if matches!(ch, ' ' | '\t' | '\r' | '\n') {
            if active {
                words.push(std::mem::take(&mut word));
                active = false;
            }
        } else {
            match ch {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                _ => word.push(ch),
            }
            active = true;
        }
    }
    if quote != Quote::None {
        return Err(error("No closing quotation"));
    }
    if active {
        words.push(word);
    }
    Ok(words)
}
impl Client {
    /// Resolves container options, including registry lookup and environment merging.
    ///
    /// # Errors
    /// Returns invalid assignments/quoting, resolution, API/transport, or malformed-record errors.
    pub async fn container_fields(
        &self,
        options: &ContainerOptions,
        current: Option<&Value>,
        unset_env: &[String],
    ) -> Result<Value> {
        let current = current.unwrap_or(&Value::Null);
        let mut body = Map::new();
        for (key, value) in [
            ("image", &options.image),
            ("templateId", &options.template),
            ("args", &options.args),
        ] {
            if let Some(value) = value {
                body.insert(key.into(), value.clone().into());
            }
        }
        if let Some(value) = options.disk {
            body.insert("disk".into(), value.into());
        }
        if let Some(value) = options.global_networking {
            body.insert("globalNetworking".into(), value.into());
        }
        for (key, value) in [("cmd", &options.cmd), ("entrypoint", &options.entrypoint)] {
            if let Some(value) = value {
                body.insert(key.into(), command_words(value, key)?.into());
            }
        }
        if !options.ports.is_empty() {
            body.insert("ports".into(), json!(options.ports));
        }
        if let Some(registry) = &options.registry {
            let value = if registry.is_empty() {
                Value::Null
            } else {
                let rows =
                    crate::pagination::list_all(self, "/v2/registries", "registries", &[]).await?;
                rows.iter().find(|row| row["id"] == *registry || row["name"] == *registry)
                    .and_then(|row| row.get("id")).cloned()
                    .ok_or_else(|| compute::resolution(format!("unknown registry credential '{registry}'. list them with rp registry list")))?
            };
            body.insert("registry".into(), value);
        }
        if !options.env.is_empty() || !unset_env.is_empty() {
            let mut env = map_or_empty(&current["env"])?;
            env.extend(parse_env(options.env.iter().map(String::as_str))?);
            for key in unset_env {
                env.shift_remove(key);
            }
            body.insert("env".into(), env.into());
        }
        Ok(body.into())
    }
    /// Builds a creation payload, resolving hardware/region selectors without creating resources.
    ///
    /// # Errors
    /// Returns incompatible options, count/timeout overflow, resolution, or API/transport errors.
    /// Like the Python configuration helper, this does not enforce CLI-required image/name flags.
    pub async fn create_endpoint_config(
        &self,
        options: &EndpointOptions,
        kind: EndpointType,
    ) -> Result<Value> {
        endpoint_config(self, options, &json!({"type": kind.api()}), false).await
    }
    /// Builds an update payload against the current endpoint without applying changes.
    ///
    /// # Errors
    /// Returns incompatible options, count/timeout overflow, resolution, or API/transport errors.
    pub async fn update_endpoint_config(
        &self,
        options: &EndpointOptions,
        current: &Value,
    ) -> Result<Value> {
        object(current)?;
        endpoint_config(self, options, current, true).await
    }
}

/// Merges worker limits and selects an autoscaler without changing other endpoint fields.
///
/// # Errors
/// Returns conflicting scaler options, queue-delay scaling on load balancers, nonfinite
/// queue delays, or malformed current worker settings. CLI numeric ranges are not enforced.
pub fn scale_patch(options: &ScaleOptions, current: &Value) -> Result<Value> {
    let mut body = Map::new();
    let changes: Map<String, Value> = [
        ("min", options.min_workers),
        ("max", options.max_workers),
        ("idleTimeout", options.idle_timeout_seconds),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| (key.into(), value.into())))
    .collect();
    if !changes.is_empty() {
        let mut workers = map_or_empty(&current["workers"])?;
        workers.extend(changes);
        body.insert("workers".into(), workers.into());
    }
    match (options.queue_delay_seconds, options.requests_per_worker) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "use either --queue-delay-seconds or --requests-per-worker",
            ));
        }
        (Some(delay), None) => {
            if current["type"] == "LOAD_BALANCER" {
                return Err(invalid(
                    "load-balancer endpoints scale on --requests-per-worker",
                ));
            }
            if !delay.is_finite() {
                return Err(invalid("queue delay must be finite"));
            }
            body.insert(
                "scaling".into(),
                json!({"type":"QUEUE_DELAY", "queueDelay":delay}),
            );
        }
        (None, Some(count)) => {
            body.insert(
                "scaling".into(),
                json!({"type":"REQUEST_COUNT", "requestCount":count}),
            );
        }
        (None, None) => {}
    }
    Ok(body.into())
}
async fn gpu_config(
    client: &Client,
    options: &EndpointOptions,
    updating: bool,
) -> Result<Map<String, Value>> {
    let mut gpu = Map::new();
    let mut count = options.gpu_count.filter(|count| *count != 0);
    if !options.gpu.is_empty() {
        let mut catalog = client
            .list_catalog_gpus(&compute::GpuCatalogOptions {
                product: Some("SERVERLESS".into()),
                ..compute::GpuCatalogOptions::default()
            })
            .await?;
        catalog.retain(|row| truthy(&row["pool"]));
        let mut matches = Vec::new();
        let mut counts = std::collections::BTreeSet::new();
        if let Some(count) = count {
            counts.insert(count);
        }
        for spec in &options.gpu {
            let (name, specified) = compute::parse_gpu_spec(spec)?;
            if let Some(count) = specified {
                counts.insert(count);
            }
            for row in compute::resolve_gpus(&name, &catalog)? {
                if !matches.contains(&row) {
                    matches.push(row);
                }
            }
        }
        if counts.len() > 1 {
            return Err(invalid("conflicting gpu counts; use one count per worker"));
        }
        let (pools, excluded) = compute::endpoint_gpu_selection(matches, &catalog)?;
        gpu.insert("pools".into(), pools.into());
        if !excluded.is_empty() || updating {
            gpu.insert("excludedTypes".into(), excluded.into());
        }
        count = counts.into_iter().next();
    }
    if let Some(count) = count {
        gpu.insert("count".into(), count.into());
    }
    if !options.cuda.is_empty() {
        if options.cuda.len() == 1 && options.cuda[0].eq_ignore_ascii_case("any") {
            gpu.insert("allowedCudaVersions".into(), json!([]));
            if updating {
                gpu.insert("minCudaVersion".into(), "".into());
            }
        } else {
            let (minimum, exact) = compute::parse_cuda(options.cuda.iter().map(String::as_str))?;
            if let Some(minimum) = minimum {
                gpu.insert("minCudaVersion".into(), minimum.into());
                if updating {
                    gpu.insert("allowedCudaVersions".into(), json!([]));
                }
            } else {
                gpu.insert("allowedCudaVersions".into(), exact.into());
                if updating {
                    gpu.insert("minCudaVersion".into(), "".into());
                }
            }
        }
    }
    Ok(gpu)
}
async fn cpu_config(
    client: &Client,
    options: &EndpointOptions,
    current: &Value,
) -> Result<Option<Value>> {
    let vcpus = options.vcpu_count.filter(|count| *count != 0);
    if options.cpu.is_empty() {
        if let Some(count) = vcpus {
            if let Some(cpus) = current["cpu"].as_array().filter(|v| !v.is_empty()) {
                return Ok(Some(
                    cpus.iter()
                        .map(|cpu| {
                            let id = cpu.get("id").ok_or_else(|| {
                                Error::InvalidResponse("CPU is missing id".into())
                            })?;
                            Ok(json!({"id":id,"vcpuCount":count}))
                        })
                        .collect::<Result<Vec<_>>>()?
                        .into(),
                ));
            }
            return Err(invalid(
                "--vcpu-count applies to CPU endpoints (pass --cpu)",
            ));
        }
        return Ok(None);
    }
    let catalog = client.list_catalog_cpus(Some("SERVERLESS"), None).await?;
    let mut cpus = Vec::new();
    for spec in &options.cpu {
        let (name, count) = spec.rsplit_once(':').unwrap_or((spec, ""));
        let count = if count.is_empty() {
            vcpus
        } else {
            if !count.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid(format!(
                    "invalid --cpu '{spec}'. use a flavor like cpu3c or cpu3c:8"
                )));
            }
            Some(count.parse::<u64>().map_err(|_| {
                invalid(format!(
                    "invalid --cpu '{spec}'. use a flavor like cpu3c or cpu3c:8"
                ))
            })?)
        }
        .filter(|v| *v != 0)
        .ok_or_else(|| invalid(format!("pass --vcpu-count or --cpu {name}:VCPUS")))?;
        let cpu = compute::resolve_cpu(name, &catalog)?;
        cpus.push(json!({"id": cpu["id"], "vcpuCount": count}));
    }
    Ok(Some(cpus.into()))
}
async fn endpoint_config(
    client: &Client,
    options: &EndpointOptions,
    current: &Value,
    updating: bool,
) -> Result<Value> {
    let mut body = object(
        &client
            .container_fields(&options.container, Some(current), &options.unset_env)
            .await?,
    )?
    .clone();
    if !updating {
        body.insert("type".into(), current["type"].clone());
    }
    if let Some(name) = &options.name {
        body.insert("name".into(), name.clone().into());
    }
    if let Some(mode) = options.flashboot {
        body.insert("flashboot".into(), mode.api().into());
    }
    if let Some(seconds) = options.request_timeout_seconds {
        body.insert(
            "timeout".into(),
            seconds
                .checked_mul(1000)
                .ok_or_else(|| invalid("request timeout exceeds the millisecond integer limit"))?
                .into(),
        );
    }
    if !options.gpu.is_empty() && !options.cpu.is_empty() {
        return Err(invalid("use either --gpu or --cpu"));
    }
    if truthy(&current["cpu"]) && !options.gpu.is_empty() {
        return Err(invalid("a CPU endpoint cannot switch to GPUs"));
    }
    if truthy(&current["gpu"]) && !options.cpu.is_empty() {
        return Err(invalid("a GPU endpoint cannot switch to CPUs"));
    }
    if (!options.cpu.is_empty() || truthy(&current["cpu"]))
        && (!options.cuda.is_empty() || options.gpu_count.is_some_and(|v| v != 0))
    {
        return Err(invalid("--cuda and --gpu-count apply to GPU endpoints"));
    }
    let gpu = gpu_config(client, options, updating).await?;
    if !gpu.is_empty() {
        body.insert("gpu".into(), gpu.into());
    }
    if let Some(cpu) = cpu_config(client, options, current).await? {
        body.insert("cpu".into(), cpu);
    }
    body.extend(object(&scale_patch(&options.scale, current)?)?.clone());
    if !updating && !body.contains_key("scaling") {
        if current["type"] == "LOAD_BALANCER" {
            return Err(invalid(
                "load-balancer endpoints need --requests-per-worker",
            ));
        }
        body.insert(
            "scaling".into(),
            json!({"type":"QUEUE_DELAY", "queueDelay":4}),
        );
    }
    if !options.data_centers.is_empty() || !options.regions.is_empty() {
        let centers = client.list_catalog_datacenters(&[]).await?;
        body.insert(
            "dataCenterIds".into(),
            compute::resolve_regions(
                options
                    .data_centers
                    .iter()
                    .chain(&options.regions)
                    .map(String::as_str),
                &centers,
            )?
            .into(),
        );
    }
    if !options.network_volumes.is_empty() || !options.global_volumes.is_empty() {
        let mut mounts = map_or_empty(&current["mounts"])?;
        for (kind, ids) in [
            ("network", &options.network_volumes),
            ("global", &options.global_volumes),
        ] {
            if !ids.is_empty() {
                mounts.insert(
                    kind.into(),
                    ids.iter().map(|id| json!({"volumeId":id})).collect(),
                );
            }
        }
        body.insert("mounts".into(), mounts.into());
    }
    Ok(body.into())
}

/// Extracts an endpoint's editable document, dropping runtime-only fields and mount metadata.
///
/// # Errors
/// Returns malformed CPU/GPU/mount-record errors; the input is never changed.
pub fn mutable_endpoint(endpoint: &Value) -> Result<Value> {
    object(endpoint)?;
    let mut doc = Map::new();
    for key in ["name", "image", "args", "env", "disk", "ports"] {
        if let Some(value) = endpoint.get(key).filter(|v| !v.is_null()) {
            doc.insert(key.into(), value.clone());
        }
    }
    doc.insert("registry".into(), endpoint["registry"].clone());
    let gpu = &endpoint["gpu"];
    if truthy(gpu) {
        object(gpu)?;
        let or = |key: &str, default: Value| {
            if truthy(&gpu[key]) {
                gpu[key].clone()
            } else {
                default
            }
        };
        doc.insert("gpu".into(), json!({"pools":or("pools",json!([])),"excludedTypes":or("excludedTypes",json!([])),"count":or("count",json!(1)),"allowedCudaVersions":or("allowedCudaVersions",json!([])),"minCudaVersion":gpu["minCudaVersion"]}));
    }
    if truthy(&endpoint["cpu"]) {
        let cpus = endpoint["cpu"]
            .as_array()
            .ok_or_else(|| Error::InvalidResponse("cpu must be an array".into()))?;
        doc.insert(
            "cpu".into(),
            cpus.iter()
                .map(|cpu| {
                    let id = cpu
                        .get("id")
                        .ok_or_else(|| Error::InvalidResponse("CPU is missing id".into()))?;
                    let count = cpu
                        .get("vcpuCount")
                        .ok_or_else(|| Error::InvalidResponse("CPU is missing vcpuCount".into()))?;
                    Ok(json!({"id":id,"vcpuCount":count}))
                })
                .collect::<Result<Vec<_>>>()?
                .into(),
        );
    }
    for key in [
        "workers",
        "scaling",
        "timeout",
        "flashboot",
        "dataCenterIds",
    ] {
        if let Some(value) = endpoint.get(key).filter(|v| !v.is_null()) {
            doc.insert(key.into(), value.clone());
        }
    }
    let mounts = map_or_empty(&endpoint["mounts"])?;
    let mut selected = Map::new();
    for kind in ["network", "global"] {
        let values = match mounts.get(kind) {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(values)) => values.as_slice(),
            _ => return Err(Error::InvalidResponse("mounts must contain arrays".into())),
        };
        selected.insert(
            kind.into(),
            values
                .iter()
                .map(|mount| {
                    mount
                        .get("volumeId")
                        .map(|id| json!({"volumeId":id}))
                        .ok_or_else(|| Error::InvalidResponse("mount is missing volumeId".into()))
                })
                .collect::<Result<Vec<_>>>()?
                .into(),
        );
    }
    doc.insert("mounts".into(), selected.into());
    Ok(doc.into())
}

/// Validates edited JSON fields and endpoint-kind restrictions without changing the document.
///
/// # Errors
/// Returns `InvalidInput` for invalid JSON or semicolon-separated validation problems.
/// Nested value ranges are left to the API, matching the source editor validator.
pub fn validate_endpoint_document(text: &str, before: &Value) -> Result<Value> {
    let doc: Value =
        serde_json::from_str(text).map_err(|e| invalid(format!("invalid JSON: {e}")))?;
    let fields = doc
        .as_object()
        .ok_or_else(|| invalid("the document must be a JSON object"))?;
    let mut problems = Vec::new();
    for (key, value) in fields {
        let (valid, kind) = match key.as_str() {
            "name" | "image" | "args" | "flashboot" => (value.is_string(), "a string"),
            "disk" | "timeout" => (value.is_i64() || value.is_u64(), "an integer"),
            "env" | "gpu" | "workers" | "scaling" | "mounts" => (value.is_object(), "an object"),
            "ports" | "cpu" | "dataCenterIds" => (value.is_array(), "a list"),
            "registry" => (value.is_string() || value.is_null(), "a string or null"),
            _ => {
                problems.push(format!("unknown field '{key}'"));
                continue;
            }
        };
        if !valid {
            problems.push(format!("'{key}' must be {kind}"));
            continue;
        }
        let allowed: &[&str] = match key.as_str() {
            "gpu" => &[
                "pools",
                "excludedTypes",
                "count",
                "allowedCudaVersions",
                "minCudaVersion",
            ],
            "workers" => &["min", "max", "idleTimeout"],
            "scaling" => &["type", "queueDelay", "requestCount"],
            "mounts" => &["network", "global"],
            _ => &[],
        };
        if !allowed.is_empty()
            && let Some(nested_fields) = value.as_object()
        {
            for nested in nested_fields.keys() {
                if !allowed.contains(&nested.as_str()) {
                    problems.push(format!("unknown field '{key}.{nested}'"));
                }
            }
        }
    }
    if doc["env"]
        .as_object()
        .is_some_and(|env| env.values().any(|v| !v.is_string()))
    {
        problems.push("env values must be strings".into());
    }
    if doc["flashboot"]
        .as_str()
        .is_some_and(|v| !matches!(v, "OFF" | "FLASHBOOT" | "PRIORITY_FLASHBOOT"))
    {
        problems.push("'flashboot' must be one of OFF, FLASHBOOT, PRIORITY_FLASHBOOT".into());
    }
    if fields.contains_key("gpu") && before.get("gpu").is_none() {
        problems.push("a CPU endpoint cannot switch to GPUs".into());
    }
    if fields.contains_key("cpu") && before.get("cpu").is_none() {
        problems.push("a GPU endpoint cannot switch to CPUs".into());
    }
    if problems.is_empty() {
        Ok(doc)
    } else {
        Err(invalid(problems.join("; ")))
    }
}

/// Computes changes from an edited document; omitted fields remain unchanged.
///
/// GPU pool/exclusion updates are paired, and a cleared CUDA floor becomes an empty string.
/// # Errors
/// Returns malformed object errors. Validate edited documents before calling this function.
pub fn endpoint_patch(before: &Value, after: &Value) -> Result<Value> {
    object(before)?;
    let mut patch = Map::new();
    for (key, value) in object(after)? {
        if crate::json_value::equivalent(&before[key], value) {
            continue;
        }
        if key != "gpu" {
            patch.insert(key.clone(), value.clone());
            continue;
        }
        let old = map_or_empty(&before["gpu"])?;
        let edited = object(value)?;
        let mut gpu: Map<String, Value> = edited
            .iter()
            .filter(|(key, value)| {
                !crate::json_value::equivalent(old.get(*key).unwrap_or(&Value::Null), value)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if gpu.contains_key("pools") || gpu.contains_key("excludedTypes") {
            gpu.insert(
                "pools".into(),
                edited
                    .get("pools")
                    .or_else(|| old.get("pools"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            gpu.insert(
                "excludedTypes".into(),
                edited
                    .get("excludedTypes")
                    .or_else(|| old.get("excludedTypes"))
                    .filter(|v| truthy(v))
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            );
        }
        if gpu.get("minCudaVersion") == Some(&Value::Null) {
            gpu.insert("minCudaVersion".into(), "".into());
        }
        patch.insert("gpu".into(), gpu.into());
    }
    Ok(patch.into())
}

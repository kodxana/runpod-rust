//! Preserves the Python Apps client's atomic REST/GraphQL save selection.
use crate::{Error, Result, Value, client::truthy, control::cpu_config, json};
use serde_json::Map;

fn split(value: &Value) -> Result<Vec<String>> {
    let parts: Vec<&str> = match value {
        Value::String(s) => s.split(',').collect(),
        Value::Array(a) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| Error::InvalidInput("expected string list".into()))
            })
            .collect::<Result<_>>()?,
        _ => {
            return Err(Error::InvalidInput(
                "expected comma-separated string or string list".into(),
            ));
        }
    };
    Ok(parts
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

pub(super) fn rest_endpoint_input(endpoint: &Value) -> Result<Option<Value>> {
    let input = endpoint
        .as_object()
        .ok_or_else(|| Error::InvalidInput("endpoint input must be an object".into()))?;
    let empty = Map::new();
    let template = input
        .get("template")
        .filter(|v| truthy(v))
        .map(|v| {
            v.as_object()
                .ok_or_else(|| Error::InvalidInput("template must be an object".into()))
        })
        .transpose()?
        .unwrap_or(&empty);
    const FIELDS: &[&str] = &[
        "id",
        "name",
        "type",
        "template",
        "templateId",
        "workersMin",
        "workersMax",
        "idleTimeout",
        "scalerType",
        "scalerValue",
        "executionTimeoutMs",
        "flashBootType",
        "locations",
        "instanceIds",
        "gpuIds",
        "gpuCount",
        "minCudaVersion",
        "allowedCudaVersions",
        "networkVolumeIds",
    ];
    const TEMPLATE: &[(&str, &str)] = &[
        ("imageName", "image"),
        ("containerDiskInGb", "disk"),
        ("dockerArgs", "args"),
        ("containerRegistryAuthId", "registry"),
    ];
    if input.keys().any(|k| !FIELDS.contains(&k.as_str()))
        || template
            .keys()
            .any(|k| !TEMPLATE.iter().any(|(g, _)| g == k) && k != "env" && k != "ports")
    {
        return Ok(None);
    }
    let routing = input.get("type").and_then(Value::as_str).unwrap_or("QB");
    if !matches!(routing, "QB" | "LB") {
        return Ok(None);
    }
    let scaler = input.get("scalerType").filter(|v| !v.is_null());
    if scaler.is_some_and(|v| v != "QUEUE_DELAY" && v != "REQUEST_COUNT")
        || input.contains_key("scalerType") != input.contains_key("scalerValue")
    {
        return Ok(None);
    }
    let creating = !input.get("id").is_some_and(truthy);
    if creating
        && (scaler.is_none()
            || !(template.get("imageName").is_some_and(truthy)
                || input.get("templateId").is_some_and(truthy)))
    {
        return Ok(None);
    }
    let mut body = Map::new();
    for &(gql, rest) in TEMPLATE {
        if let Some(value) = template.get(gql) {
            body.insert(rest.into(), value.clone());
        }
    }
    if let Some(env) = template.get("env") {
        let entries = env
            .as_array()
            .ok_or_else(|| Error::InvalidInput("template env must be a list".into()))?;
        let mut values = Map::new();
        for entry in entries {
            let key = crate::client::required_str(entry, "key")?;
            values.insert(
                key.into(),
                entry
                    .get("value")
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput("env entry is missing value".into()))?,
            );
        }
        body.insert("env".into(), Value::Object(values));
    }
    if let Some(ports) = template.get("ports") {
        body.insert("ports".into(), json!(split(ports)?));
    }
    for (gql, rest) in [
        ("name", "name"),
        ("templateId", "templateId"),
        ("executionTimeoutMs", "timeout"),
        ("flashBootType", "flashboot"),
    ] {
        if let Some(value) = input.get(gql) {
            body.insert(rest.into(), value.clone());
        }
    }
    if let Some(volumes) = input.get("networkVolumeIds") {
        let volumes = volumes
            .as_array()
            .ok_or_else(|| Error::InvalidInput("networkVolumeIds must be a list".into()))?;
        if volumes.iter().any(|v| {
            !v.as_object()
                .is_some_and(|o| o.len() == 1 && o.contains_key("networkVolumeId"))
        }) {
            return Ok(None);
        }
        body.insert(
            "networkVolumes".into(),
            json!(
                volumes
                    .iter()
                    .map(|v| &v["networkVolumeId"])
                    .collect::<Vec<_>>()
            ),
        );
    }
    if creating {
        body.insert(
            "type".into(),
            json!(if routing == "LB" {
                "LOAD_BALANCER"
            } else {
                "QUEUE"
            }),
        );
    }
    if let Some(locations) = input.get("locations") {
        body.insert("dataCenterIds".into(), json!(split(locations)?));
    }
    if let Some(instances) = input.get("instanceIds") {
        let instances = instances
            .as_array()
            .ok_or_else(|| Error::InvalidInput("instanceIds must be a list".into()))?;
        let cpus: Vec<_> = instances
            .iter()
            .map(|v| cpu_config(v.as_str().unwrap_or("")))
            .collect::<Result<_>>()?;
        if cpus
            .iter()
            .any(|cpu| cpu["vcpuCount"].as_u64().is_some_and(|n| n < 2))
        {
            return Ok(None);
        }
        body.insert("cpu".into(), json!(cpus));
    }
    let mut gpu = Map::new();
    if let Some(ids) = input.get("gpuIds") {
        let tokens = split(ids)?;
        gpu.insert(
            "pools".into(),
            json!(
                tokens
                    .iter()
                    .filter(|t| !t.starts_with('-'))
                    .collect::<Vec<_>>()
            ),
        );
        gpu.insert(
            "excludedTypes".into(),
            json!(
                tokens
                    .iter()
                    .filter_map(|t| t.strip_prefix('-'))
                    .collect::<Vec<_>>()
            ),
        );
    }
    for (gql, rest) in [
        ("gpuCount", "count"),
        ("minCudaVersion", "minCudaVersion"),
        ("allowedCudaVersions", "allowedCudaVersions"),
    ] {
        if let Some(v) = input.get(gql) {
            gpu.insert(
                rest.into(),
                if gql == "allowedCudaVersions" {
                    json!(split(v)?)
                } else {
                    v.clone()
                },
            );
        }
    }
    if !gpu.is_empty() {
        body.insert("gpu".into(), Value::Object(gpu));
    }
    let mut workers = Map::new();
    for (gql, rest) in [
        ("workersMin", "min"),
        ("workersMax", "max"),
        ("idleTimeout", "idleTimeout"),
    ] {
        if let Some(v) = input.get(gql) {
            workers.insert(rest.into(), v.clone());
        }
    }
    if creating && routing == "QB" && scaler.is_some_and(|v| v == "REQUEST_COUNT") {
        workers.remove("idleTimeout");
    }
    if !workers.is_empty() {
        body.insert("workers".into(), Value::Object(workers));
    }
    if let Some(scaler) = scaler {
        let key = if scaler == "QUEUE_DELAY" {
            "queueDelay"
        } else {
            "requestCount"
        };
        body.insert(
            "scaling".into(),
            json!({"type":scaler,key:&input["scalerValue"]}),
        );
    }
    Ok(Some(Value::Object(body)))
}

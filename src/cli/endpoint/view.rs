use super::super::output;
use crate::{
    Error, Result, Value,
    client::{required_str, truthy},
    control::python_string,
    json,
};

pub(super) fn kind(record: &Value) -> Value {
    match record["type"].as_str() {
        Some("QUEUE") => "queue".into(),
        Some("LOAD_BALANCER") => "load-balancer".into(),
        _ => record["type"].clone(),
    }
}
fn flashboot(record: &Value) -> Value {
    match record["flashboot"].as_str() {
        Some("OFF") => "off".into(),
        Some("FLASHBOOT") => "standard".into(),
        Some("PRIORITY_FLASHBOOT") => "priority".into(),
        _ => record["flashboot"].clone(),
    }
}
fn strings(value: &Value) -> Result<Vec<String>> {
    if !truthy(value) {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or_else(|| Error::InvalidResponse("expected an array of strings".into()))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::InvalidResponse("expected a string".into()))
        })
        .collect()
}
fn number(value: &Value) -> Result<f64> {
    value
        .as_f64()
        .ok_or_else(|| Error::InvalidResponse("expected a number".into()))
}
fn compute(record: &Value, compact: bool) -> Result<Value> {
    let gpu = &record["gpu"];
    if truthy(gpu) {
        let pools = strings(&gpu["pools"])?;
        let count = if truthy(&gpu["count"]) {
            python_string(&gpu["count"])
        } else {
            "1".into()
        };
        let text = format!("{} x{count}", pools.join(", "));
        let excluded = strings(&gpu["excludedTypes"])?;
        return Ok(if compact || excluded.is_empty() {
            text
        } else {
            format!("{text} (excluding {})", excluded.join(", "))
        }
        .into());
    }
    let cpus = record["cpu"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let values = cpus
        .iter()
        .map(|cpu| {
            Ok(format!(
                "{} {} vcpu",
                required_str(cpu, "id")?,
                python_string(&cpu["vcpuCount"])
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(if values.is_empty() {
        Value::Null
    } else {
        values.join(", ").into()
    })
}
fn cuda(record: &Value) -> Result<Value> {
    let gpu = &record["gpu"];
    if !truthy(gpu) {
        return Ok(Value::Null);
    }
    if truthy(&gpu["minCudaVersion"]) {
        return Ok(format!(">={}", python_string(&gpu["minCudaVersion"])).into());
    }
    let versions = strings(&gpu["allowedCudaVersions"])?;
    Ok(if versions.is_empty() {
        "any".into()
    } else {
        versions.join(", ").into()
    })
}
fn scaling(record: &Value, compact: bool) -> Result<Value> {
    let scale = &record["scaling"];
    Ok(match scale["type"].as_str() {
        Some("QUEUE_DELAY") => {
            let delay = output::general(number(&scale["queueDelay"])?, 6);
            if compact {
                format!("{delay}s delay")
            } else {
                format!("queue delay {delay}s")
            }
            .into()
        }
        Some("REQUEST_COUNT") => {
            let count = python_string(&scale["requestCount"]);
            if compact {
                format!("{count}/worker")
            } else {
                format!("{count} requests per worker")
            }
            .into()
        }
        _ => Value::Null,
    })
}
fn workers(record: &Value) -> Value {
    let workers = &record["workers"];
    if workers.get("min").is_none() && workers.get("max").is_none() {
        return Value::Null;
    }
    format!(
        "{}-{}",
        python_string(workers.get("min").unwrap_or(&json!(0))),
        python_string(workers.get("max").unwrap_or(&json!(0)))
    )
    .into()
}
fn seconds(value: &Value) -> Result<Value> {
    if !truthy(value) {
        return Ok(Value::Null);
    }
    Ok(format!("{}s", output::general(number(value)?, 6)).into())
}
fn volumes(record: &Value) -> Result<Value> {
    let mut values = Vec::new();
    for kind in ["network", "global"] {
        if let Some(mounts) = record["mounts"][kind].as_array() {
            for mount in mounts {
                values.push(format!("{} ({kind})", required_str(mount, "volumeId")?));
            }
        }
    }
    Ok(json!(values))
}
fn fields(values: Vec<(&str, Value)>) -> String {
    output::fields(
        &values
            .into_iter()
            .map(|(k, v)| (k.into(), output::cell(&v)))
            .collect::<Vec<_>>(),
    )
}
fn table(headers: &[&str], rows: Vec<Vec<Value>>, empty: &str) -> String {
    output::table(
        &headers.iter().map(|v| (*v).into()).collect::<Vec<_>>(),
        &rows
            .into_iter()
            .map(|r| r.iter().map(output::cell).collect())
            .collect::<Vec<_>>(),
        empty,
    )
}
fn created(record: &Value, length: usize) -> Value {
    record["createdAt"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(length)
        .collect::<String>()
        .into()
}
pub(super) fn listing(records: &[Value]) -> Result<String> {
    let rows = records
        .iter()
        .map(|r| {
            Ok(vec![
                required_str(r, "id")?.into(),
                r["name"].clone(),
                kind(r),
                compute(r, true)?,
                workers(r),
                scaling(r, true)?,
                created(r, 10),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(table(
        &[
            "ID", "NAME", "TYPE", "COMPUTE", "WORKERS", "SCALING", "CREATED",
        ],
        rows,
        "no endpoints match",
    ))
}
pub(super) fn detail(record: &Value) -> Result<String> {
    let mut env: Vec<_> = record["env"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.keys().cloned())
        .collect();
    env.sort();
    let timeout = if truthy(&record["timeout"]) {
        json!(number(&record["timeout"])? / 1000.0)
    } else {
        json!(0)
    };
    let url = if truthy(&record["requestUrls"]["base"]) {
        record["requestUrls"]["base"].clone()
    } else {
        record["requestUrls"]["run"].clone()
    };
    Ok(fields(vec![
        ("id", record["id"].clone()),
        ("name", record["name"].clone()),
        ("type", kind(record)),
        ("image", record["image"].clone()),
        ("compute", compute(record, false)?),
        ("cuda", cuda(record)?),
        ("workers", workers(record)),
        ("idle timeout", seconds(&record["workers"]["idleTimeout"])?),
        ("scaling", scaling(record, false)?),
        ("request timeout", seconds(&timeout)?),
        ("flashboot", flashboot(record)),
        ("data centers", record["dataCenterIds"].clone()),
        ("volumes", volumes(record)?),
        ("env", json!(env)),
        ("ports", record["ports"].clone()),
        (
            "disk",
            if truthy(&record["disk"]) {
                format!("{}GB", python_string(&record["disk"])).into()
            } else {
                Value::Null
            },
        ),
        ("args", record["args"].clone()),
        ("registry", record["registry"].clone()),
        ("url", url),
        ("created", record["createdAt"].clone()),
    ]))
}
pub(super) fn scale(record: &Value, id: &str) -> Result<String> {
    Ok(fields(vec![
        (
            "id",
            if truthy(&record["id"]) {
                record["id"].clone()
            } else {
                id.into()
            },
        ),
        ("workers", workers(record)),
        ("idle timeout", seconds(&record["workers"]["idleTimeout"])?),
        ("scaling", scaling(record, false)?),
    ]))
}
pub(super) fn rollout(state: &Value) -> String {
    let rollout = &state["rollout"];
    let phase = if truthy(&rollout["inProgress"]) {
        "rolling out"
    } else {
        "live"
    };
    format!(
        "v{} {phase}, {}/{} workers on latest",
        python_string(&state["endpointVersion"]),
        python_string(rollout.get("workersOnLatest").unwrap_or(&json!(0))),
        python_string(rollout.get("workersTotal").unwrap_or(&json!(0)))
    )
}
pub(super) fn releases(state: &Value, releases: &[Value]) -> Result<String> {
    let rows = releases
        .iter()
        .map(|r| {
            let changes = r["diff"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|v| truthy(&v["field"]) && (truthy(&v["old"]) || truthy(&v["new"])))
                .map(|v| {
                    required_str(v, "field")
                        .map(|s| s.strip_prefix("template.").unwrap_or(s).to_owned())
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(vec![
                required_str(r, "id")?.into(),
                if r["version"].is_null() {
                    Value::Null
                } else {
                    format!("v{}", python_string(&r["version"])).into()
                },
                r["source"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase()
                    .replace('_', " ")
                    .into(),
                r["buildId"].clone(),
                r["workerCount"].clone(),
                created(r, 16)
                    .as_str()
                    .unwrap_or_default()
                    .replace('T', " ")
                    .into(),
                json!(changes),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    let mut text = if state["endpointVersion"].is_null() {
        String::new()
    } else {
        format!("{}\n", rollout(state))
    };
    text.push_str(&table(
        &[
            "RELEASE", "VERSION", "SOURCE", "BUILD", "WORKERS", "CREATED", "CHANGES",
        ],
        rows,
        "no releases",
    ));
    Ok(text)
}
fn uptime(value: &Value) -> Result<Value> {
    if !truthy(value) {
        return Ok(Value::Null);
    }
    let seconds = value
        .as_i64()
        .ok_or_else(|| Error::InvalidResponse("uptimeSeconds must be an integer".into()))?;
    let hours = seconds.div_euclid(3600);
    let rest = seconds.rem_euclid(3600);
    let minutes = rest / 60;
    let seconds = rest % 60;
    Ok(if hours != 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes != 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
    .into())
}
fn worker_version(worker: &Value) -> Value {
    let stale = truthy(&worker["isStale"]);
    if worker["version"].is_null() {
        return if stale { "stale".into() } else { Value::Null };
    }
    format!(
        "v{}{}",
        python_string(&worker["version"]),
        if stale { " (stale)" } else { "" }
    )
    .into()
}
pub(super) fn worker_view(data: &Value) -> Result<String> {
    let summary = &data["summary"];
    let counts = ["running", "idle", "initializing", "throttled", "unhealthy"]
        .iter()
        .map(|status| {
            summary
                .get(*status)
                .map(|v| format!("{} {status}", python_string(v)))
                .ok_or_else(|| Error::InvalidResponse(format!("missing worker summary {status}")))
        })
        .collect::<Result<Vec<_>>>()?;
    let total = summary
        .get("total")
        .ok_or_else(|| Error::InvalidResponse("missing worker summary total".into()))?;
    let suffix = if data["endpointVersion"].is_null() {
        String::new()
    } else {
        format!(", endpoint v{}", python_string(&data["endpointVersion"]))
    };
    let mut text = format!(
        "{} workers: {}{suffix}\n",
        python_string(total),
        counts.join(", ")
    );
    let workers = data["workers"]
        .as_array()
        .ok_or_else(|| Error::InvalidResponse("missing workers array".into()))?;
    if !workers.is_empty() {
        let rows = workers
            .iter()
            .map(|w| {
                Ok(vec![
                    required_str(w, "id")?.into(),
                    w["status"]
                        .as_str()
                        .unwrap_or_default()
                        .to_lowercase()
                        .into(),
                    if truthy(&w["gpuCount"]) {
                        format!(
                            "{} x{}",
                            python_string(&w["gpuTypeId"]),
                            python_string(&w["gpuCount"])
                        )
                        .into()
                    } else {
                        Value::Null
                    },
                    w["dataCenterId"].clone(),
                    worker_version(w),
                    uptime(&w["uptimeSeconds"])?,
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        text.push_str(&table(
            &[
                "WORKER",
                "STATUS",
                "GPU",
                "DATA CENTER",
                "VERSION",
                "UPTIME",
            ],
            rows,
            "",
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_workers_empty_and_uptime() {
        assert_eq!(uptime(&Value::Null).unwrap(), Value::Null);
        assert_eq!(uptime(&json!(42)).unwrap(), json!("42s"));
        assert_eq!(worker_version(&json!({"version":null})), Value::Null);
        let text=worker_view(&json!({"summary":{"running":0,"idle":0,"initializing":0,"throttled":0,"unhealthy":0,"total":0},"workers":[]})).unwrap();
        assert!(text.starts_with("0 workers:"));
        assert!(!text.contains("WORKER"));
    }
}

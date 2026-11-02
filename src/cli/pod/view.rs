use super::{CliResult, output};
use crate::{
    Value,
    client::{required_str, truthy},
    control::python_string,
    json,
};

pub(super) fn status(pod: &Value) -> crate::Result<String> {
    let status = required_str(pod, "status")?.to_lowercase();
    Ok(
        if status == "running" && !truthy(&pod["runtime"]["uptime"]) {
            "starting".into()
        } else {
            status
        },
    )
}
fn hardware(pod: &Value) -> &Value {
    if truthy(&pod["gpu"]) {
        &pod["gpu"]
    } else {
        &pod["cpu"]
    }
}
fn summary(pod: &Value) -> crate::Result<Vec<(String, Value)>> {
    let hardware = hardware(pod);
    let description = if truthy(&pod["gpu"]) {
        json!(format!(
            "{}x {}",
            python_string(&hardware["count"]),
            required_str(hardware, "id")?
        ))
    } else if truthy(&pod["cpu"]) {
        json!(format!(
            "{} {} vcpu",
            required_str(hardware, "id")?,
            python_string(&hardware["vcpuCount"])
        ))
    } else {
        Value::Null
    };
    Ok(vec![
        ("id".into(), json!(required_str(pod, "id")?)),
        ("name".into(), json!(required_str(pod, "name")?)),
        ("status".into(), json!(status(pod)?)),
        ("hardware".into(), description),
        ("data center".into(), pod["dataCenterId"].clone()),
        (
            "cost".into(),
            pod["cost"]
                .as_f64()
                .filter(|n| *n != 0.0)
                .map_or(Value::Null, |n| json!(format!("${n:.2}/hr"))),
        ),
        ("image".into(), json!(required_str(pod, "image")?)),
    ])
}
pub(super) fn list(pods: &[Value]) -> CliResult<String> {
    let headers = [
        "ID",
        "NAME",
        "STATUS",
        "HARDWARE",
        "DATA CENTER",
        "COST",
        "IMAGE",
    ]
    .map(str::to_owned);
    let rows = pods
        .iter()
        .map(|pod| {
            summary(pod).map(|fields| {
                fields
                    .iter()
                    .map(|(_, value)| output::cell(value))
                    .collect()
            })
        })
        .collect::<crate::Result<Vec<Vec<String>>>>()?;
    Ok(output::table(
        &headers,
        &rows,
        "no pods. create one with rp pod create",
    ))
}
pub(super) fn show(pod: &Value, as_json: bool) -> CliResult<String> {
    if as_json {
        return Ok(output::json(pod)?);
    }
    let mut fields = summary(pod)?;
    let hardware = hardware(pod);
    let runtime = &pod["runtime"];
    let networking = &pod["globalNetworking"];
    let mut volumes = vec![];
    let persistent = &pod["mounts"]["persistent"];
    if truthy(persistent) {
        volumes.push(format!(
            "persistent {}GB at {}",
            python_string(&persistent["size"]),
            python_string(&persistent["path"])
        ));
    }
    for kind in ["network", "global"] {
        for volume in pod["mounts"][kind].as_array().into_iter().flatten() {
            volumes.push(format!(
                "{kind} {} at {}",
                python_string(&volume["volumeId"]),
                volume["path"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("-")
            ));
        }
    }
    let uptime = runtime["uptime"].as_u64().filter(|s| *s != 0).map(|total| {
        let seconds = total % 60;
        let minutes = total / 60 % 60;
        let hours = total / 3600;
        if hours > 0 {
            format!("{hours}h {minutes}m")
        } else if minutes > 0 {
            format!("{minutes}m {seconds}s")
        } else {
            format!("{seconds}s")
        }
    });
    let resources = [("vcpuCount", " vcpu"), ("memory", "GB ram")]
        .into_iter()
        .filter_map(|(key, unit)| {
            hardware[key]
                .as_f64()
                .filter(|n| *n != 0.0)
                .map(|n| format!("{}{unit}", output::general(n, 6)))
        })
        .collect::<Vec<_>>();
    let mut env = pod["env"]
        .as_object()
        .into_iter()
        .flat_map(|object| object.keys().cloned())
        .collect::<Vec<_>>();
    env.sort();
    let network = if !truthy(&networking["enabled"]) {
        json!(false)
    } else if truthy(&networking["internalDns"]) {
        networking["internalDns"].clone()
    } else if truthy(&networking["ip"]) {
        networking["ip"].clone()
    } else {
        json!(true)
    };
    let urls = pod["ports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|port| port.ends_with("/http"))
        .map(|port| {
            format!(
                "https://{}-{}.proxy.runpod.net",
                pod["id"].as_str().unwrap_or(""),
                port.split('/').next().unwrap_or("")
            )
        })
        .collect::<Vec<_>>();
    let tcp = runtime["ports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|port| port["type"] == "tcp" && truthy(&port["ip"]) && truthy(&port["public"]))
        .map(|port| {
            format!(
                "{}:{} -> {}",
                python_string(&port["ip"]),
                python_string(&port["public"]),
                python_string(&port["private"])
            )
        })
        .collect::<Vec<_>>();
    fields.extend([
        ("resources".into(), json!(resources)),
        (
            "cloud".into(),
            json!(pod["cloud"].as_str().unwrap_or("").to_lowercase()),
        ),
        ("cuda".into(), pod["cudaVersion"].clone()),
        (
            "disk".into(),
            if truthy(&pod["disk"]) {
                json!(format!("{}GB", python_string(&pod["disk"])))
            } else {
                Value::Null
            },
        ),
        ("volumes".into(), json!(volumes)),
        ("ports".into(), pod["ports"].clone()),
        ("env".into(), json!(env)),
        ("command".into(), pod["args"].clone()),
        ("locked".into(), pod["locked"].clone()),
        ("global network".into(), network),
        ("cluster".into(), pod["cluster"]["id"].clone()),
        ("created".into(), pod["createdAt"].clone()),
        ("started".into(), pod["startedAt"].clone()),
        ("uptime".into(), json!(uptime)),
        ("actions".into(), pod["actions"].clone()),
        ("ssh".into(), pod["ssh"]["proxy"]["command"].clone()),
        ("ssh direct".into(), pod["ssh"]["direct"]["command"].clone()),
        ("urls".into(), json!(urls)),
        ("tcp".into(), json!(tcp)),
    ]);
    Ok(output::fields(
        &fields
            .iter()
            .map(|(label, value)| (label.clone(), output::cell(value)))
            .collect::<Vec<_>>(),
    ))
}

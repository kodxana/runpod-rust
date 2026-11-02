use super::{
    CliError, CliResult, Streams,
    options::{Create, Hardware},
    say, usage,
};
use crate::{
    Client, Error, Value,
    client::{required_str, segment, truthy},
    compute,
    configuration::ContainerOptions,
    json,
};

#[derive(Default, PartialEq, Eq)]
pub(super) struct Container {
    pub options: ContainerOptions,
    pub persistent: Option<String>,
    pub network: Option<String>,
    pub global: Option<String>,
}
impl Container {
    pub fn needs_current(&self) -> bool {
        !self.options.env.is_empty()
            || [&self.persistent, &self.network, &self.global]
                .into_iter()
                .any(|s| s.as_ref().is_some_and(|s| !s.is_empty()))
    }
    pub async fn body(
        &self,
        client: &Client,
        current: Option<&Value>,
        unset: &[String],
    ) -> CliResult<Value> {
        if current.is_some_and(|value| truthy(value) && !value.is_object()) {
            return Err(
                Error::InvalidResponse("current pod settings must be an object".into()).into(),
            );
        }
        let mut body = client
            .container_fields(&self.options, current, unset)
            .await
            .map_err(usage)?;
        let mut mounts = serde_json::Map::new();
        for (kind, value) in [
            ("persistent", &self.persistent),
            ("network", &self.network),
            ("global", &self.global),
        ] {
            let Some(value) = value else { continue };
            let (head, path) = value
                .split_once(':')
                .map_or((value.as_str(), None), |(h, p)| (h, Some(p)));
            if head.is_empty() || path == Some("") {
                return Err(CliError::Usage(format!(
                    "Invalid value for --{kind}-volume: '{value}' should look like VALUE[:PATH]"
                )));
            }
            let previous = &current.unwrap_or(&Value::Null)["mounts"][kind];
            let previous = if kind == "persistent" {
                previous
            } else {
                &previous[0]
            };
            let path = path
                .or_else(|| previous["path"].as_str().filter(|s| !s.is_empty()))
                .or_else(|| (kind != "global").then_some("/workspace"));
            let mount = if kind == "persistent" {
                let size=head.bytes().all(|b|b.is_ascii_digit()).then(||head.parse::<u64>().ok()).flatten().ok_or_else(||CliError::Usage("Invalid value for --persistent-volume: size is in GB, like 50 or 50:/workspace".into()))?;
                json!({"size":size,"path":path})
            } else {
                let mut mount = json!({"volumeId":head});
                if let Some(path) = path {
                    mount["path"] = json!(path);
                }
                json!([mount])
            };
            mounts.insert(kind.into(), mount);
        }
        if !mounts.is_empty() {
            body["mounts"] = Value::Object(mounts);
        }
        Ok(body)
    }
}
impl Create {
    async fn datacenters(&self, client: &Client) -> CliResult<Vec<String>> {
        let mut ids = if self.regions.is_empty() {
            vec![]
        } else {
            compute::resolve_regions(
                self.regions.iter().map(String::as_str),
                &client.list_catalog_datacenters(&[]).await?,
            )?
        };
        for id in &self.datacenters {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        if ids.is_empty()
            && let Some(volume) = self.container.network.as_ref().filter(|v| !v.is_empty())
        {
            let id = volume
                .split(':')
                .next()
                .expect("split yields at least one item");
            let volume = client
                .run_rest_request(
                    "GET",
                    &format!("/v2/network-volumes/{}", segment(id)),
                    None,
                    &[],
                )
                .await?;
            ids.push(required_str(&volume, "dataCenter")?.into());
        }
        Ok(ids)
    }
    pub async fn body(
        &self,
        client: &Client,
        name: String,
        as_json: bool,
        streams: &mut Streams<'_>,
    ) -> CliResult<Value> {
        let mut body = json!({"name":name});
        let Value::Object(container) = self.container.body(client, None, &[]).await? else {
            return Err(
                Error::InvalidResponse("container settings must be an object".into()).into(),
            );
        };
        body.as_object_mut()
            .expect("object literal")
            .extend(container);
        let datacenters = self.datacenters(client).await?;
        if !datacenters.is_empty() {
            body["dataCenterIds"] = json!(datacenters);
        }
        if let Some(cloud) = self.cloud.as_ref().filter(|s| !s.is_empty()) {
            body["cloud"] = json!(cloud.to_uppercase());
        }
        match &self.hardware {
            Hardware::Gpu { .. } => {
                body["gpu"] = self.gpu(client, &datacenters, as_json, streams).await?;
            }
            Hardware::Cpu { spec, vcpu } => {
                let flavors = client.list_catalog_cpus(Some("POD"), *vcpu).await?;
                let flavor = compute::resolve_cpu(spec, &flavors)?;
                body["cpu"] = json!({"id":required_str(flavor,"id")?,"vcpuCount":vcpu.or_else(||flavor["vcpu"]["min"].as_u64().filter(|n|*n!=0)).unwrap_or(2)});
            }
        }
        if self.ssh {
            body["startSsh"] = json!(true);
            let keys = client
                .run_rest_request("GET", "/v2/account/ssh-keys", None, &[])
                .await?;
            if !truthy(&keys["keys"]) {
                say(
                    "your account has no ssh keys, so --ssh has no effect. add one with rp ssh add",
                    as_json,
                    streams,
                )?;
            }
        }
        if self.jupyter {
            body["startJupyter"] = json!(true);
        }
        Ok(body)
    }
    async fn gpu(
        &self,
        client: &Client,
        datacenters: &[String],
        as_json: bool,
        streams: &mut Streams<'_>,
    ) -> CliResult<Value> {
        let Hardware::Gpu {
            spec,
            count,
            cuda,
            min_ram,
            min_vcpu,
        } = &self.hardware
        else {
            return Err(Error::InvalidInput("GPU selection required".into()).into());
        };
        let (query, spec_count) = compute::parse_gpu_spec(spec)?;
        if let (Some(spec_count), Some(count)) = (spec_count, *count)
            && spec_count != count
        {
            return Err(CliError::Usage(format!(
                "--gpu {spec} conflicts with --gpu-count {count}"
            )));
        }
        let count = spec_count.or(*count).unwrap_or(1);
        let (min_cuda, exact_cuda) = compute::parse_cuda(cuda.iter().map(String::as_str))?;
        let cloud = self.cloud.as_deref().unwrap_or("secure").to_lowercase();
        let catalog = client
            .list_catalog_gpus(&compute::GpuCatalogOptions {
                product: Some("POD".into()),
                count: Some(count),
                cloud: Some(cloud.to_uppercase()),
                cuda_versions: exact_cuda.clone(),
                min_cuda_version: min_cuda.clone(),
            })
            .await?;
        let matches = compute::resolve_gpus(&query, &catalog)?;
        let price = |gpu: &Value| {
            gpu["price"][&cloud]
                .as_f64()
                .filter(|p| *p != 0.0)
                .unwrap_or(f64::INFINITY)
        };
        let best = matches
            .iter()
            .copied()
            .filter(|gpu| {
                matches.len() == 1
                    || (gpu.get(&cloud).is_none_or(truthy)
                        && gpu["maxCount"][&cloud].as_u64().unwrap_or(count) >= count)
            })
            .min_by(|a, b| {
                compute::availability_rank(Some(stock(b, datacenters)))
                    .cmp(&compute::availability_rank(Some(stock(a, datacenters))))
                    .then_with(|| price(a).total_cmp(&price(b)))
                    .then_with(|| label(a).cmp(label(b)))
            })
            .ok_or_else(|| {
                compute::resolution(format!(
                    "no '{query}' gpu ({}) offers {count} per pod on {cloud} cloud",
                    matches
                        .iter()
                        .map(|gpu| label(gpu))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        let availability = stock(best, datacenters);
        let label = label(best);
        if matches.len() > 1 {
            let cost = if price(best).is_finite() {
                format!(", ${:.2}/hr", price(best) * count as f64)
            } else {
                String::new()
            };
            let stock = if availability == "NONE" {
                "no".into()
            } else {
                availability.to_lowercase()
            };
            say(
                &format!(
                    "'{query}' matches {} gpu types; using {label} ({}): {stock} stock{cost}",
                    matches.len(),
                    required_str(best, "id")?
                ),
                as_json,
                streams,
            )?;
        }
        if availability == "NONE" {
            say(
                &format!(
                    "the catalog reports no {cloud} stock for {count}x {label} in the requested data centers; creation may fail"
                ),
                as_json,
                streams,
            )?;
        }
        let mut body = json!({"id":required_str(best,"id")?,"count":count});
        if let Some(minimum) = min_cuda {
            body["minCudaVersion"] = json!(minimum);
        }
        if !exact_cuda.is_empty() {
            body["allowedCudaVersions"] = json!(exact_cuda);
        }
        if let Some(value) = min_ram {
            body["minRamPerGpu"] = json!(value);
        }
        if let Some(value) = min_vcpu {
            body["minVcpuCountPerGpu"] = json!(value);
        }
        Ok(body)
    }
}
fn label(gpu: &Value) -> &str {
    gpu["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| gpu["id"].as_str())
        .unwrap_or("")
}
fn stock<'a>(gpu: &'a Value, allowed: &[String]) -> &'a str {
    if allowed.is_empty() {
        return gpu["availability"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("NONE");
    }
    compute::best_availability(
        gpu["dataCenters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|dc| {
                dc["id"]
                    .as_str()
                    .is_some_and(|id| allowed.iter().any(|a| a == id))
            })
            .map(|dc| dc["availability"].as_str()),
    )
}

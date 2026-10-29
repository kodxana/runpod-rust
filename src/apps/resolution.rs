use super::{
    AppsApiClient, DataCenter, ResourceSpec,
    events::{EventSink, emit},
    lookup::find_by_id_or_name,
    placement::{PlacementOptions, StockMap, hardware_keys},
    volume::{Mount, Volume, normalize_mounts},
};
use crate::{Error, Result, Value, client::required_str, json};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A resolved volume identifier and its optional datacenter constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedVolume {
    /// Backend volume identifier.
    pub id: String,
    /// Network volume location; absent for global volumes.
    #[serde(rename = "dataCenterId", skip_serializing_if = "Option::is_none")]
    pub data_center_id: Option<String>,
}

/// A resolved worker attachment, including the original name and actual backend ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedMount {
    /// Storage backend identifier, network or global.
    pub kind: String,
    /// User-provided name or ID.
    pub reference: String,
    /// Normalized POSIX worker path.
    pub path: String,
    /// Resolved ID and optional location serialized directly into the mount record.
    #[serde(flatten)]
    pub volume: ResolvedVolume,
}

/// Resolves storage once per provisioning run, applying shared-resource placement constraints.
#[derive(Debug)]
pub struct VolumeResolver {
    api: AppsApiClient,
    events: Option<EventSink>,
    resolved: BTreeMap<(String, String), ResolvedVolume>,
    stock: StockMap,
    volume_datacenters: Option<BTreeSet<String>>,
}
impl VolumeResolver {
    /// Creates a resolver with fresh storage and capacity caches.
    pub fn new(api: AppsApiClient) -> Self {
        Self {
            stock: StockMap::new(api.clone()),
            api,
            events: None,
            resolved: BTreeMap::new(),
            volume_datacenters: None,
        }
    }

    /// Installs an observation callback for volume creation events.
    pub fn set_events(&mut self, events: EventSink) {
        self.events = Some(events);
    }

    /// Resolves a volume by ID or unique name, creating it when configured to do so.
    ///
    /// # Errors
    /// Returns invalid resource, ambiguous name, placement, storage capability, or API errors.
    pub async fn resolve(
        &mut self,
        volume: &Volume,
        specs: &[ResourceSpec],
    ) -> Result<ResolvedVolume> {
        self.resolve_refs(volume, &specs.iter().collect::<Vec<_>>())
            .await
    }

    async fn resolve_refs(
        &mut self,
        volume: &Volume,
        specs: &[&ResourceSpec],
    ) -> Result<ResolvedVolume> {
        for spec in specs {
            spec.validate()?;
        }
        let key = (volume.kind().to_owned(), volume.reference().to_owned());
        if let Some(cached) = self.resolved.get(&key) {
            return Ok(cached.clone());
        }
        let network = match volume {
            Volume::Global(global) => {
                let existing = self.api.list_global_volumes().await?;
                let record = if let Some(record) =
                    find_by_id_or_name(&existing, global.name(), "global volumes")?
                {
                    record.clone()
                } else {
                    if !global.create {
                        return Err(Error::InvalidInput(format!(
                            "global volume '{}' not found and create=False",
                            global.name()
                        )));
                    }
                    self.api.create_global_volume(global.name()).await?
                };
                let resolved = ResolvedVolume {
                    id: required_str(&record, "id")?.into(),
                    data_center_id: None,
                };
                self.resolved.insert(key, resolved.clone());
                return Ok(resolved);
            }
            Volume::Network(network) => network,
        };
        let existing = self.api.list_network_volumes().await?;
        let mut record = find_by_id_or_name(&existing, network.name(), "volumes")?.cloned();
        if record.is_none() && !network.create {
            return Err(Error::InvalidInput(format!(
                "volume '{}' not found and create=False",
                network.name()
            )));
        }
        let mut dc = record
            .as_ref()
            .map(|r| required_str(r, "dataCenter").map(str::to_owned))
            .transpose()?;
        if record.is_none() {
            let mut pins = BTreeSet::new();
            let mut add_pin = |pin: Option<&str>| -> Result<()> {
                if let Some(pin) = pin.filter(|s| !s.is_empty()) {
                    pins.insert(pin.parse::<DataCenter>()?.to_string());
                }
                Ok(())
            };
            add_pin(network.datacenter.as_deref())?;
            for spec in specs {
                for mount in &spec.mounts {
                    if let Volume::Network(reference) = &mount.volume
                        && reference.name() == network.name()
                    {
                        add_pin(reference.datacenter.as_deref())?;
                    }
                }
            }
            if pins.len() > 1 {
                return Err(Error::InvalidInput(format!(
                    "volume '{}' has conflicting datacenter pins: {}",
                    network.name(),
                    pins.iter().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
            dc = pins.into_iter().next();
            if specs.is_empty() && dc.is_none() {
                return Err(Error::InvalidInput(format!(
                    "creating network volume '{}' requires a datacenter when no app placement constraints are available",
                    network.name()
                )));
            }
            if self.volume_datacenters.is_none() {
                let supported = self.api.network_volume_datacenters().await.map_err(|error| {
                    Error::Context {
                        message:format!("cannot determine network volume support for '{}': datacenter catalog lookup failed",network.name()),
                        source:Box::new(error),
                    }
                })?;
                self.volume_datacenters = Some(supported);
            }
            if let Some(dc) = &dc
                && !self
                    .volume_datacenters
                    .as_ref()
                    .is_some_and(|dcs| dcs.contains(dc))
            {
                return Err(Error::InvalidInput(format!(
                    "cannot create network volume '{}' in {dc}: datacenter does not support network volumes",
                    network.name()
                )));
            }
        }
        if !specs.is_empty() {
            let mut keys = Vec::new();
            for spec in specs {
                keys.extend(hardware_keys(spec)?);
            }
            self.stock.fetch(&keys).await;
            let options = PlacementOptions {
                volume_name: network.name().into(),
                volume_datacenters: self.volume_datacenters.clone().unwrap_or_default(),
                volume_dc: if record.is_none() { dc.clone() } else { None },
                existing_dc: if record.is_some() { dc.clone() } else { None },
            };
            // The solver only reads specs; reuse the reference-based implementation.
            dc = Some(super::placement::solve_placement_refs(
                specs,
                &self.stock,
                &options,
            )?);
        }
        let dc = dc.ok_or_else(|| {
            Error::InvalidResponse("network volume placement is missing a datacenter".into())
        })?;
        if record.is_none() {
            record = Some(
                self.api
                    .create_network_volume(network.name(), network.size, &dc)
                    .await?,
            );
            emit(
                self.events.as_ref(),
                "volume_created",
                &[json!(network.name()), json!(network.size), json!(dc)],
            );
        }
        let resolved = ResolvedVolume {
            id: required_str(
                record.as_ref().expect("existing or created volume record"),
                "id",
            )?
            .into(),
            data_center_id: Some(dc),
        };
        self.resolved.insert(key, resolved.clone());
        Ok(resolved)
    }

    /// Resolves explicit mounts against all resources sharing the same volume reference.
    ///
    /// # Errors
    /// Returns mount validation, placement, resolution, or API errors.
    pub async fn resolve_mounts(
        &mut self,
        mounts: &[Mount],
        specs: &[ResourceSpec],
    ) -> Result<Vec<ResolvedMount>> {
        let mut bindings = Vec::new();
        for mount in normalize_mounts(mounts)? {
            let sharing: Vec<_> = specs
                .iter()
                .filter(|spec| {
                    spec.mounts.iter().any(|m| {
                        m.volume.kind() == mount.volume.kind()
                            && m.volume.reference() == mount.volume.reference()
                    })
                })
                .collect();
            let volume = self.resolve_refs(&mount.volume, &sharing).await?;
            bindings.push(ResolvedMount {
                kind: mount.volume.kind().into(),
                reference: mount.volume.reference().into(),
                path: mount.path,
                volume,
            });
        }
        Ok(bindings)
    }
}

fn bind_worker_mounts(payload: &mut Value, bindings: &[ResolvedMount]) -> Result<()> {
    let object = payload
        .as_object_mut()
        .ok_or_else(|| Error::InvalidInput("mount payload must be an object".into()))?;
    let env = object
        .entry("env")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| Error::InvalidInput("worker env must be a list".into()))?;
    env.retain(|entry| entry["key"] != "RUNPOD_MOUNTS");
    env.push(json!({"key":"RUNPOD_MOUNTS","value":serde_json::to_string(bindings)?}));
    Ok(())
}

/// Attaches task volumes and publishes their resolved worker mount bindings.
///
/// # Errors
/// Returns configuration, resolution, placement, or API errors.
pub async fn attach_pod_mounts(
    payload: &mut Value,
    spec: &ResourceSpec,
    resolver: &mut VolumeResolver,
    specs: &[ResourceSpec],
) -> Result<()> {
    spec.validate()?;
    if !payload.is_object() {
        return Err(Error::InvalidInput("pod payload must be an object".into()));
    }
    let bindings = resolver.resolve_mounts(&spec.mounts, specs).await?;
    payload["volumeMounts"]=json!(bindings.iter().map(|b|json!({"volumeId":b.volume.id,"volumeType":if b.kind=="network" {"NETWORK_VOLUME"} else {"OBJECT_STORE_VOLUME"},"mountPath":b.path})).collect::<Vec<_>>());
    for binding in &bindings {
        if let Some(dc) = &binding.volume.data_center_id {
            payload["dataCenterIds"] = json!([dc]);
        }
    }
    bind_worker_mounts(payload, &bindings)
}

/// Attaches endpoint volumes and publishes bindings at the platform's fixed mount path.
///
/// # Errors
/// Returns configuration, resolution, placement, or API errors.
pub async fn attach_endpoint_volumes(
    payload: &mut Value,
    spec: &ResourceSpec,
    resolver: &mut VolumeResolver,
    specs: &[ResourceSpec],
) -> Result<()> {
    spec.validate()?;
    if !payload.is_object() {
        return Err(Error::InvalidInput(
            "endpoint payload must be an object".into(),
        ));
    }
    let bindings = resolver.resolve_mounts(&spec.mounts, specs).await?;
    payload["networkVolumeIds"] = json!(
        bindings
            .iter()
            .filter(|b| b.kind == "network")
            .map(|b| json!({"networkVolumeId":b.volume.id}))
            .collect::<Vec<_>>()
    );
    payload["volumes"] = json!(
        bindings
            .iter()
            .filter(|b| b.kind == "global")
            .map(|b| json!({"volumeId":b.volume.id,"volumeType":"OBJECT_STORE_VOLUME"}))
            .collect::<Vec<_>>()
    );
    for binding in &bindings {
        if let Some(dc) = &binding.volume.data_center_id {
            payload["locations"] = json!(dc);
        }
    }
    let template = payload
        .as_object_mut()
        .expect("payload was checked")
        .entry("template")
        .or_insert_with(|| json!({}));
    bind_worker_mounts(template, &bindings)
}

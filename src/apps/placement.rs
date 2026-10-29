//! Datacenter placement using shared storage constraints and product-specific capacity.
use super::{
    AppsApiClient, DataCenter, GpuGroup, ResourceKind, ResourceSpec, api::Product,
    gpu::resolve_gpu_string,
};
use crate::{Error, Result};
use futures_util::{StreamExt, stream::FuturesUnordered};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Hardware and product identifying a capacity request.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StockKey {
    /// A device and count; None accepts any GPU at that count.
    Gpu {
        /// Exact device name, or no preference.
        id: Option<String>,
        /// GPUs per worker.
        count: u64,
        /// Pod or serverless availability plane.
        product: Product,
    },
    /// CPU instance flavor and size, including its target product.
    Cpu {
        /// Full instance identifier including vCPU count and memory.
        instance_id: String,
        /// Pod or serverless availability plane.
        product: Product,
    },
}

/// Expands resource GPU pools into device queries and retains CPU alternatives.
///
/// # Errors
/// Returns invalid resource configuration errors before querying capacity.
pub fn hardware_keys(spec: &ResourceSpec) -> Result<Vec<StockKey>> {
    spec.validate()?;
    let product = if spec.kind == ResourceKind::Task {
        Product::Pod
    } else {
        Product::Serverless
    };
    if let Some(cpu) = &spec.cpu {
        return Ok(cpu
            .values()
            .iter()
            .map(|instance_id| StockKey::Cpu {
                instance_id: instance_id.clone(),
                product,
            })
            .collect());
    }
    let count = u64::try_from(spec.gpu_count)
        .map_err(|_| Error::InvalidInput("GPU count must be positive".into()))?;
    let gpu = spec.gpu.as_ref().map(|v| v.values()).unwrap_or_default();
    let mut normalized = Vec::new();
    for value in gpu {
        normalized.extend(resolve_gpu_string(value)?);
    }
    if normalized.is_empty() || normalized.iter().any(|v| v.eq_ignore_ascii_case("any")) {
        return Ok(vec![StockKey::Gpu {
            id: None,
            count,
            product,
        }]);
    }
    let mut keys = Vec::new();
    for entry in normalized {
        if let Ok(pool) = entry.parse::<GpuGroup>() {
            keys.extend(pool.device_names().iter().map(|d| StockKey::Gpu {
                id: Some(d.to_string()),
                count,
                product,
            }));
        } else {
            keys.push(StockKey::Gpu {
                id: Some(entry),
                count,
                product,
            });
        }
    }
    Ok(keys)
}

/// Cached advisory capacity signals for a single provisioning run.
#[derive(Debug, Clone)]
pub struct StockMap {
    api: AppsApiClient,
    scores: BTreeMap<(StockKey, String), u8>,
    fetched: BTreeSet<StockKey>,
}
impl StockMap {
    /// Starts with no capacity requests or cached scores.
    pub fn new(api: AppsApiClient) -> Self {
        Self {
            api,
            scores: BTreeMap::new(),
            fetched: BTreeSet::new(),
        }
    }

    /// Fetches previously unseen configurations concurrently across all known datacenters.
    ///
    /// Failed capacity reads become zero scores: stock is advisory. Like Python,
    /// configurations are marked fetched before requests begin, including canceled batches.
    pub async fn fetch(&mut self, keys: &[StockKey]) {
        let mut jobs = FuturesUnordered::new();
        let api = &self.api;
        for key in keys {
            if matches!(key, StockKey::Gpu { id: None, .. }) || !self.fetched.insert(key.clone()) {
                continue;
            }
            for dc in DataCenter::all() {
                let key = key.clone();
                jobs.push(async move {
                    let result = match &key {
                        StockKey::Gpu {
                            id: Some(id),
                            count,
                            product,
                        } => {
                            api.gpu_stock_status(id, dc.as_str(), *count, *product)
                                .await
                        }
                        StockKey::Cpu {
                            instance_id,
                            product,
                        } => {
                            api.cpu_stock_status(instance_id, dc.as_str(), *product)
                                .await
                        }
                        StockKey::Gpu { id: None, .. } => {
                            unreachable!("wildcard stock requires no request")
                        }
                    };
                    let status = match result {
                        Ok(status) => status,
                        Err(error) => {
                            tracing::debug!(%error,datacenter=dc.as_str(),"stock query failed");
                            None
                        }
                    };
                    (key, dc.to_string(), score_status(status.as_deref()))
                });
            }
        }
        while let Some((key, dc, score)) = jobs.next().await {
            self.scores.insert((key, dc), score);
        }
    }

    /// Returns 0–3 capacity, with a wildcard default of 1 when no matching devices were fetched.
    pub fn score(&self, key: &StockKey, dc: &str) -> u8 {
        if let StockKey::Gpu {
            id: None,
            count,
            product,
        } = key
        {
            return self
                .scores
                .iter()
                .filter_map(|((k, location), score)| match k {
                    StockKey::Gpu {
                        count: n,
                        product: p,
                        ..
                    } if n == count && p == product && location == dc => Some(*score),
                    _ => None,
                })
                .max()
                .unwrap_or(1);
        }
        self.scores
            .get(&(key.clone(), dc.into()))
            .copied()
            .unwrap_or(0)
    }
}

fn score_status(status: Option<&str>) -> u8 {
    match status.map(|s| s.trim().to_ascii_uppercase()).as_deref() {
        Some("HIGH") => 3,
        Some("MEDIUM") => 2,
        Some("LOW") => 1,
        _ => 0,
    }
}

/// Finds datacenters with suitable hardware, intersected with explicit resource pins.
///
/// # Errors
/// Returns invalid resource or datacenter errors.
pub fn candidates(spec: &ResourceSpec, stock: &StockMap) -> Result<BTreeSet<String>> {
    let keys = hardware_keys(spec)?;
    let mut allowed: BTreeSet<_> = DataCenter::all().iter().map(ToString::to_string).collect();
    if let Some(pins) = &spec.datacenter
        && !pins.values().is_empty()
    {
        let pins: BTreeSet<_> = pins
            .values()
            .iter()
            .map(|s| s.parse::<DataCenter>().map(|d| d.to_string()))
            .collect::<Result<_>>()?;
        allowed.retain(|dc| pins.contains(dc));
    }
    if !keys.is_empty() {
        allowed.retain(|dc| keys.iter().any(|key| stock.score(key, dc) > 0));
    }
    Ok(allowed)
}

/// Storage capabilities and optional pins constraining a shared volume.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacementOptions {
    /// Volume name used in actionable conflict reports.
    pub volume_name: String,
    /// Datacenters whose backend supports network storage.
    pub volume_datacenters: BTreeSet<String>,
    /// Explicit pin for a new volume.
    pub volume_dc: Option<String>,
    /// Existing volume location, which must never be relocated.
    pub existing_dc: Option<String>,
}

/// Chooses the datacenter maximizing the least available resource, then aggregate capacity.
///
/// Equal ranks choose the lexicographically first datacenter. Existing volumes keep their
/// location even when a different location has higher capacity or supports new storage.
///
/// # Errors
/// Returns resource validation or placement conflicts with per-resource candidate details.
pub fn solve_placement(
    specs: &[ResourceSpec],
    stock: &StockMap,
    options: &PlacementOptions,
) -> Result<String> {
    solve_placement_refs(&specs.iter().collect::<Vec<_>>(), stock, options)
}

pub(crate) fn solve_placement_refs(
    specs: &[&ResourceSpec],
    stock: &StockMap,
    options: &PlacementOptions,
) -> Result<String> {
    let mut per_resource: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for spec in specs {
        let dcs = candidates(spec, stock)?;
        if let Some((_, previous)) = per_resource.iter_mut().find(|(name, _)| *name == spec.name) {
            *previous = dcs;
        } else {
            per_resource.push((&spec.name, dcs));
        }
    }
    if let Some(existing) = &options.existing_dc {
        let existing = existing.parse::<DataCenter>()?.to_string();
        let blocked: Vec<_> = per_resource
            .iter()
            .filter(|(_, dcs)| !dcs.contains(&existing))
            .map(|(name, _)| *name)
            .collect();
        if !blocked.is_empty() {
            return Err(Error::InvalidInput(format!(
                "volume '{}' lives in {existing}, but {} cannot schedule there (no hardware stock or conflicting datacenter pin)",
                options.volume_name,
                blocked.join(", ")
            )));
        }
        return Ok(existing);
    }
    let mut shared = options.volume_datacenters.clone();
    for (_, dcs) in &per_resource {
        shared.retain(|dc| dcs.contains(dc));
    }
    if let Some(pin) = &options.volume_dc {
        shared.retain(|dc| dc == pin);
    }
    if shared.is_empty() {
        let lines = per_resource
            .iter()
            .map(|(name, dcs)| {
                let dcs = if dcs.is_empty() {
                    "(nowhere)".into()
                } else {
                    dcs.iter().cloned().collect::<Vec<_>>().join(", ")
                };
                format!("  {name:<12} schedulable in: {dcs}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let pinned = options
            .volume_dc
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|dc| format!(" in pinned datacenter {dc}"))
            .unwrap_or_default();
        return Err(Error::InvalidInput(format!(
            "cannot place volume '{}': no datacenter can host every resource using it with network volume support{pinned}\n{lines}\nuse separate volumes or compatible hardware",
            options.volume_name
        )));
    }
    let keys: Vec<Vec<StockKey>> = specs
        .iter()
        .map(|spec| hardware_keys(spec))
        .collect::<Result<_>>()?;
    let mut best: Option<(String, (u8, u64))> = None;
    for dc in shared {
        let scores: Vec<u8> = keys
            .iter()
            .map(|ks| ks.iter().map(|k| stock.score(k, &dc)).max().unwrap_or(1))
            .collect();
        let min = scores
            .iter()
            .min()
            .copied()
            .ok_or_else(|| Error::InvalidInput("min() iterable argument is empty".into()))?;
        let rank = (min, scores.iter().map(|v| u64::from(*v)).sum());
        if best.as_ref().is_none_or(|(_, previous)| rank > *previous) {
            best = Some((dc, rank));
        }
    }
    Ok(best
        .expect("nonempty candidate set has a best datacenter")
        .0)
}

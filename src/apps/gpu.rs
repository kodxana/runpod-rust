//! GPU selection, exact-device exclusions, and live catalog membership.
use super::catalog::{GpuGroup, GpuType, pool_for_gpu_type};
use crate::{Client, Error, Result, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

/// Maps pool identifiers to their exact member device identifiers.
pub type PoolMembers = BTreeMap<String, BTreeSet<String>>;

/// An owned live GPU catalog shared by clones, with a sixty-second failure cooldown.
#[derive(Debug, Clone)]
pub struct GpuCatalog {
    client: Client,
    state: Arc<tokio::sync::Mutex<CatalogState>>,
}
#[derive(Debug, Default)]
struct CatalogState {
    members: Option<PoolMembers>,
    failed_at: Option<tokio::time::Instant>,
}
impl GpuCatalog {
    /// Scopes the cache to this client's immutable credentials and service address.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Arc::new(tokio::sync::Mutex::new(CatalogState::default())),
        }
    }

    /// Fetches membership asynchronously, returning None when static fallback is necessary.
    ///
    /// Successful results remain cached; failures may retry after sixty seconds.
    /// Canceling a pending fetch leaves the cache eligible for another attempt.
    pub async fn live_pool_members(&self) -> Option<PoolMembers> {
        let mut state = self.state.lock().await;
        if state.members.is_some()
            || state
                .failed_at
                .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        {
            return state.members.clone();
        }
        let result = self
            .client
            .request(
                &self.client.config.api_base_url,
                "GET",
                "/v2/catalog/gpus",
                None,
                &[],
                Duration::from_secs(10),
            )
            .await
            .and_then(parse_catalog);
        match result {
            Ok(members) => state.members = Some(members),
            Err(error) => {
                state.failed_at = Some(tokio::time::Instant::now());
                tracing::warn!(%error,"gpu catalog unavailable; device pins use the static pool table, which can miss newer devices in a pool");
            }
        }
        state.members.clone()
    }
}

fn parse_catalog(data: Value) -> Result<PoolMembers> {
    let gpus = if data.is_object() {
        data.get("gpus").unwrap_or(&Value::Null)
    } else {
        &data
    };
    let mut members = PoolMembers::new();
    let invalid = || Error::InvalidResponse("catalog lists no pools".into());
    for gpu in gpus.as_array().ok_or_else(invalid)? {
        let pool = gpu
            .get("pool")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let id = gpu
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        if let (Some(pool), Some(id)) = (pool, id) {
            members.entry(pool.into()).or_default().insert(id.into());
        }
    }
    if members.is_empty() {
        Err(invalid())
    } else {
        Ok(members)
    }
}

/// Resolves pool IDs, exact device names, enum names, or shorthand fragments.
///
/// # Errors
/// Returns invalid input when no known GPU matches the value.
pub fn resolve_gpu_string(value: &str) -> Result<Vec<String>> {
    let text = value.trim();
    if text.eq_ignore_ascii_case("any") {
        return Ok(vec!["any".into()]);
    }
    for group in GpuGroup::all() {
        if group.as_str().eq_ignore_ascii_case(text) {
            return Ok(vec![group.to_string()]);
        }
    }
    let upper = text.to_uppercase();
    for gpu in GpuType::all() {
        if gpu.as_str().to_uppercase() == upper
            || gpu.as_str().replace([' ', '-'], "_").to_uppercase() == upper
        {
            return Ok(vec![gpu.to_string()]);
        }
    }
    let fragment = upper.replace('_', " ");
    let matches: Vec<_> = GpuType::all()
        .iter()
        .filter(|gpu| gpu.as_str().to_uppercase().contains(&fragment))
        .map(ToString::to_string)
        .collect();
    if !matches.is_empty() {
        return Ok(matches);
    }
    let pools = GpuGroup::all()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let devices = GpuType::all()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Err(Error::InvalidInput(format!(
        "unknown gpu '{value}'. use a pool id ({pools}), a device name ({devices}), or a fragment like '4090' or 'H100'"
    )))
}
fn device_pool(value: &str) -> Option<GpuGroup> {
    value.parse::<GpuType>().ok().and_then(pool_for_gpu_type)
}

/// Tests whether exact-device pinning needs a live pool catalog to exclude sibling devices.
#[must_use]
pub fn needs_gpu_catalog(gpu: &[String]) -> bool {
    !gpu.is_empty()
        && !gpu.iter().any(|v| v.eq_ignore_ascii_case("any"))
        && gpu
            .iter()
            .any(|v| device_pool(v).is_some_and(|p| !gpu.iter().any(|v| v == p.as_str())))
}

/// Serializes GPU pools and sorted device exclusions with optional live membership.
///
/// Callers fetch live membership explicitly; None uses the pinned static fallback.
pub fn gpu_ids_value(gpu: &[String], pool_members: Option<&PoolMembers>) -> String {
    if gpu.is_empty() || gpu.iter().any(|v| v.eq_ignore_ascii_case("any")) {
        return GpuGroup::all()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
    }
    let mut pools = Vec::<String>::new();
    let mut named = BTreeSet::new();
    let mut devices = BTreeSet::new();
    for entry in gpu {
        let pool = if let Some(pool) = device_pool(entry) {
            devices.insert(entry.as_str());
            pool.to_string()
        } else {
            named.insert(entry.as_str());
            entry.clone()
        };
        if !pools.contains(&pool) {
            pools.push(pool);
        }
    }
    let mut excluded = BTreeSet::new();
    for device in &devices {
        if let Some(pool) = device_pool(device) {
            if named.contains(pool.as_str()) {
                continue;
            }
            let members = pool_members
                .and_then(|m| m.get(pool.as_str()))
                .cloned()
                .unwrap_or_else(|| {
                    pool.device_names()
                        .iter()
                        .map(ToString::to_string)
                        .collect()
                });
            excluded.extend(
                members
                    .into_iter()
                    .filter(|m| !devices.contains(m.as_str())),
            );
        }
    }
    pools.extend(excluded.into_iter().map(|name| format!("-{name}")));
    pools.join(",")
}

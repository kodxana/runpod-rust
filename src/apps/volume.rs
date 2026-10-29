//! Volume references and execution-specific mount bindings.
pub use super::resolution::{
    ResolvedMount, ResolvedVolume, VolumeResolver, attach_endpoint_volumes, attach_pod_mounts,
};
use crate::{Error, Result, Value};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Compute family constraining the available storage backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeType {
    /// CPU-only execution.
    Cpu,
    /// GPU-backed execution.
    Gpu,
}

fn valid_reference(reference: &str) -> Result<()> {
    if reference.trim().is_empty() {
        return Err(Error::InvalidInput(
            "volume reference must be a non-empty string".into(),
        ));
    }
    Ok(())
}

fn deserialize_reference<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    let reference = String::deserialize(deserializer)?;
    valid_reference(&reference).map_err(serde::de::Error::custom)?;
    Ok(reference)
}

/// Named network volume with size and datacenter placement constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkVolume {
    #[serde(deserialize_with = "deserialize_reference")]
    reference: String,
    /// Requested size in GB when creating a volume.
    pub size: u64,
    /// Optional datacenter requirement.
    pub datacenter: Option<String>,
    /// Whether resolution may create a missing volume.
    pub create: bool,
}
impl NetworkVolume {
    /// Creates a reference with 50 GB requested size and creation enabled.
    ///
    /// # Errors
    /// Returns invalid input for a blank reference.
    pub fn new(reference: impl Into<String>) -> Result<Self> {
        let reference = reference.into();
        valid_reference(&reference)?;
        Ok(Self {
            reference,
            size: 50,
            datacenter: None,
            create: true,
        })
    }
    /// Returns the volume name or ID.
    pub fn name(&self) -> &str {
        &self.reference
    }
}
/// Named global object-storage volume independent of datacenter placement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalVolume {
    #[serde(deserialize_with = "deserialize_reference")]
    reference: String,
    /// Whether resolution may create a missing volume.
    pub create: bool,
}
impl GlobalVolume {
    /// Creates a global volume reference with creation enabled.
    ///
    /// # Errors
    /// Returns invalid input for a blank reference.
    pub fn new(reference: impl Into<String>) -> Result<Self> {
        let reference = reference.into();
        valid_reference(&reference)?;
        Ok(Self {
            reference,
            create: true,
        })
    }
    /// Returns the volume name or ID.
    pub fn name(&self) -> &str {
        &self.reference
    }
}
/// Storage backend selection carrying its appropriate reference and options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Volume {
    /// A datacenter-bound network volume.
    Network(NetworkVolume),
    /// A global object-store volume.
    Global(GlobalVolume),
}
impl Volume {
    /// Returns the protocol backend identifier.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Network(_) => "network",
            Self::Global(_) => "global",
        }
    }
    /// Returns the configured name or ID.
    pub fn reference(&self) -> &str {
        match self {
            Self::Network(v) => v.name(),
            Self::Global(v) => v.name(),
        }
    }
    /// Resolves this volume's unique mount in the supplied execution bindings.
    ///
    /// # Errors
    /// Returns invalid input if the volume is unmounted or has multiple mount paths.
    pub fn path<'a>(&self, bindings: &'a MountBindings) -> Result<&'a str> {
        bindings.path(self.kind(), self.reference())
    }
}
impl From<NetworkVolume> for Volume {
    fn from(value: NetworkVolume) -> Self {
        Self::Network(value)
    }
}
impl From<GlobalVolume> for Volume {
    fn from(value: GlobalVolume) -> Self {
        Self::Global(value)
    }
}

/// An explicit absolute mount path and its backing storage reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    /// POSIX worker path, independent of the host operating system.
    pub path: String,
    /// Backing storage resource.
    pub volume: Volume,
}
fn mount_path(path: &str) -> Result<String> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains('\0')
        || path.split('/').any(|s| s == "..")
    {
        return Err(Error::InvalidInput(
            "mount paths must be absolute paths without '..'".into(),
        ));
    }
    Ok(format!(
        "/{}",
        path.split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect::<Vec<_>>()
            .join("/")
    ))
}
/// Normalizes POSIX paths while retaining declaration order and rejecting duplicates.
///
/// # Errors
/// Returns invalid input for relative, traversing, or duplicate mount paths.
pub fn normalize_mounts(mounts: &[Mount]) -> Result<Vec<Mount>> {
    let mut result = Vec::<Mount>::new();
    for mount in mounts {
        let path = mount_path(&mount.path)?;
        valid_reference(mount.volume.reference())?;
        if result.iter().any(|m| m.path == path) {
            return Err(Error::InvalidInput(format!(
                "duplicate mount path '{path}'"
            )));
        }
        result.push(Mount {
            path,
            volume: mount.volume.clone(),
        });
    }
    Ok(result)
}
fn overlaps(a: &str, b: &str) -> bool {
    a == b
        || a == "/"
        || b == "/"
        || a.strip_prefix(b).is_some_and(|s| s.starts_with('/'))
        || b.strip_prefix(a).is_some_and(|s| s.starts_with('/'))
}
/// Checks mount overlap, backend cardinality, and endpoint/CPU capabilities.
///
/// # Errors
/// Returns invalid input for an unsupported storage layout.
pub fn validate_mounts(mounts: &[Mount], kind: &str, compute: ComputeType) -> Result<()> {
    let mounts = normalize_mounts(mounts)?;
    for (index, mount) in mounts.iter().enumerate() {
        if mounts[..index]
            .iter()
            .any(|m| overlaps(&m.path, &mount.path))
        {
            return Err(Error::InvalidInput("mount paths must not overlap".into()));
        }
    }
    for backend in ["network", "global"] {
        if mounts.iter().filter(|m| m.volume.kind() == backend).count() > 1 {
            return Err(Error::InvalidInput(format!(
                "resources support at most one {backend} volume"
            )));
        }
    }
    if kind == "sandbox" && mounts.iter().any(|m| overlaps(&m.path, "/etc/resolv.conf")) {
        return Err(Error::InvalidInput(
            "sandbox mounts must not overlap /etc/resolv.conf".into(),
        ));
    }
    if ["queue", "api"].contains(&kind)
        && !mounts.is_empty()
        && (mounts.len() != 1 || mounts[0].path != "/runpod-volume")
    {
        return Err(Error::InvalidInput(
            "endpoints support one volume mounted at /runpod-volume".into(),
        ));
    }
    if compute == ComputeType::Cpu && mounts.iter().any(|m| matches!(m.volume, Volume::Global(_))) {
        return Err(Error::InvalidInput(
            "global volumes require gpu compute".into(),
        ));
    }
    Ok(())
}

/// Immutable mount lookup for one worker execution, keyed by backend and reference/ID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MountBindings {
    bindings: BTreeMap<(String, String), Vec<String>>,
}
impl MountBindings {
    /// Parses the platform's RUNPOD_MOUNTS payload.
    ///
    /// # Errors
    /// Returns invalid input for malformed binding records or paths.
    pub fn from_value(value: &Value) -> Result<Self> {
        let values = value
            .as_array()
            .ok_or_else(|| Error::InvalidInput("worker mount bindings must be a list".into()))?;
        let mut result = Self::default();
        for binding in values {
            let get = |key: &str| {
                binding
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        Error::InvalidInput(
                            "worker mount bindings require kind, reference, and id".into(),
                        )
                    })
            };
            let kind = get("kind")?;
            let reference = get("reference")?;
            let id = get("id")?;
            if !["network", "global"].contains(&kind) {
                return Err(Error::InvalidInput(
                    "worker mount bindings require kind, reference, and id".into(),
                ));
            }
            let path = mount_path(get("path")?)?;
            for key in [reference, id] {
                let paths = result
                    .bindings
                    .entry((kind.into(), key.into()))
                    .or_default();
                if !paths.contains(&path) {
                    paths.push(path.clone());
                }
            }
        }
        Ok(result)
    }
    /// Reads mount bindings from RUNPOD_MOUNTS, or an empty map when absent.
    ///
    /// # Errors
    /// Returns malformed environment JSON or invalid binding errors.
    pub fn from_env() -> Result<Self> {
        match std::env::var("RUNPOD_MOUNTS")
            .ok()
            .filter(|s| !s.is_empty())
        {
            Some(raw) => Self::from_value(&serde_json::from_str(&raw)?),
            None => Ok(Self::default()),
        }
    }
    /// Resolves the unique path for a volume by its name or ID.
    ///
    /// # Errors
    /// Returns invalid input when no path or multiple paths are bound.
    pub fn path(&self, kind: &str, reference: &str) -> Result<&str> {
        match self
            .bindings
            .get(&(kind.into(), reference.into()))
            .map(Vec::as_slice)
        {
            Some([path]) => Ok(path),
            Some(paths) if paths.len() > 1 => Err(Error::InvalidInput(format!(
                "{kind} volume '{reference}' has multiple mounts; use an explicit mount path"
            ))),
            _ => Err(Error::InvalidInput(format!(
                "{kind} volume '{reference}' is not mounted in the current execution"
            ))),
        }
    }
}

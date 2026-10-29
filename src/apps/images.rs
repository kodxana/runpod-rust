//! Runtime image selection for the pinned Python-compatible remote runners.
use super::{ResourceKind, ResourceSpec, volume::ComputeType};
use crate::{Error, Result};

/// Runtime image versions supported by the pinned SDK.
pub const SUPPORTED_PYTHON_VERSIONS: &[&str] = &["3.10", "3.11", "3.12", "3.13", "3.14"];
/// Default remote interpreter version; Rust does not require a local Python interpreter.
pub const DEFAULT_PYTHON_VERSION: &str = "3.12";

/// Reads the runtime channel override, including an explicitly empty value.
#[must_use]
pub fn runtime_tag() -> String {
    std::env::var("RUNPOD_RUNTIME_TAG").unwrap_or_else(|_| "latest".into())
}

/// Explicit remote interpreter/channel selection, replacing local Python introspection.
#[derive(Debug, Clone)]
pub struct RuntimeImages {
    /// Remote Python version, defaulting to 3.12. Empty also selects the default.
    pub python_version: String,
    /// Image channel or pinned version suffix.
    pub tag: String,
}
impl Default for RuntimeImages {
    fn default() -> Self {
        Self {
            python_version: DEFAULT_PYTHON_VERSION.into(),
            tag: runtime_tag(),
        }
    }
}
impl RuntimeImages {
    /// Selects a built-in image for the execution mode and hardware.
    ///
    /// # Errors
    /// Returns invalid input for an unsupported interpreter version.
    pub fn runtime_image(&self, kind: ResourceKind, compute: ComputeType) -> Result<String> {
        let version = if self.python_version.is_empty() {
            DEFAULT_PYTHON_VERSION
        } else {
            &self.python_version
        };
        if !SUPPORTED_PYTHON_VERSIONS.contains(&version) {
            return Err(Error::InvalidInput(format!(
                "python {version} is not supported by the runtime images (supported: {})",
                SUPPORTED_PYTHON_VERSIONS.join(", ")
            )));
        }
        let kind = match kind {
            ResourceKind::Queue => "queue",
            ResourceKind::Api => "api",
            ResourceKind::Task => "task",
        };
        let suffix = match compute {
            ComputeType::Gpu => "-gpu",
            ComputeType::Cpu => "",
        };
        Ok(format!("runpod/{kind}{suffix}:py{version}-{}", self.tag))
    }

    /// Uses a custom image when present, otherwise selects the matching built-in image.
    ///
    /// # Errors
    /// Returns invalid input for an unsupported version when using a built-in image.
    pub fn image_for_spec(&self, spec: &ResourceSpec) -> Result<String> {
        match spec.image.as_ref().filter(|s| !s.is_empty()) {
            Some(image) => Ok(image.clone()),
            None => self.runtime_image(
                spec.kind,
                if spec.is_cpu() {
                    ComputeType::Cpu
                } else {
                    ComputeType::Gpu
                },
            ),
        }
    }
}

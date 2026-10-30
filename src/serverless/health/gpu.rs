use super::{HealthError, ProbeCommand, integer};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

/// Parsed `gpu_test` output; process exit status is deliberately not part of success.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuTestOutput {
    /// At least one GPU passed, all enumerated GPUs passed, and no error line appeared.
    pub success: bool,
    /// Number of lines reporting a successful allocation.
    pub gpu_count: usize,
    /// Last successfully parsed "Found N GPUs" count, including negative source values.
    pub found_gpus: i64,
    /// Complete error lines in output order.
    pub errors: Vec<String>,
    /// CUDA driver and kernel version text, when present.
    pub details: BTreeMap<String, String>,
}

/// Parse allocation results and metadata, retaining Python's case and error-word rules.
pub fn parse_gpu_test_output(output: &str) -> GpuTestOutput {
    let mut result = GpuTestOutput::default();
    for line in output
        .trim()
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(value) = line.strip_prefix("CUDA Driver Version:") {
            result
                .details
                .insert("cuda_version".into(), value.trim().into());
        } else if let Some(value) = line.strip_prefix("Linux Kernel Version:") {
            result.details.insert("kernel".into(), value.trim().into());
        } else if line.starts_with("Found")
            && line.contains("GPUs")
            && let Some(count) = line.split_whitespace().nth(1).and_then(|v| v.parse().ok())
        {
            result.found_gpus = count;
        }
        let lower = line.to_lowercase();
        if lower.contains("memory allocation test passed") {
            result.gpu_count += 1;
        }
        if ["failed", "error", "cannot", "unable"]
            .iter()
            .any(|word| lower.contains(word))
        {
            result.errors.push(line.into());
        }
    }
    result.success = result.gpu_count > 0
        && i64::try_from(result.gpu_count).ok() == Some(result.found_gpus)
        && result.errors.is_empty();
    result
}

/// Time limits and error-line truncation for native GPU probes.
#[derive(Clone, Debug)]
pub struct GpuConfig {
    /// Deadline for the allocation binary, default thirty seconds.
    pub test_timeout: Duration,
    /// Deadline for the nvidia-smi fallback, default ten seconds.
    pub fallback_timeout: Duration,
    /// Python slice endpoint for reported error lines; negative values omit trailing lines.
    pub max_error_messages: i64,
}
impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            test_timeout: Duration::from_secs(30),
            fallback_timeout: Duration::from_secs(10),
            max_error_messages: 10,
        }
    }
}
impl GpuConfig {
    /// Parse `RUNPOD_GPU_TEST_TIMEOUT` and `RUNPOD_GPU_MAX_ERROR_MESSAGES` from owned values.
    ///
    /// # Errors
    /// Returns Configuration for a value that is not a signed 64-bit integer.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self, HealthError> {
        let seconds = integer(values, "RUNPOD_GPU_TEST_TIMEOUT", 30)?;
        Ok(Self {
            test_timeout: Duration::from_secs(seconds.max(0).cast_unsigned()),
            max_error_messages: integer(values, "RUNPOD_GPU_MAX_ERROR_MESSAGES", 10)?,
            ..Self::default()
        })
    }
}

/// Native allocation testing with a bounded nvidia-smi fallback.
#[derive(Clone, Debug)]
pub struct GpuChecks {
    config: GpuConfig,
    binary: Option<ProbeCommand>,
    nvidia_smi: ProbeCommand,
    _binary_file: Option<std::sync::Arc<tempfile::TempPath>>,
}
impl GpuChecks {
    /// Use explicit executables, including None to request the fallback directly.
    pub fn with_commands(
        config: GpuConfig,
        binary: Option<ProbeCommand>,
        nvidia_smi: ProbeCommand,
    ) -> Self {
        Self {
            config,
            binary,
            nvidia_smi,
            _binary_file: None,
        }
    }
    /// Configure probes from explicit environment values and the bundled native resource.
    ///
    /// # Errors
    /// Returns malformed settings or an error extracting the packaged allocation probe.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self, HealthError> {
        Self::with_environment(GpuConfig::from_values(values)?, values)
    }
    pub(super) fn with_environment(
        config: GpuConfig,
        values: &BTreeMap<String, String>,
    ) -> Result<Self, HealthError> {
        let override_path = values
            .get("RUNPOD_BINARY_GPU_TEST_PATH")
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_file());
        let checks = Self::with_commands(
            config,
            override_path.map(ProbeCommand::new),
            ProbeCommand::new("nvidia-smi"),
        );
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let checks = {
            let mut checks = checks;
            if checks.binary.is_none() {
                use std::{io::Write, os::unix::fs::PermissionsExt};
                let extract = || -> std::io::Result<tempfile::TempPath> {
                    let mut file = tempfile::Builder::new()
                        .prefix("runpod-gpu-test-")
                        .tempfile()?;
                    file.write_all(include_bytes!("../../../binaries/gpu_test"))?;
                    file.as_file()
                        .set_permissions(std::fs::Permissions::from_mode(0o700))?;
                    Ok(file.into_temp_path())
                };
                let file = extract().map_err(|source| HealthError::Io {
                    operation: "extract bundled gpu_test",
                    source,
                })?;
                checks.binary = Some(ProbeCommand::new(file.to_path_buf()));
                checks._binary_file = Some(std::sync::Arc::new(file));
            }
            checks
        };
        Ok(checks)
    }
    /// Detect a usable GPU without initializing a CUDA context in this process.
    pub async fn is_available(&self) -> bool {
        match self
            .nvidia_smi
            .output(&[], Duration::from_secs(5), "nvidia-smi")
            .await
        {
            Ok(output) => {
                output.status.success()
                    && std::str::from_utf8(&output.stdout).is_ok_and(|s| s.contains("NVIDIA-SMI"))
            }
            Err(HealthError::Timeout { .. }) => {
                tracing::warn!(
                    "nvidia-smi did not respond within 5s; treating worker as having no usable GPU"
                );
                false
            }
            Err(_) => false,
        }
    }
    /// Execute the allocation binary and classify failures before considering fallback.
    ///
    /// # Errors
    /// Returns [`HealthError::GpuAllocation`] when devices were reached; other errors permit fallback.
    pub async fn run_gpu_test_binary(&self) -> Result<GpuTestOutput, HealthError> {
        let binary = self.binary.as_ref().ok_or_else(|| HealthError::Io {
            operation: "gpu_test binary not found in package",
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        })?;
        let output = binary
            .output(&[], self.config.test_timeout, "GPU test binary")
            .await?;
        let result = parse_gpu_test_output(&String::from_utf8_lossy(&output.stdout));
        if result.success {
            return Ok(result);
        }
        let mut message = "GPU memory allocation test failed".to_owned();
        if !result.errors.is_empty() {
            let end = if self.config.max_error_messages >= 0 {
                usize::try_from(self.config.max_error_messages)
                    .unwrap_or(usize::MAX)
                    .min(result.errors.len())
            } else {
                result.errors.len().saturating_sub(
                    usize::try_from(self.config.max_error_messages.unsigned_abs())
                        .unwrap_or(usize::MAX),
                )
            };
            message.push_str(": ");
            message.push_str(&result.errors[..end].join("; "));
        }
        if result.found_gpus != 0 {
            Err(HealthError::GpuAllocation(message))
        } else {
            Err(HealthError::Check(format!(
                "GPU test binary execution failed: {message}"
            )))
        }
    }
    /// Validate that nvidia-smi lists at least one nonempty line; no allocation is tested.
    ///
    /// # Errors
    /// Returns a missing tool, timeout, nonzero exit, or empty-list failure.
    pub async fn run_gpu_test_fallback(&self) -> Result<(), HealthError> {
        let output = self
            .nvidia_smi
            .output(&["--list-gpus"], self.config.fallback_timeout, "nvidia-smi")
            .await
            .map_err(|error| {
                let message = match &error {
                    HealthError::Io { source, .. }
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        "nvidia-smi not found. Cannot validate GPU availability.".into()
                    }
                    HealthError::Timeout { .. } => "nvidia-smi timed out".into(),
                    _ => format!("nvidia-smi fallback check failed: {error}"),
                };
                HealthError::Context {
                    message,
                    source: Box::new(error),
                }
            })?;
        if !output.status.success() {
            return Err(HealthError::Check(format!(
                "nvidia-smi --list-gpus failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        if !String::from_utf8_lossy(&output.stdout)
            .split('\n')
            .any(|line| !line.trim().is_empty())
        {
            return Err(HealthError::Check("No GPUs detected by nvidia-smi".into()));
        }
        Ok(())
    }
    /// Prefer allocation testing; fall back only when no device allocation failure was found.
    ///
    /// # Errors
    /// Returns an allocation failure immediately or a composite error if both probes fail.
    pub async fn check_gpu_health(&self) -> Result<(), HealthError> {
        let error = match self.run_gpu_test_binary().await {
            Ok(_) => return Ok(()),
            Err(e @ HealthError::GpuAllocation(_)) => return Err(e),
            Err(e) => e,
        };
        let absent = matches!(&error,HealthError::Io {source,..} if matches!(source.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::PermissionDenied));
        self.run_gpu_test_fallback().await.map_err(|source| {
            let message = if absent {
                format!(
                    "GPU health check failed (binary disabled/missing, fallback failed): {source}"
                )
            } else {
                format!("GPU health check failed. Binary test: {error}. Fallback test: {source}")
            };
            HealthError::Context {
                message,
                source: Box::new(source),
            }
        })
    }
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn bundled_binary_is_executable_owned_and_shared_until_last_drop() {
        let checks = GpuChecks::from_values(&BTreeMap::new()).unwrap();
        let path = checks.binary.as_ref().unwrap().program.clone();
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"\x7fELF");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let clone = checks.clone();
        drop(checks);
        assert!(path.exists());
        drop(clone);
        assert!(!path.exists());
    }
}

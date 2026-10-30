use super::{HealthError, ProbeCommand, integer};
use std::{collections::BTreeMap, fmt, path::Path, sync::OnceLock, time::Duration};

/// Parse the first major.minor pair, returning (0, 0) when no pair is present.
pub fn parse_version(value: &str) -> (u64, u64) {
    static VERSION: OnceLock<regex::Regex> = OnceLock::new();
    VERSION
        .get_or_init(|| regex::Regex::new(r"([0-9]+)\.([0-9]+)").expect("valid version pattern"))
        .captures(value)
        .and_then(|c| Some((c[1].parse().ok()?, c[2].parse().ok()?)))
        .unwrap_or((0, 0))
}
/// Extract only the host and port for a TCP probe, falling back on malformed worker URLs.
pub fn network_probe_target(value: Option<&str>) -> (String, u16) {
    let parsed = value.and_then(|value| reqwest::Url::parse(value).ok());
    if let Some(url) = parsed
        && matches!(url.scheme(), "http" | "https")
        && let Some(host) = url.host_str()
    {
        let fallback = if url.scheme() == "https" { 443 } else { 80 };
        return (
            host.trim_start_matches('[').trim_end_matches(']').into(),
            url.port().filter(|p| *p != 0).unwrap_or(fallback),
        );
    }
    ("api.runpod.ai".into(), 443)
}

/// Memory values in binary gigabytes, matching the Python health payload.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MemoryInfo {
    /// Total physical memory in GiB.
    pub total_gb: f64,
    /// Memory available to new work in GiB.
    pub available_gb: f64,
    /// Percentage unavailable, from zero to one hundred on ordinary host readings.
    pub used_percent: f64,
}
impl MemoryInfo {
    /// Parse the Linux fallback format, with missing memory fields defaulting to zero.
    ///
    /// # Errors
    /// Returns a check failure for a malformed line or non-integer value.
    pub fn from_meminfo(text: &str) -> Result<Self, HealthError> {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            let (key, value) = line.split_once(':').ok_or_else(|| {
                HealthError::Check("Failed to read memory info: missing colon".into())
            })?;
            let value = value
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<i64>().ok())
                .ok_or_else(|| {
                    HealthError::Check("Failed to read memory info: invalid integer".into())
                })?;
            values.insert(key.trim(), value);
        }
        let total_gb = *values.get("MemTotal").unwrap_or(&0) as f64 / 1024.0_f64.powi(2);
        let available_gb = *values.get("MemAvailable").unwrap_or(&0) as f64 / 1024.0_f64.powi(2);
        Ok(Self {
            total_gb,
            available_gb,
            used_percent: if total_gb > 0.0 {
                100.0 * (1.0 - available_gb / total_gb)
            } else {
                0.0
            },
        })
    }
}
/// Available and total bytes on the filesystem containing a requested path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskInfo {
    /// Total filesystem capacity.
    pub total_bytes: u64,
    /// Free space available to the calling process.
    pub free_bytes: u64,
}

/// Owned system health thresholds and worker API address.
#[derive(Clone)]
pub struct SystemConfig {
    /// Minimum available memory, default 4 GiB.
    pub min_memory_gb: f64,
    /// Minimum root-filesystem free percentage, default 10 percent.
    pub min_disk_percent: f64,
    /// Minimum CUDA major.minor version, default 11.8.
    pub min_cuda_version: String,
    /// Total TCP readiness budget, default five seconds.
    pub network_timeout: Duration,
    /// Compute benchmark budget, used by framework integration.
    pub benchmark_timeout: Duration,
    /// Deadline for each version-detection process, default five seconds.
    pub cuda_probe_timeout: Duration,
    /// Worker API URL used only to select the probe host and port.
    pub job_url: Option<String>,
}
impl fmt::Debug for SystemConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemConfig")
            .field("min_memory_gb", &self.min_memory_gb)
            .field("min_disk_percent", &self.min_disk_percent)
            .field("min_cuda_version", &self.min_cuda_version)
            .field("network_timeout", &self.network_timeout)
            .finish_non_exhaustive()
    }
}
impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            min_memory_gb: 4.0,
            min_disk_percent: 10.0,
            min_cuda_version: "11.8".into(),
            network_timeout: Duration::from_secs(5),
            benchmark_timeout: Duration::from_secs(2),
            cuda_probe_timeout: Duration::from_secs(5),
            job_url: None,
        }
    }
}
impl SystemConfig {
    /// Read the current RUNPOD health tuning values without changing process environment.
    ///
    /// # Errors
    /// Returns malformed numeric values or nonpositive network/benchmark timeouts.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self, HealthError> {
        let number = |name: &str, default: f64| {
            values.get(name).map_or(Ok(default), |v| {
                v.trim().parse().map_err(|_| HealthError::Configuration {
                    variable: name.into(),
                })
            })
        };
        let timeout = |name: &str, default| {
            let n = integer(values, name, default)?;
            if n <= 0 {
                return Err(HealthError::Configuration {
                    variable: name.into(),
                });
            }
            Ok(Duration::from_secs(n.cast_unsigned()))
        };
        Ok(Self {
            min_memory_gb: number("RUNPOD_MIN_MEMORY_GB", 4.0)?,
            min_disk_percent: number("RUNPOD_MIN_DISK_PERCENT", 10.0)?,
            min_cuda_version: values
                .get("RUNPOD_MIN_CUDA_VERSION")
                .cloned()
                .unwrap_or_else(|| "11.8".into()),
            network_timeout: timeout("RUNPOD_NETWORK_CHECK_TIMEOUT", 5)?,
            benchmark_timeout: timeout("RUNPOD_GPU_BENCHMARK_TIMEOUT", 2)?,
            job_url: values.get("RUNPOD_WEBHOOK_GET_JOB").cloned(),
            ..Self::default()
        })
    }
}

/// System probes that leave CUDA initialization to the handler's existing GPU framework.
#[derive(Clone, Debug)]
pub struct SystemChecks {
    pub(super) config: SystemConfig,
    nvcc: ProbeCommand,
    nvidia_smi: ProbeCommand,
}
impl SystemChecks {
    /// Use nvcc and nvidia-smi from PATH; constructing checks performs no probes.
    pub fn new(config: SystemConfig) -> Self {
        Self::with_commands(
            config,
            ProbeCommand::new("nvcc"),
            ProbeCommand::new("nvidia-smi"),
        )
    }
    /// Select explicit CUDA executables, including fixed arguments and child environment.
    pub fn with_commands(
        config: SystemConfig,
        nvcc: ProbeCommand,
        nvidia_smi: ProbeCommand,
    ) -> Self {
        Self {
            config,
            nvcc,
            nvidia_smi,
        }
    }
    /// Read native OS memory counters without enumerating processes or refreshing CPU data.
    pub fn memory_info() -> MemoryInfo {
        let mut system = sysinfo::System::new();
        system.refresh_memory_specifics(sysinfo::MemoryRefreshKind::nothing().with_ram());
        let total = system.total_memory() as f64;
        let available = system.available_memory() as f64;
        MemoryInfo {
            total_gb: total / 1024.0_f64.powi(3),
            available_gb: available / 1024.0_f64.powi(3),
            used_percent: if total > 0.0 {
                ((1.0 - available / total) * 1000.0).round() / 10.0
            } else {
                0.0
            },
        }
    }
    /// Read space available to the process on the filesystem containing path.
    ///
    /// # Errors
    /// Returns the underlying OS error if the filesystem cannot be queried.
    pub fn disk_info(path: impl AsRef<Path>) -> Result<DiskInfo, HealthError> {
        let info = fs2::statvfs(path).map_err(|source| HealthError::Io {
            operation: "Could not check disk space",
            source,
        })?;
        Ok(DiskInfo {
            total_bytes: info.total_space(),
            free_bytes: info.available_space(),
        })
    }
    /// Verify current available memory against the configured threshold.
    ///
    /// # Errors
    /// Returns insufficient memory with observed and required amounts.
    pub fn check_memory_availability(&self) -> Result<(), HealthError> {
        self.check_memory_info(Self::memory_info())
    }
    /// Apply the memory threshold to an observed snapshot.
    ///
    /// # Errors
    /// Returns insufficient memory when `available_gb` is strictly below the threshold.
    pub fn check_memory_info(&self, info: MemoryInfo) -> Result<(), HealthError> {
        if info.available_gb < self.config.min_memory_gb {
            return Err(HealthError::Check(format!(
                "Insufficient memory: {:.2}GB available, {:?}GB required",
                info.available_gb, self.config.min_memory_gb
            )));
        }
        tracing::info!(
            "Memory check passed: {:.2}GB available (of {:.2}GB total)",
            info.available_gb,
            info.total_gb
        );
        Ok(())
    }
    /// Verify free space on the root filesystem.
    ///
    /// # Errors
    /// Returns an OS query failure or insufficient disk space.
    pub fn check_disk_space(&self) -> Result<(), HealthError> {
        self.check_disk_info(Self::disk_info("/")?)
    }
    /// Apply the disk threshold to an observed snapshot, treating zero total as zero percent.
    ///
    /// # Errors
    /// Returns insufficient disk space when free percentage is below the threshold.
    pub fn check_disk_info(&self, info: DiskInfo) -> Result<(), HealthError> {
        let total = info.total_bytes as f64;
        let free = info.free_bytes as f64;
        let percent = if total > 0.0 {
            100.0 * free / total
        } else {
            0.0
        };
        if percent < self.config.min_disk_percent {
            return Err(HealthError::Check(format!(
                "Insufficient disk space: {:.2}GB free ({percent:.1}%), {:?}% required",
                free / 1024.0_f64.powi(3),
                self.config.min_disk_percent
            )));
        }
        tracing::info!(
            "Disk space check passed: {:.2}GB free ({percent:.1}% available)",
            free / 1024.0_f64.powi(3)
        );
        Ok(())
    }
    /// Attempt TCP readiness three times within one budget, without sending API requests.
    ///
    /// # Errors
    /// Returns a bounded connection failure containing only the selected host and port.
    pub async fn check_network_connectivity(&self) -> Result<(), HealthError> {
        let (host, port) = network_probe_target(self.config.job_url.as_deref());
        probe_network(&host, port, self.config.network_timeout, || async {
            let mut stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
            tokio::io::AsyncWriteExt::shutdown(&mut stream).await
        })
        .await
    }
    /// Try nvcc first, then parse the CUDA field from nvidia-smi; probe failures are skipped.
    pub async fn cuda_version(&self) -> Option<String> {
        if let Ok(output) = self
            .nvcc
            .output(&["--version"], self.config.cuda_probe_timeout, "nvcc")
            .await
            && output.status.success()
        {
            for line in String::from_utf8_lossy(&output.stdout).split('\n') {
                let lower = line.to_lowercase();
                if lower.contains("release") || lower.contains("version") {
                    return Some(line.trim().into());
                }
            }
        }
        if let Ok(output) = self
            .nvidia_smi
            .output(&[], self.config.cuda_probe_timeout, "nvidia-smi")
            .await
            && output.status.success()
        {
            for line in String::from_utf8_lossy(&output.stdout).split('\n') {
                if let Some((_, rest)) = line.split_once("CUDA Version:") {
                    return rest
                        .split_whitespace()
                        .next()
                        .map(|v| format!("CUDA Version: {v}"));
                }
            }
        }
        None
    }
    /// Check the discovered CUDA version; absence of version information is a permitted skip.
    ///
    /// # Errors
    /// Returns a check failure when the parsed version is below the minimum.
    pub async fn check_cuda_versions(&self) -> Result<(), HealthError> {
        if let Some(version) = self.cuda_version().await {
            let found = parse_version(&version);
            let min = parse_version(&self.config.min_cuda_version);
            if found < min {
                return Err(HealthError::Check(format!(
                    "CUDA version too old: {}.{} found, {}.{} required",
                    found.0, found.1, min.0, min.1
                )));
            }
        }
        Ok(())
    }
}

async fn probe_network<F, Fut>(
    host: &str,
    port: u16,
    budget: Duration,
    mut probe: F,
) -> Result<(), HealthError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::io::Result<()>>,
{
    let start = tokio::time::Instant::now();
    let mut last = "Timeout".to_owned();
    for attempt in 0..3 {
        let remaining = budget.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining / (3 - attempt), probe()).await {
            Ok(Ok(())) => {
                tracing::info!(%host,port,"Network connectivity passed");
                return Ok(());
            }
            Err(_) => last = "Timeout".into(),
            Ok(Err(error)) => {
                last = if error.kind() == std::io::ErrorKind::ConnectionRefused {
                    "Connection refused".into()
                } else {
                    format!("{:?}", error.kind())
                }
            }
        }
        if attempt < 2 {
            tokio::time::sleep(
                Duration::from_millis(100 * u64::from(attempt + 1))
                    .min(budget.saturating_sub(start.elapsed())),
            )
            .await;
        }
    }
    Err(HealthError::Check(format!(
        "Network connectivity failed: {last} connecting to {host}:{port} after bounded retries ({}s budget)",
        budget.as_secs_f64()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn network_retries_refusal_and_can_succeed_on_the_next_attempt() {
        let mut calls = 0;
        probe_network("worker", 8443, Duration::from_secs(5), || {
            calls += 1;
            let call = calls;
            async move {
                if call == 1 {
                    Err(std::io::ErrorKind::ConnectionRefused.into())
                } else {
                    Ok(())
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(calls, 2);
        calls = 0;
        let error = probe_network("worker", 443, Duration::from_secs(5), || {
            calls += 1;
            async { Err(std::io::ErrorKind::ConnectionRefused.into()) }
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 3);
        assert!(error.to_string().contains("Connection refused"));
    }
    #[tokio::test(start_paused = true)]
    async fn network_probe_and_teardown_share_one_bounded_budget() {
        let start = tokio::time::Instant::now();
        let mut calls = 0;
        struct Closed(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for Closed {
            fn drop(&mut self) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let closed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let error = probe_network("worker", 443, Duration::from_secs(1), || {
            calls += 1;
            let closed = closed.clone();
            async move {
                let _guard = Closed(closed);
                std::future::pending::<std::io::Result<()>>().await
            }
        })
        .await
        .unwrap_err();
        assert!(calls <= 3);
        assert!(calls >= 2);
        assert_eq!(closed.load(std::sync::atomic::Ordering::SeqCst), calls);
        assert!(start.elapsed() <= Duration::from_secs(1));
        assert!(error.to_string().contains("Timeout"));
    }
}

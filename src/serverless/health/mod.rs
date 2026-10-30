//! Native startup fitness checks, hardware probes, and unhealthy worker reporting.
mod environment;
mod fitness;
mod framework;
mod gpu;
mod process;
mod reporting;
mod system;

pub use environment::{get_binary_path, is_early_check_eligible, is_serverless_environment};
pub use fitness::{
    EARLY_CHECKS_DONE_ENV, EarlyCheckStatus, FitnessCheck, FitnessChecks, FitnessFailure,
    FitnessPass,
};
pub use framework::{GpuFramework, GpuFrameworks};
pub use gpu::{GpuChecks, GpuConfig, GpuTestOutput, parse_gpu_test_output};
pub use process::ProbeCommand;
pub use reporting::{report_unhealthy, terminate_unhealthy};
pub use system::{
    DiskInfo, MemoryInfo, SystemChecks, SystemConfig, network_probe_target, parse_version,
};

/// A health failure that retains whether a GPU allocation test reached the hardware.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HealthError {
    /// A device was enumerated but failed its memory allocation test. Never fall back.
    #[error("{0}")]
    GpuAllocation(String),
    /// A readiness check failed without a more specific native error.
    #[error("{0}")]
    Check(String),
    /// A tuning variable could not be parsed.
    #[error("invalid fitness setting: {variable}")]
    Configuration { variable: String },
    /// An operating-system operation failed, retaining its original cause.
    #[error("{operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// A bounded health probe exceeded its time budget.
    #[error("{operation} timed out after {seconds}s")]
    Timeout {
        operation: &'static str,
        seconds: f64,
    },
    /// Context for a fallback or nested health failure.
    #[error("{message}")]
    Context {
        message: String,
        #[source]
        source: Box<HealthError>,
    },
}

pub(crate) fn integer(
    values: &std::collections::BTreeMap<String, String>,
    name: &str,
    default: i64,
) -> Result<i64, HealthError> {
    values.get(name).map_or(Ok(default), |value| {
        value
            .trim()
            .parse()
            .map_err(|_| HealthError::Configuration {
                variable: name.into(),
            })
    })
}

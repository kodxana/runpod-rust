use super::{GpuChecks, GpuConfig, HealthError, SystemChecks};
use std::{fmt, sync::Arc, time::Instant};

/// Adapter for a GPU framework the handler already owns; no runtime is loaded implicitly.
/// Use `HealthError::Check` for runtime/compute failures and other variants for setup failures.
pub trait GpuFramework: fmt::Debug + Send + Sync {
    /// Whether the handler has already initialized a CUDA context; queried before availability.
    fn is_initialized(&self) -> bool;
    /// The framework's CUDA availability, queried only after initialization is confirmed.
    ///
    /// # Errors
    /// Return the framework's device-query or runtime-initialization failure.
    fn is_available(&self) -> Result<bool, HealthError>;
    /// Reset the framework's peak-memory counters without creating a new context.
    ///
    /// # Errors
    /// Return a failure to access or reset the existing CUDA context.
    fn reset_peak_memory_stats(&self) -> Result<(), HealthError>;
    /// Wait until GPU operations complete.
    ///
    /// # Errors
    /// Return asynchronous device errors or synchronization failures.
    fn synchronize(&self) -> Result<(), HealthError>;
    /// Number of accessible devices.
    ///
    /// # Errors
    /// Return device-enumeration or runtime-query failures.
    fn device_count(&self) -> Result<usize, HealthError>;
    /// Total memory of one device; used by the Torch-style check.
    ///
    /// # Errors
    /// Return an invalid-device or property-query failure.
    fn device_memory(&self, index: usize) -> Result<u64, HealthError>;
    /// Select the active device; used by the CuPy-style check and intentionally not restored.
    ///
    /// # Errors
    /// Return a failure to activate the requested device.
    fn select_device(&self, index: usize) -> Result<(), HealthError>;
    /// Allocate a zero-filled vector, retaining it until the subsequent synchronization completes.
    ///
    /// # Errors
    /// Return an allocation or device-access failure, including insufficient memory.
    fn allocate_zeros(&self, index: usize, elements: usize) -> Result<(), HealthError>;
    /// Allocate and multiply two random square matrices, retaining their buffers through synchronization.
    ///
    /// # Errors
    /// Return allocation/compute failures as `HealthError::Check`; other setup errors may allow fallback.
    fn matrix_multiply(&self, size: usize) -> Result<(), HealthError>;
}

/// Explicit handles replacing Python's lookup of frameworks already imported by the handler.
#[derive(Clone, Debug, Default)]
pub struct GpuFrameworks {
    torch: Option<Arc<dyn GpuFramework>>,
    cupy: Option<Arc<dyn GpuFramework>>,
}
impl GpuFrameworks {
    /// Supply a Torch-style runtime; its initialized-context gate takes precedence over fallback.
    pub fn set_torch(&mut self, framework: impl GpuFramework + 'static) {
        self.torch = Some(Arc::new(framework));
    }
    /// Supply a CuPy-style fallback runtime, used when Torch is absent or benchmark setup fails.
    pub fn set_cupy(&mut self, framework: impl GpuFramework + 'static) {
        self.cupy = Some(Arc::new(framework));
    }
    /// Whether a handler has explicitly supplied either runtime.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.torch.is_some() || self.cupy.is_some()
    }
}
fn context(prefix: &str, source: HealthError) -> HealthError {
    HealthError::Context {
        message: format!("{prefix}: {source}"),
        source: Box::new(source),
    }
}
fn torch_init(framework: &dyn GpuFramework) -> Result<(), HealthError> {
    if !framework.is_initialized() || !framework.is_available()? {
        return Ok(());
    }
    framework.reset_peak_memory_stats()?;
    framework.synchronize()?;
    let count = framework.device_count()?;
    if count == 0 {
        return Err(HealthError::Check(
            "No CUDA devices available despite cuda.is_available() being True".into(),
        ));
    }
    for index in 0..count {
        let result = (|| {
            if framework.device_memory(index)? == 0 {
                return Err(HealthError::Check(format!(
                    "GPU {index} reports zero memory"
                )));
            }
            framework.allocate_zeros(index, 1024)?;
            framework.synchronize()
        })();
        result.map_err(|error| context(&format!("Failed to initialize GPU {index}"), error))?;
    }
    tracing::info!("CUDA initialization passed: {count} device(s) initialized successfully");
    Ok(())
}
fn cupy_init(framework: &dyn GpuFramework) -> Result<(), HealthError> {
    framework.synchronize()?;
    let count = framework.device_count()?;
    if count == 0 {
        return Err(HealthError::Check(
            "No CUDA devices available via CuPy".into(),
        ));
    }
    for index in 0..count {
        let result = (|| {
            framework.select_device(index)?;
            framework.allocate_zeros(index, 1024)?;
            framework.synchronize()
        })();
        result.map_err(|error| {
            context(
                &format!("Failed to initialize GPU {index} with CuPy"),
                error,
            )
        })?;
    }
    tracing::info!("CUDA initialization passed: {count} device(s) initialized successfully");
    Ok(())
}
fn benchmark(
    framework: &dyn GpuFramework,
    maximum: std::time::Duration,
) -> Result<(), HealthError> {
    let started = Instant::now();
    framework.matrix_multiply(1024)?;
    framework.synchronize()?;
    let elapsed = started.elapsed();
    let milliseconds = elapsed.as_secs_f64() * 1000.0;
    if elapsed > maximum {
        return Err(HealthError::Check(format!(
            "GPU compute too slow: Matrix multiply took {milliseconds:.0}ms (max: {:.0}ms)",
            maximum.as_secs_f64() * 1000.0
        )));
    }
    tracing::info!(
        "GPU compute benchmark passed: Matrix multiply completed in {milliseconds:.0}ms"
    );
    Ok(())
}
fn runtime_failure(error: &HealthError) -> bool {
    matches!(
        error,
        HealthError::Check(_)
            | HealthError::GpuAllocation(_)
            | HealthError::Context { .. }
            | HealthError::Timeout { .. }
    )
}

impl SystemChecks {
    /// Supply runtime adapters already initialized/loaded by the handler.
    pub fn set_gpu_frameworks(&mut self, frameworks: GpuFrameworks) {
        self.frameworks = frameworks;
    }
    async fn framework_gpu_available(&self) -> bool {
        GpuChecks::with_commands(GpuConfig::default(), None, self.nvidia_smi.clone())
            .is_available()
            .await
    }
    /// Verify device memory and a small allocation without being the first CUDA initializer.
    /// No supplied frameworks or no GPU is a successful skip, as in the Python worker.
    ///
    /// # Errors
    /// Returns a device, allocation, or synchronization failure with framework/device context.
    pub async fn check_cuda_initialization(&self) -> Result<(), HealthError> {
        if !self.frameworks.is_loaded() || !self.framework_gpu_available().await {
            return Ok(());
        }
        if let Some(torch) = &self.frameworks.torch {
            return torch_init(torch.as_ref())
                .map_err(|error| context("CUDA initialization failed", error));
        }
        if let Some(cupy) = &self.frameworks.cupy {
            return cupy_init(cupy.as_ref())
                .map_err(|error| context("CUDA initialization check failed", error));
        }
        Ok(())
    }
    /// Measure matrix allocation, multiplication, and synchronization against the configured limit.
    /// The limit is checked after completion; synchronous framework operations are not interrupted.
    ///
    /// # Errors
    /// Runtime/compute failures propagate; other setup failures permit fallback or a successful skip.
    pub async fn check_gpu_compute_benchmark(&self) -> Result<(), HealthError> {
        if !self.frameworks.is_loaded() || !self.framework_gpu_available().await {
            return Ok(());
        }
        if let Some(torch) = &self.frameworks.torch {
            let result = (|| {
                if !torch.is_initialized() || !torch.is_available()? {
                    return Ok(());
                }
                benchmark(torch.as_ref(), self.config.benchmark_timeout)
            })();
            match result {
                Ok(()) => return Ok(()),
                Err(error) if runtime_failure(&error) => return Err(error),
                Err(error) => tracing::warn!(%error,"PyTorch GPU benchmark setup failed"),
            }
        }
        if let Some(cupy) = &self.frameworks.cupy {
            match benchmark(cupy.as_ref(), self.config.benchmark_timeout) {
                Ok(()) => return Ok(()),
                Err(error) if runtime_failure(&error) => return Err(error),
                Err(error) => tracing::warn!(%error,"CuPy GPU benchmark setup failed"),
            }
        }
        Ok(())
    }
}

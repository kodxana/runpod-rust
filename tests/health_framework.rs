use runpod::serverless::health::{
    GpuFramework, GpuFrameworks, HealthError, SystemChecks, SystemConfig,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
mod support;

#[derive(Clone, Debug)]
struct Framework {
    events: Arc<Mutex<Vec<String>>>,
    initialized: bool,
    available: bool,
    devices: usize,
    memory: u64,
    allocation_fails: bool,
    benchmark_error: Option<&'static str>,
    delay: Duration,
}
impl Default for Framework {
    fn default() -> Self {
        Self {
            events: Arc::default(),
            initialized: true,
            available: true,
            devices: 2,
            memory: 8 * 1024 * 1024 * 1024,
            allocation_fails: false,
            benchmark_error: None,
            delay: Duration::ZERO,
        }
    }
}
impl Framework {
    fn record(&self, value: impl Into<String>) {
        self.events.lock().unwrap().push(value.into());
    }
}
impl GpuFramework for Framework {
    fn is_initialized(&self) -> bool {
        self.record("initialized");
        self.initialized
    }
    fn is_available(&self) -> Result<bool, HealthError> {
        self.record("available");
        Ok(self.available)
    }
    fn reset_peak_memory_stats(&self) -> Result<(), HealthError> {
        self.record("reset");
        Ok(())
    }
    fn synchronize(&self) -> Result<(), HealthError> {
        self.record("sync");
        Ok(())
    }
    fn device_count(&self) -> Result<usize, HealthError> {
        self.record("count");
        Ok(self.devices)
    }
    fn device_memory(&self, index: usize) -> Result<u64, HealthError> {
        self.record(format!("memory {index}"));
        Ok(self.memory)
    }
    fn select_device(&self, index: usize) -> Result<(), HealthError> {
        self.record(format!("select {index}"));
        Ok(())
    }
    fn allocate_zeros(&self, index: usize, elements: usize) -> Result<(), HealthError> {
        self.record(format!("allocate {index} {elements}"));
        if self.allocation_fails {
            Err(HealthError::Check("allocation broken".into()))
        } else {
            Ok(())
        }
    }
    fn matrix_multiply(&self, size: usize) -> Result<(), HealthError> {
        self.record(format!("matmul {size}"));
        std::thread::sleep(self.delay);
        match self.benchmark_error {
            Some("runtime") => Err(HealthError::Check("compute broken".into())),
            Some(_) => Err(HealthError::Configuration {
                variable: "setup".into(),
            }),
            None => Ok(()),
        }
    }
}
fn checks(frameworks: GpuFrameworks) -> SystemChecks {
    let mut checks = SystemChecks::with_commands(
        SystemConfig::default(),
        support::command("nvcc"),
        support::command("detected"),
    );
    checks.set_gpu_frameworks(frameworks);
    checks
}
#[tokio::test]
async fn unloaded_or_uninitialized_frameworks_never_initialize_cuda() {
    let mut system = checks(GpuFrameworks::default());
    system.check_cuda_initialization().await.unwrap();
    system.check_gpu_compute_benchmark().await.unwrap();
    let torch = Framework {
        initialized: false,
        ..Framework::default()
    };
    let events = torch.events.clone();
    let mut frames = GpuFrameworks::default();
    frames.set_torch(torch);
    system.set_gpu_frameworks(frames);
    system.check_cuda_initialization().await.unwrap();
    system.check_gpu_compute_benchmark().await.unwrap();
    assert_eq!(*events.lock().unwrap(), vec!["initialized", "initialized"]);
}
#[tokio::test]
async fn torch_initialization_checks_memory_and_allocation_on_every_device() {
    let torch = Framework::default();
    let events = torch.events.clone();
    let mut frames = GpuFrameworks::default();
    frames.set_torch(torch);
    checks(frames).check_cuda_initialization().await.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "initialized",
            "available",
            "reset",
            "sync",
            "count",
            "memory 0",
            "allocate 0 1024",
            "sync",
            "memory 1",
            "allocate 1 1024",
            "sync"
        ]
    );
}
#[tokio::test]
async fn torch_device_failures_retain_context_and_do_not_fall_back() {
    for (devices, memory, allocation_fails, expected) in [
        (0, 8, false, "No CUDA devices"),
        (1, 0, false, "GPU 0 reports zero memory"),
        (1, 8, true, "allocation broken"),
    ] {
        let torch = Framework {
            devices,
            memory,
            allocation_fails,
            ..Framework::default()
        };
        let cupy = Framework::default();
        let events = cupy.events.clone();
        let mut frames = GpuFrameworks::default();
        frames.set_torch(torch);
        frames.set_cupy(cupy);
        let error = checks(frames)
            .check_cuda_initialization()
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(events.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn cupy_fallback_selects_each_device_without_torch_only_probes() {
    let cupy = Framework::default();
    let events = cupy.events.clone();
    let mut frames = GpuFrameworks::default();
    frames.set_cupy(cupy);
    let system = checks(frames);
    system.check_cuda_initialization().await.unwrap();
    system.check_gpu_compute_benchmark().await.unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "sync",
            "count",
            "select 0",
            "allocate 0 1024",
            "sync",
            "select 1",
            "allocate 1 1024",
            "sync",
            "matmul 1024",
            "sync"
        ]
    );
}
#[tokio::test]
async fn benchmark_setup_error_allows_cupy_but_runtime_error_does_not() {
    for (error, fallback) in [("setup", true), ("runtime", false)] {
        let torch = Framework {
            benchmark_error: Some(error),
            ..Framework::default()
        };
        let cupy = Framework::default();
        let events = cupy.events.clone();
        let mut frames = GpuFrameworks::default();
        frames.set_torch(torch);
        frames.set_cupy(cupy);
        let result = checks(frames).check_gpu_compute_benchmark().await;
        assert_eq!(result.is_ok(), fallback);
        assert_eq!(!events.lock().unwrap().is_empty(), fallback);
    }
}
#[tokio::test]
async fn benchmark_measures_allocation_compute_and_synchronization_together() {
    let torch = Framework {
        delay: Duration::from_millis(20),
        ..Framework::default()
    };
    let mut frames = GpuFrameworks::default();
    frames.set_torch(torch);
    let mut checks = SystemChecks::with_commands(
        SystemConfig {
            benchmark_timeout: Duration::from_millis(5),
            ..SystemConfig::default()
        },
        support::command("nvcc"),
        support::command("detected"),
    );
    checks.set_gpu_frameworks(frames);
    assert!(
        checks
            .check_gpu_compute_benchmark()
            .await
            .unwrap_err()
            .to_string()
            .contains("GPU compute too slow")
    );
}

#[tokio::test]
async fn unavailable_gpu_and_unavailable_torch_skip_framework_work() {
    for gpu_present in [false, true] {
        let torch = Framework {
            available: false,
            ..Framework::default()
        };
        let events = torch.events.clone();
        let mut frames = GpuFrameworks::default();
        frames.set_torch(torch);
        let mut system = SystemChecks::with_commands(
            SystemConfig::default(),
            support::command("nvcc"),
            support::command(if gpu_present {
                "detected"
            } else {
                "detection_error"
            }),
        );
        system.set_gpu_frameworks(frames);
        system.check_cuda_initialization().await.unwrap();
        system.check_gpu_compute_benchmark().await.unwrap();
        let expected: &[&str] = if gpu_present {
            &["initialized", "available", "initialized", "available"]
        } else {
            &[]
        };
        assert_eq!(*events.lock().unwrap(), expected);
    }
}

#[tokio::test]
async fn cupy_device_failures_are_named_and_benchmark_setup_can_skip() {
    for (devices, allocation_fails, expected) in [
        (0, false, "No CUDA devices available via CuPy"),
        (1, true, "Failed to initialize GPU 0 with CuPy"),
    ] {
        let mut frames = GpuFrameworks::default();
        frames.set_cupy(Framework {
            devices,
            allocation_fails,
            ..Framework::default()
        });
        let error = checks(frames)
            .check_cuda_initialization()
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    for (error, succeeds) in [("setup", true), ("runtime", false)] {
        let mut frames = GpuFrameworks::default();
        frames.set_cupy(Framework {
            benchmark_error: Some(error),
            ..Framework::default()
        });
        assert_eq!(
            checks(frames).check_gpu_compute_benchmark().await.is_ok(),
            succeeds
        );
    }
}

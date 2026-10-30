#[cfg(test)]
mod tests;

use super::{
    GpuChecks, GpuConfig, GpuFrameworks, HealthError, SystemChecks, SystemConfig,
    environment::flag, is_early_check_eligible, report_unhealthy, reporting::UnhealthyExit,
};
use crate::{Error, Result};
use futures_util::{FutureExt, future::BoxFuture};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// Marker forwarded in child environments after an eligible early pass succeeds.
pub const EARLY_CHECKS_DONE_ENV: &str = "RUNPOD_EARLY_FITNESS_CHECKS_DONE";
const SETTINGS: &[&str] = &[
    "RUNPOD_MIN_MEMORY_GB",
    "RUNPOD_MIN_DISK_PERCENT",
    "RUNPOD_MIN_CUDA_VERSION",
    "RUNPOD_NETWORK_CHECK_TIMEOUT",
    "RUNPOD_GPU_BENCHMARK_TIMEOUT",
    "RUNPOD_GPU_TEST_TIMEOUT",
    "RUNPOD_GPU_MAX_ERROR_MESSAGES",
    "RUNPOD_BINARY_GPU_TEST_PATH",
    "RUNPOD_SKIP_AUTO_SYSTEM_CHECKS",
    "RUNPOD_SKIP_GPU_CHECK",
];

/// Whether only safe shared checks or all pending checks may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitnessPass {
    /// Safe shared checks before the handler is initialized.
    Early,
    /// All pending checks, plus checks marked for a worker-start recheck.
    WorkerStart,
}
/// Outcome of the explicit synchronous early-check entrypoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EarlyCheckStatus {
    /// An eligibility, skip, defer, or inherited-marker gate prevented early execution.
    Skipped,
    /// Setup could not finish, or the calling thread already has an async runtime.
    Deferred,
    /// Built-in registration and all safe shared checks succeeded.
    Passed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Gpu,
    System,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Builtin {
    Gpu,
    Memory,
    Disk,
    Network,
    CudaVersion,
    CudaInit,
    Benchmark,
}
impl Builtin {
    fn name(self) -> &'static str {
        match self {
            Self::Gpu => "_gpu_health_check",
            Self::Memory => "_memory_check",
            Self::Disk => "_disk_check",
            Self::Network => "_network_check",
            Self::CudaVersion => "_cuda_version_check",
            Self::CudaInit => "_cuda_init_check",
            Self::Benchmark => "_benchmark_check",
        }
    }
    fn group(self) -> Group {
        if self == Self::Gpu {
            Group::Gpu
        } else {
            Group::System
        }
    }
    fn dependencies(self) -> &'static [&'static str] {
        match self {
            Self::Gpu => &[
                "RUNPOD_GPU_TEST_TIMEOUT",
                "RUNPOD_GPU_MAX_ERROR_MESSAGES",
                "RUNPOD_BINARY_GPU_TEST_PATH",
            ],
            Self::Memory => &["RUNPOD_MIN_MEMORY_GB"],
            Self::Disk => &["RUNPOD_MIN_DISK_PERCENT"],
            Self::Network => &["RUNPOD_NETWORK_CHECK_TIMEOUT"],
            Self::CudaVersion => &["RUNPOD_MIN_CUDA_VERSION"],
            Self::Benchmark => &["RUNPOD_GPU_BENCHMARK_TIMEOUT"],
            Self::CudaInit => &[],
        }
    }
}
type Callback = Box<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;
enum Body {
    Custom(Callback),
    Builtin(Builtin),
}
struct CheckInner {
    name: String,
    body: Body,
    deferred: AtomicBool,
    recheck: AtomicBool,
}

/// A cloneable check handle whose identity is preserved across registration and passes.
#[derive(Clone)]
pub struct FitnessCheck(Arc<CheckInner>);
impl fmt::Debug for FitnessCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FitnessCheck")
            .field("name", &self.name())
            .field("deferred", &self.is_deferred())
            .finish_non_exhaustive()
    }
}
impl PartialEq for FitnessCheck {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for FitnessCheck {}
impl FitnessCheck {
    /// Create an async check factory without executing or registering it.
    pub fn new<F, Fut>(name: impl Into<String>, check: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self::from_body(name.into(), Body::Custom(Box::new(move || check().boxed())))
    }
    /// Create a synchronous check, executed in order on the caller's thread like Python.
    pub fn sync(
        name: impl Into<String>,
        check: impl Fn() -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self::from_body(
            name.into(),
            Body::Custom(Box::new(move || {
                futures_util::future::ready(check()).boxed()
            })),
        )
    }
    fn from_body(name: String, body: Body) -> Self {
        Self(Arc::new(CheckInner {
            name,
            body,
            deferred: AtomicBool::new(false),
            recheck: AtomicBool::new(false),
        }))
    }
    fn builtin(kind: Builtin) -> Self {
        let check = Self::from_body(kind.name().into(), Body::Builtin(kind));
        if matches!(
            kind,
            Builtin::Network | Builtin::CudaInit | Builtin::Benchmark
        ) {
            check.0.deferred.store(true, Ordering::SeqCst);
        }
        if kind == Builtin::Disk {
            check.0.recheck.store(true, Ordering::SeqCst);
        }
        check
    }
    /// The name included in health reports and timing logs.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0.name
    }
    /// Mark this same check identity as unsafe to execute before handler initialization.
    #[must_use]
    pub fn defer_to_worker_start(self) -> Self {
        self.0.deferred.store(true, Ordering::SeqCst);
        self
    }
    /// Request one worker-start execution even if an early execution already passed.
    #[must_use]
    pub fn recheck_at_worker_start(self) -> Self {
        self.0.recheck.store(true, Ordering::SeqCst);
        self
    }
    /// Whether the check is deferred until handler initialization completes.
    #[must_use]
    pub fn is_deferred(&self) -> bool {
        self.0.deferred.load(Ordering::SeqCst)
    }
    fn group(&self) -> Option<Group> {
        match self.0.body {
            Body::Builtin(kind) => Some(kind.group()),
            Body::Custom(_) => None,
        }
    }
    fn shared(&self) -> bool {
        self.group().is_some() && !self.is_deferred()
    }
    /// Invoke a custom check directly, outside lifecycle tracking.
    ///
    /// # Errors
    /// Returns the check's error; built-in checks require a registry's settings.
    pub async fn run(&self) -> Result<()> {
        match &self.0.body {
            Body::Custom(check) => check().await,
            Body::Builtin(_) => Err(Error::InvalidInput(
                "built-in fitness checks require FitnessChecks::check".into(),
            )),
        }
    }
}

/// The named check or setup operation that failed, retaining its typed native cause.
#[derive(Debug, thiserror::Error)]
#[error("Fitness check failed: {check} | {source}")]
pub struct FitnessFailure {
    /// Registered name, or `fitness_check_setup` for a configuration/registration failure.
    pub check: String,
    /// Typed error returned by setup or the failing check.
    #[source]
    pub source: Error,
}
impl FitnessFailure {
    /// Python-compatible category and message for the unhealthy heartbeat reason.
    #[must_use]
    pub fn reason(&self) -> String {
        fn io_name(error: &std::io::Error) -> &'static str {
            match error.kind() {
                std::io::ErrorKind::NotFound => "FileNotFoundError",
                std::io::ErrorKind::PermissionDenied => "PermissionError",
                _ => "OSError",
            }
        }
        let category = match &self.source {
            Error::Health(HealthError::GpuAllocation(_)) => "GpuAllocationError",
            Error::Health(HealthError::Configuration { .. }) | Error::InvalidInput(_) => {
                "ValueError"
            }
            Error::Health(HealthError::Io { source, .. }) | Error::Io(source) => io_name(source),
            _ => "RuntimeError",
        };
        format!("{category}: {}", self.source)
    }
}
#[derive(Clone, Debug, Default)]
struct ProbeSettings {
    gpu: GpuConfig,
    system: SystemConfig,
    frameworks: GpuFrameworks,
}
trait Probes: fmt::Debug + Send + Sync {
    fn gpu_available(&self) -> BoxFuture<'_, bool>;
    fn run<'a>(
        &'a self,
        kind: Builtin,
        settings: &'a ProbeSettings,
        values: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<()>>;
}
#[derive(Debug, Default)]
struct NativeProbes;
impl Probes for NativeProbes {
    fn gpu_available(&self) -> BoxFuture<'_, bool> {
        crate::utils::is_cuda_available().boxed()
    }
    fn run<'a>(
        &'a self,
        kind: Builtin,
        settings: &'a ProbeSettings,
        values: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<()>> {
        async move {
            if kind == Builtin::Gpu {
                return Ok(GpuChecks::with_environment(settings.gpu.clone(), values)?
                    .check_gpu_health()
                    .await?);
            }
            let mut config = settings.system.clone();
            config.job_url = values.get("RUNPOD_WEBHOOK_GET_JOB").cloned();
            let mut system = SystemChecks::new(config);
            system.set_gpu_frameworks(settings.frameworks.clone());
            match kind {
                Builtin::Memory => system.check_memory_availability()?,
                Builtin::Disk => system.check_disk_space()?,
                Builtin::Network => system.check_network_connectivity().await?,
                Builtin::CudaVersion => system.check_cuda_versions().await?,
                Builtin::CudaInit => system.check_cuda_initialization().await?,
                Builtin::Benchmark => system.check_gpu_compute_benchmark().await?,
                Builtin::Gpu => unreachable!("GPU allocation returns above"),
            }
            Ok(())
        }
        .boxed()
    }
}

/// Ordered fitness checks with owned environment, completion identities, and early-pass state.
pub struct FitnessChecks {
    environment: BTreeMap<String, String>,
    checks: Vec<FitnessCheck>,
    completed: Vec<FitnessCheck>,
    worker_completed: Vec<FitnessCheck>,
    gpu_registered: bool,
    system_registered: bool,
    snapshot: BTreeMap<String, Option<String>>,
    settings: ProbeSettings,
    probes: Arc<dyn Probes>,
}
impl fmt::Debug for FitnessChecks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FitnessChecks")
            .field("checks", &self.checks)
            .field("completed", &self.completed.len())
            .finish_non_exhaustive()
    }
}
impl FitnessChecks {
    /// Capture explicit configuration without registering checks or probing the machine.
    #[must_use]
    pub fn new(environment: BTreeMap<String, String>) -> Self {
        Self {
            environment,
            checks: Vec::new(),
            completed: Vec::new(),
            worker_completed: Vec::new(),
            gpu_registered: false,
            system_registered: false,
            snapshot: BTreeMap::new(),
            settings: ProbeSettings::default(),
            probes: Arc::new(NativeProbes),
        }
    }
    /// Capture the current process environment without mutating it.
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(std::env::vars().collect())
    }
    /// Configuration and the successful early marker to forward to child processes.
    #[must_use]
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
    /// Change owned settings before a later pass; early-consumed settings are compared then.
    pub fn environment_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.environment
    }
    /// Supply frameworks already loaded by the handler for deferred CUDA checks.
    pub fn set_gpu_frameworks(&mut self, frameworks: GpuFrameworks) {
        self.settings.frameworks = frameworks;
    }
    /// Registered checks in execution order, including duplicates.
    #[must_use]
    pub fn checks(&self) -> &[FitnessCheck] {
        &self.checks
    }
    /// Append a check and return its original identity unchanged.
    pub fn register_fitness_check(&mut self, check: FitnessCheck) -> FitnessCheck {
        self.checks.push(check.clone());
        tracing::debug!("Registered fitness check: {}", check.name());
        check
    }
    /// Clear checks and completion tracking, retaining the source's registration latches.
    pub fn clear_fitness_checks(&mut self) {
        self.checks.clear();
        self.completed.clear();
        self.worker_completed.clear();
    }
    /// Clear built-in registration latches without clearing registered checks. Intended for tests.
    pub fn reset_registration_state(&mut self) {
        self.gpu_registered = false;
        self.system_registered = false;
    }
    fn capture_settings(&mut self) {
        self.snapshot = SETTINGS
            .iter()
            .map(|name| ((*name).into(), self.environment.get(*name).cloned()))
            .collect();
    }
    async fn register_builtins(&mut self) -> Result<()> {
        let before = self.checks.len();
        let state = (self.gpu_registered, self.system_registered);
        let result = async {
            if !self.gpu_registered {
                if !flag(&self.environment, "RUNPOD_SKIP_GPU_CHECK") {
                    self.settings.gpu = GpuConfig::from_values(&self.environment)?;
                    if self.probes.gpu_available().await {
                        self.register_fitness_check(FitnessCheck::builtin(Builtin::Gpu));
                    }
                }
                self.gpu_registered = true;
            }
            if !self.system_registered {
                if !flag(&self.environment, "RUNPOD_SKIP_AUTO_SYSTEM_CHECKS") {
                    self.settings.system = SystemConfig::from_values(&self.environment)?;
                    for kind in [Builtin::Memory, Builtin::Disk, Builtin::Network] {
                        self.register_fitness_check(FitnessCheck::builtin(kind));
                    }
                    if self.probes.gpu_available().await {
                        for kind in [Builtin::CudaVersion, Builtin::CudaInit, Builtin::Benchmark] {
                            self.register_fitness_check(FitnessCheck::builtin(kind));
                        }
                    }
                } else {
                    tracing::debug!("System fitness checks disabled via environment (RUNPOD_SKIP_AUTO_SYSTEM_CHECKS)");
                }
                self.system_registered = true;
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            self.checks.truncate(before);
            (self.gpu_registered, self.system_registered) = state;
        }
        result
    }
    fn refresh_late_config(&mut self) -> Result<()> {
        let changed: BTreeSet<_> = self
            .snapshot
            .iter()
            .filter(|(name, old)| self.environment.get(*name) != old.as_ref())
            .map(|(name, _)| name.clone())
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        tracing::warn!(
            "Fitness check config changed since early checks; applying at worker start: {}",
            changed.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        if !flag(&self.environment, "RUNPOD_SKIP_GPU_CHECK") {
            self.settings.gpu = GpuConfig::from_values(&self.environment)?;
        }
        if !flag(&self.environment, "RUNPOD_SKIP_AUTO_SYSTEM_CHECKS") {
            self.settings.system = SystemConfig::from_values(&self.environment)?;
        }
        self.completed.retain(|check| match &check.0.body {
            Body::Custom(_) => true,
            Body::Builtin(kind) => !kind
                .dependencies()
                .iter()
                .any(|name| changed.contains(*name)),
        });
        for (name, group) in [
            ("RUNPOD_SKIP_GPU_CHECK", Group::Gpu),
            ("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS", Group::System),
        ] {
            if changed.contains(name) {
                self.checks.retain(|c| c.group() != Some(group));
                self.completed.retain(|c| c.group() != Some(group));
                match group {
                    Group::Gpu => self.gpu_registered = false,
                    Group::System => self.system_registered = false,
                }
            }
        }
        self.capture_settings();
        Ok(())
    }
    /// Validate one pass and return a typed failure instead of terminating the process.
    /// Early setup errors are deferred for retry; hardware and customer errors are returned.
    ///
    /// # Errors
    /// Returns the first failing check or a worker-start setup failure.
    pub async fn check(&mut self, pass: FitnessPass) -> std::result::Result<(), FitnessFailure> {
        if flag(&self.environment, "RUNPOD_SKIP_FITNESS_CHECKS") {
            tracing::info!("Fitness checks disabled via RUNPOD_SKIP_FITNESS_CHECKS, skipping.");
            return Ok(());
        }
        let prepared = async {
            if pass == FitnessPass::WorkerStart && !self.snapshot.is_empty() {
                self.refresh_late_config()?;
            }
            self.register_builtins().await
        }
        .await;
        if let Err(source) = prepared {
            if pass == FitnessPass::Early {
                tracing::error!(%source,"Fitness checks could not be prepared; retrying at worker start");
                return Ok(());
            }
            return Err(FitnessFailure {
                check: "fitness_check_setup".into(),
                source,
            });
        }
        let pending: Vec<_> = self
            .checks
            .iter()
            .filter(|check| {
                !self.completed.contains(check)
                    || (pass == FitnessPass::WorkerStart
                        && check.0.recheck.load(Ordering::SeqCst)
                        && !self.worker_completed.contains(check))
            })
            .filter(|check| pass == FitnessPass::WorkerStart || check.shared())
            .cloned()
            .collect();
        if pending.is_empty() {
            tracing::debug!("No pending fitness checks, skipping.");
            return Ok(());
        }
        tracing::info!("Running {} fitness check(s)...", pending.len());
        let started = Instant::now();
        for check in pending {
            tracing::debug!("Executing fitness check: {}", check.name());
            let check_started = Instant::now();
            let result = match &check.0.body {
                Body::Custom(callback) => callback().await,
                Body::Builtin(kind) => {
                    self.probes
                        .run(*kind, &self.settings, &self.environment)
                        .await
                }
            };
            if let Err(source) = result {
                return Err(FitnessFailure {
                    check: check.name().into(),
                    source,
                });
            }
            if !self.completed.contains(&check) {
                self.completed.push(check.clone());
            }
            if pass == FitnessPass::WorkerStart && check.0.recheck.load(Ordering::SeqCst) {
                self.worker_completed.push(check.clone());
            }
            tracing::debug!(
                "Fitness check passed: {} ({:.2}ms)",
                check.name(),
                check_started.elapsed().as_secs_f64() * 1000.0
            );
        }
        tracing::info!(
            "All fitness checks passed. ({:.2}ms)",
            started.elapsed().as_secs_f64() * 1000.0
        );
        Ok(())
    }
    async fn fail_worker(&self, failure: FitnessFailure) -> ! {
        let _exit = UnhealthyExit;
        let reason = failure.reason();
        tracing::error!("Fitness check failed: {} | {reason}", failure.check);
        report_unhealthy(&self.environment, &failure.check, &reason).await;
        tracing::error!("Worker is unhealthy, exiting.");
        super::terminate_unhealthy(1)
    }
    /// Run all pending worker-start checks; a failure reports unhealthy and force-exits.
    /// This process-wide behavior intentionally matches the Python worker entrypoint.
    pub async fn run_fitness_checks(&mut self) {
        if let Err(failure) = self.check(FitnessPass::WorkerStart).await {
            self.fail_worker(failure).await;
        }
    }
    /// Run safe early checks before loading the handler, outside an existing async runtime.
    /// A successful marker is stored in the owned environment, which callers forward to children.
    /// Hardware failures force-exit; setup errors remain retryable at worker start.
    pub fn run_startup_fitness_checks(&mut self, args: &[String]) -> EarlyCheckStatus {
        if flag(&self.environment, "RUNPOD_SKIP_FITNESS_CHECKS")
            || flag(&self.environment, "RUNPOD_DEFER_FITNESS_CHECKS")
            || !is_early_check_eligible(&self.environment, args)
            || self
                .environment
                .get(EARLY_CHECKS_DONE_ENV)
                .is_some_and(|s| !s.is_empty())
        {
            return EarlyCheckStatus::Skipped;
        }
        if tokio::runtime::Handle::try_current().is_ok() {
            return EarlyCheckStatus::Deferred;
        }
        self.capture_settings();
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(%error,"Startup fitness checks could not run");
                return EarlyCheckStatus::Deferred;
            }
        };
        runtime.block_on(async {
            if let Err(failure) = self.check(FitnessPass::Early).await {
                self.fail_worker(failure).await;
            }
        });
        if self.gpu_registered && self.system_registered {
            self.environment
                .insert(EARLY_CHECKS_DONE_ENV.into(), "1".into());
            EarlyCheckStatus::Passed
        } else {
            EarlyCheckStatus::Deferred
        }
    }
}

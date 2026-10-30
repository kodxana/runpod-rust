use super::*;
use std::sync::{Mutex, atomic::AtomicUsize};

#[derive(Debug)]
struct MockProbes {
    gpu: bool,
    detections: AtomicUsize,
    calls: Mutex<Vec<(Builtin, ProbeSettings)>>,
    fail: Mutex<Option<Builtin>>,
}
impl Probes for MockProbes {
    fn gpu_available(&self) -> BoxFuture<'_, bool> {
        self.detections.fetch_add(1, Ordering::SeqCst);
        futures_util::future::ready(self.gpu).boxed()
    }
    fn run<'a>(
        &'a self,
        kind: Builtin,
        settings: &'a ProbeSettings,
        _: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<()>> {
        self.calls.lock().unwrap().push((kind, settings.clone()));
        let fail = *self.fail.lock().unwrap() == Some(kind);
        async move {
            if fail {
                Err(HealthError::Check("hardware failed".into()).into())
            } else {
                Ok(())
            }
        }
        .boxed()
    }
}
fn registry(gpu: bool) -> (FitnessChecks, Arc<MockProbes>) {
    let probes = Arc::new(MockProbes {
        gpu,
        detections: AtomicUsize::new(0),
        calls: Mutex::new(Vec::new()),
        fail: Mutex::new(None),
    });
    let mut registry = FitnessChecks::new(BTreeMap::from([
        ("RUNPOD_ENDPOINT_ID".into(), "endpoint".into()),
        (
            "RUNPOD_WEBHOOK_GET_JOB".into(),
            "https://worker.invalid/job".into(),
        ),
    ]));
    registry.probes = probes.clone();
    (registry, probes)
}
fn kinds(probes: &MockProbes) -> Vec<Builtin> {
    probes
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(kind, _)| *kind)
        .collect()
}

#[test]
fn early_only_runs_shared_checks_then_worker_runs_deferred_and_disk_once() {
    let (mut checks, probes) = registry(true);
    assert_eq!(
        checks.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Passed
    );
    assert_eq!(
        kinds(&probes),
        [
            Builtin::Gpu,
            Builtin::Memory,
            Builtin::Disk,
            Builtin::CudaVersion
        ]
    );
    assert_eq!(
        checks
            .checks
            .iter()
            .filter(|check| check.is_deferred())
            .map(FitnessCheck::name)
            .collect::<Vec<_>>(),
        ["_network_check", "_cuda_init_check", "_benchmark_check"]
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(checks.check(FitnessPass::WorkerStart))
        .unwrap();
    assert_eq!(
        kinds(&probes),
        [
            Builtin::Gpu,
            Builtin::Memory,
            Builtin::Disk,
            Builtin::CudaVersion,
            Builtin::Disk,
            Builtin::Network,
            Builtin::CudaInit,
            Builtin::Benchmark
        ]
    );
    runtime
        .block_on(checks.check(FitnessPass::WorkerStart))
        .unwrap();
    assert_eq!(kinds(&probes).len(), 8);
    assert_eq!(probes.detections.load(Ordering::SeqCst), 2);
}

#[test]
fn changed_threshold_rechecks_only_affected_completed_checks() {
    let (mut checks, probes) = registry(false);
    assert_eq!(
        checks.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Passed
    );
    checks
        .environment_mut()
        .insert("RUNPOD_MIN_DISK_PERCENT".into(), "2".into());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime
        .block_on(checks.check(FitnessPass::WorkerStart))
        .unwrap();
    assert_eq!(
        kinds(&probes),
        [
            Builtin::Memory,
            Builtin::Disk,
            Builtin::Disk,
            Builtin::Network
        ]
    );
    assert_eq!(
        probes.calls.lock().unwrap()[2].1.system.min_disk_percent,
        2.0
    );
    checks
        .environment_mut()
        .insert("RUNPOD_MIN_MEMORY_GB".into(), "8".into());
    runtime
        .block_on(checks.check(FitnessPass::WorkerStart))
        .unwrap();
    let calls = probes.calls.lock().unwrap();
    assert_eq!(calls.len(), 5);
    assert_eq!(calls[4].0, Builtin::Memory);
    assert_eq!(calls[4].1.system.min_memory_gb, 8.0);
}

#[test]
fn skip_flags_remove_and_reregister_groups_after_early_checks() {
    for (flag, group) in [
        ("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS", Group::System),
        ("RUNPOD_SKIP_GPU_CHECK", Group::Gpu),
    ] {
        let (mut checks, probes) = registry(true);
        assert_eq!(
            checks.run_startup_fitness_checks(&[]),
            EarlyCheckStatus::Passed
        );
        let original: Vec<_> = checks
            .checks
            .iter()
            .filter(|check| check.group() == Some(group))
            .cloned()
            .collect();
        checks.environment_mut().insert(flag.into(), "true".into());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(checks.check(FitnessPass::WorkerStart))
            .unwrap();
        assert!(
            checks
                .checks
                .iter()
                .all(|check| check.group() != Some(group))
        );
        let before = kinds(&probes).len();
        checks.environment_mut().remove(flag);
        runtime
            .block_on(checks.check(FitnessPass::WorkerStart))
            .unwrap();
        let restored: Vec<_> = checks
            .checks
            .iter()
            .filter(|check| check.group() == Some(group))
            .collect();
        assert_eq!(restored.len(), original.len());
        assert!(restored.iter().all(|check| !original.contains(check)));
        assert_eq!(kinds(&probes).len() - before, restored.len());
    }
}

#[tokio::test]
async fn failed_system_registration_rolls_back_gpu_registration_and_retries() {
    let (mut checks, probes) = registry(true);
    let custom = checks.register_fitness_check(FitnessCheck::sync("customer", || Ok(())));
    checks
        .environment_mut()
        .insert("RUNPOD_MIN_MEMORY_GB".into(), "invalid".into());
    checks.check(FitnessPass::Early).await.unwrap();
    assert_eq!(checks.checks(), [custom]);
    assert!(kinds(&probes).is_empty());
    assert_eq!(
        checks
            .check(FitnessPass::WorkerStart)
            .await
            .unwrap_err()
            .check,
        "fitness_check_setup"
    );
    checks.environment_mut().remove("RUNPOD_MIN_MEMORY_GB");
    checks.check(FitnessPass::WorkerStart).await.unwrap();
    assert_eq!(checks.checks.len(), 8);
    assert_eq!(kinds(&probes).len(), 7);
}

#[test]
fn disk_exhausted_after_handler_setup_stops_before_network_or_handler_checks() {
    let (mut checks, probes) = registry(false);
    assert_eq!(
        checks.run_startup_fitness_checks(&[]),
        EarlyCheckStatus::Passed
    );
    *probes.fail.lock().unwrap() = Some(Builtin::Disk);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let failure = runtime
        .block_on(checks.check(FitnessPass::WorkerStart))
        .unwrap_err();
    assert_eq!(failure.check, "_disk_check");
    assert_eq!(
        kinds(&probes),
        [Builtin::Memory, Builtin::Disk, Builtin::Disk]
    );
}

#[tokio::test]
async fn clearing_checks_preserves_registration_latches_until_explicit_reset() {
    let (mut checks, probes) = registry(false);
    checks.check(FitnessPass::WorkerStart).await.unwrap();
    checks.clear_fitness_checks();
    checks.check(FitnessPass::WorkerStart).await.unwrap();
    assert!(checks.checks.is_empty());
    assert_eq!(kinds(&probes).len(), 3);
    checks.reset_registration_state();
    checks.check(FitnessPass::WorkerStart).await.unwrap();
    assert_eq!(kinds(&probes).len(), 6);
}

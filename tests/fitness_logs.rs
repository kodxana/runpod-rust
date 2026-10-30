use runpod::serverless::health::{FitnessCheck, FitnessChecks, FitnessPass, network_probe_target};
use std::{
    collections::BTreeMap,
    fmt::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<String>>>);
impl tracing::Subscriber for Logs {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                write!(self.0, "{field}={value:?} ").unwrap();
            }
        }
        let mut fields = Fields(event.metadata().level().to_string());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
}

#[test]
fn invalid_network_target_warns_once_without_exposing_worker_credentials() {
    let logs = Logs::default();
    tracing::subscriber::with_default(logs.clone(), || {
        assert_eq!(
            network_probe_target(Some(
                "https://secret:password@worker.invalid:$PORT/job?token=secret"
            )),
            ("api.runpod.ai".into(), 443)
        );
        network_probe_target(None);
        network_probe_target(Some("https://worker.invalid/job"));
    });
    let entries = logs.0.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].starts_with("WARN"));
    assert!(!entries[0].contains("password"));
    assert!(!entries[0].contains("secret"));
}

#[test]
fn registration_names_counts_success_and_two_decimal_timings_are_logged() {
    let logs = Logs::default();
    tracing::subscriber::with_default(logs.clone(), || {
        let mut checks = FitnessChecks::new(BTreeMap::from([
            ("RUNPOD_SKIP_GPU_CHECK".into(), "true".into()),
            ("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS".into(), "true".into()),
        ]));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            checks.check(FitnessPass::WorkerStart).await.unwrap();
            checks.register_fitness_check(FitnessCheck::new("timed", || async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(())
            }));
            checks.check(FitnessPass::WorkerStart).await.unwrap();
        });
    });
    let entries = logs.0.lock().unwrap();
    for expected in [
        "No pending fitness checks",
        "Registered fitness check: timed",
        "Running 1 fitness check(s)",
        "Executing fitness check: timed",
        "Fitness check passed: timed",
        "All fitness checks passed.",
    ] {
        assert!(
            entries.iter().any(|line| line.contains(expected)),
            "missing {expected}: {entries:?}"
        );
    }
    let timing = regex::Regex::new(r"\(([0-9]+\.[0-9]{2})ms\)").unwrap();
    let milliseconds: Vec<f64> = entries
        .iter()
        .filter_map(|line| timing.captures(line))
        .map(|capture| capture[1].parse().unwrap())
        .collect();
    assert_eq!(milliseconds.len(), 2);
    assert!(milliseconds[0] >= 10.0);
    assert!(milliseconds[1] >= milliseconds[0]);
}

#[test]
fn late_configuration_warning_names_changes_once_and_omits_values() {
    let logs = Logs::default();
    tracing::subscriber::with_default(logs.clone(), || {
        let mut checks = FitnessChecks::new(BTreeMap::from([
            ("RUNPOD_SKIP_GPU_CHECK".into(), "true".into()),
            ("RUNPOD_SKIP_AUTO_SYSTEM_CHECKS".into(), "true".into()),
            ("RUNPOD_ENDPOINT_ID".into(), "endpoint".into()),
            (
                "RUNPOD_WEBHOOK_GET_JOB".into(),
                "https://worker.invalid/job".into(),
            ),
        ]));
        checks.run_startup_fitness_checks(&[]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(checks.check(FitnessPass::WorkerStart))
            .unwrap();
        assert!(
            !logs
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("config changed"))
        );
        checks
            .environment_mut()
            .insert("RUNPOD_MIN_MEMORY_GB".into(), "private-value".into());
        runtime
            .block_on(checks.check(FitnessPass::WorkerStart))
            .unwrap();
        runtime
            .block_on(checks.check(FitnessPass::WorkerStart))
            .unwrap();
    });
    let entries = logs.0.lock().unwrap();
    let warnings: Vec<_> = entries
        .iter()
        .filter(|line| line.contains("config changed"))
        .collect();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("RUNPOD_MIN_MEMORY_GB"));
    assert!(!warnings[0].contains("private-value"));
}

//! Dynamic worker concurrency and shared shutdown/cancellation controls.
use super::{JobsProgress, WorkerConfig};
use crate::{Error, Result};
use futures_util::future::AbortHandle;
use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::watch, time::Instant};

/// Called at the beginning of each intake iteration, before any required drain.
/// Returning zero pauses intake. Closures own their state and must return promptly.
#[derive(Clone)]
pub struct ConcurrencyModifier(Arc<dyn Fn(usize) -> usize + Send + Sync>);
impl ConcurrencyModifier {
    /// Own a concurrency policy that runs before each intake iteration.
    pub fn new(modifier: impl Fn(usize) -> usize + Send + Sync + 'static) -> Self {
        Self(Arc::new(modifier))
    }
    fn apply(&self, current: usize) -> usize {
        (self.0)(current)
    }
}
impl fmt::Debug for ConcurrencyModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConcurrencyModifier")
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct ControlState {
    stopped: watch::Sender<bool>,
    running: AtomicBool,
    concurrency: AtomicUsize,
    queued: AtomicUsize,
    tasks: Mutex<HashMap<String, AbortHandle>>,
    progress: JobsProgress,
}

/// A cloneable handle for observing and stopping a worker without owning its run future.
#[derive(Clone, Debug)]
pub struct WorkerControl(Arc<ControlState>);
impl WorkerControl {
    pub(super) fn new(concurrency: usize, progress: JobsProgress) -> Self {
        let (stopped, _) = watch::channel(false);
        Self(Arc::new(ControlState {
            stopped,
            running: AtomicBool::new(false),
            concurrency: AtomicUsize::new(concurrency),
            queued: AtomicUsize::new(0),
            tasks: Mutex::new(HashMap::new()),
            progress,
        }))
    }
    /// Whether shutdown has not yet been requested, including before the first run.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        !*self.0.stopped.borrow()
    }
    /// Stop intake immediately; already claimed jobs drain before run returns.
    pub fn kill_worker(&self) {
        self.0.stopped.send_replace(true);
    }
    /// Cancel a tracked job. Unknown IDs return false; other jobs are unaffected.
    pub fn stop_job(&self, job_id: &str) -> bool {
        let tasks = self.0.tasks.lock().expect("worker task registry poisoned");
        if let Some(task) = tasks.get(job_id) {
            task.abort();
            true
        } else {
            false
        }
    }
    /// The last concurrency selected by the policy, including a pending queue resize.
    #[must_use]
    pub fn current_concurrency(&self) -> usize {
        self.0.concurrency.load(Ordering::SeqCst)
    }
    /// Python counts queued jobs plus the progress tracker, which also includes queued jobs.
    #[must_use]
    pub fn current_occupancy(&self) -> usize {
        self.0
            .queued
            .load(Ordering::SeqCst)
            .saturating_add(self.0.progress.get_job_count())
    }
    pub(super) fn stopped(&self) -> watch::Receiver<bool> {
        self.0.stopped.subscribe()
    }
    pub(super) fn concurrency(&self, value: usize) {
        self.0.concurrency.store(value, Ordering::SeqCst);
    }
    pub(super) fn queued(&self, value: usize) {
        self.0.queued.store(value, Ordering::SeqCst);
    }
    pub(super) fn insert(&self, id: String, handle: AbortHandle) {
        self.0
            .tasks
            .lock()
            .expect("worker task registry poisoned")
            .insert(id, handle);
    }
    pub(super) fn remove(&self, id: &str) {
        self.0
            .tasks
            .lock()
            .expect("worker task registry poisoned")
            .remove(id);
    }
    pub(super) fn begin(&self) -> Result<RunGuard> {
        if self.0.running.swap(true, Ordering::SeqCst) {
            return Err(Error::InvalidInput("this worker is already running".into()));
        }
        Ok(RunGuard(self.clone()))
    }
}
pub(super) struct RunGuard(WorkerControl);
impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.kill_worker();
        self.0
            .0
            .tasks
            .lock()
            .expect("worker task registry poisoned")
            .clear();
        self.0.queued(0);
        self.0.0.progress.clear();
        self.0.0.running.store(false, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
enum WaitKind {
    Full,
    Resize,
    Backoff,
}
pub(super) struct IntakeSchedule {
    concurrency: usize,
    capacity: usize,
    modify: bool,
    wait: Option<(WaitKind, Instant)>,
}
impl IntakeSchedule {
    pub(super) fn new(concurrency: usize) -> Self {
        Self {
            concurrency,
            capacity: concurrency,
            modify: true,
            wait: None,
        }
    }
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.wait.map(|(_, deadline)| deadline)
    }
    pub(super) fn after_response(&mut self, delay: Duration) {
        self.modify = true;
        self.wait = (!delay.is_zero()).then(|| (WaitKind::Backoff, Instant::now() + delay));
    }
    pub(super) fn poll(
        &mut self,
        occupancy: usize,
        config: &WorkerConfig,
        control: &WorkerControl,
    ) -> Option<usize> {
        if let Some((kind, deadline)) = self.wait {
            let drained = matches!(kind, WaitKind::Full) && occupancy == 0 && self.concurrency > 0;
            if !drained && Instant::now() < deadline {
                return None;
            }
            self.wait = None;
            if !matches!(kind, WaitKind::Resize) {
                self.modify = true;
            }
        }
        if self.modify {
            if let Some(modifier) = &config.concurrency_modifier {
                self.concurrency = modifier.apply(self.concurrency);
            }
            control.concurrency(self.concurrency);
            self.modify = false;
        }
        if self.capacity != self.concurrency {
            if occupancy > 0 {
                self.wait = Some((
                    WaitKind::Resize,
                    Instant::now() + config.scale_wait_interval,
                ));
                return None;
            }
            self.capacity = self.concurrency;
        }
        let free = self.concurrency.saturating_sub(occupancy);
        if free == 0 {
            self.wait = Some((WaitKind::Full, Instant::now() + config.full_wait_interval));
            return None;
        }
        Some(free)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Value, serverless::WorkerJob};
    #[tokio::test(start_paused = true)]
    async fn python_scaler_decisions_and_occupancy() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/scaling.json")).unwrap();
        for case in cases {
            let size = |name: &str| case[name].as_u64().unwrap() as usize;
            match case["operation"].as_str().unwrap() {
                "occupancy" => {
                    let progress = JobsProgress::new();
                    for id in 0..size("tracked") {
                        progress.add(WorkerJob::new(id.to_string(), Value::Null));
                    }
                    let control = WorkerControl::new(1, progress);
                    control.queued(size("queued"));
                    assert_eq!(
                        control.current_occupancy(),
                        case["output"].as_u64().unwrap() as usize
                    );
                }
                "identity" => {
                    let mut schedule = IntakeSchedule::new(size("current"));
                    let control = WorkerControl::new(size("current"), JobsProgress::new());
                    schedule.poll(0, &WorkerConfig::default(), &control);
                    assert_eq!(
                        control.current_concurrency(),
                        case["output"].as_u64().unwrap() as usize
                    );
                }
                "scale" => {
                    let target = size("target");
                    let mut remaining = size("occupied");
                    let mut schedule = IntakeSchedule::new(size("current"));
                    let config = WorkerConfig {
                        concurrency_modifier: Some(ConcurrencyModifier::new(move |_| target)),
                        ..WorkerConfig::default()
                    };
                    let control = WorkerControl::new(size("current"), JobsProgress::new());
                    let mut sleeps = Vec::new();
                    loop {
                        schedule.poll(remaining, &config, &control);
                        if schedule.capacity == target {
                            break;
                        }
                        sleeps.push(
                            crate::json!({"delay":1,"concurrency":control.current_concurrency()}),
                        );
                        remaining -= 1;
                        tokio::time::advance(Duration::from_secs(1)).await;
                    }
                    assert_eq!(
                        crate::json!({"concurrency":control.current_concurrency(),"capacity":schedule.capacity,"sleeps":sleeps}),
                        case["output"],
                        "{case}"
                    );
                }
                _ => unreachable!(),
            }
        }
    }
}

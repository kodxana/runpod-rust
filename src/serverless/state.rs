use super::PingJobMirror;
use crate::Value;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};

/// Input job received by a worker, including unknown protocol fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerJob {
    /// Unique request identifier.
    pub id: String,
    /// User input, which can be any JSON value.
    #[serde(default)]
    pub input: Value,
    /// Optional result webhook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    /// Additional protocol metadata retained across deserialization.
    #[serde(flatten)]
    pub metadata: serde_json::Map<String, Value>,
}
impl WorkerJob {
    /// Constructs a worker job with no optional metadata.
    pub fn new(id: impl Into<String>, input: Value) -> Self {
        Self {
            id: id.into(),
            input,
            webhook: None,
            metadata: Default::default(),
        }
    }
}
impl PartialEq for WorkerJob {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for WorkerJob {}
impl Hash for WorkerJob {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
impl fmt::Display for WorkerJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.id.fmt(f)
    }
}

/// Shared in-process tracker whose clones observe the same running jobs.
#[derive(Debug, Clone, Default)]
pub struct JobsProgress {
    state: Arc<Mutex<State>>,
}
#[derive(Debug, Default)]
struct State {
    jobs: BTreeMap<String, WorkerJob>,
    mirror: Option<PingJobMirror>,
}
impl State {
    fn job_list(&self) -> Option<String> {
        (!self.jobs.is_empty()).then(|| {
            self.jobs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
    }
    fn notify_mirror(&self) -> Option<super::mirror::Publication> {
        self.mirror
            .as_ref()
            .map(|mirror| mirror.publish(self.job_list().as_deref()))
    }
}
impl JobsProgress {
    /// Creates an independent worker's tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Attaches or detaches the heartbeat snapshot, immediately publishing current jobs.
    /// Clones of this tracker share the attachment. Replacing it leaves the old mirror
    /// with its last snapshot, matching the source SDK.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    pub fn set_mirror(&self, mirror: Option<PingJobMirror>) {
        let mut state = self.state.lock().expect("job tracker lock poisoned");
        state.mirror = mirror;
        let publication = state.notify_mirror();
        drop(state);
        if let Some(publication) = publication {
            publication.report();
        }
    }
    /// Registers a job; duplicate IDs preserve the original job.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    pub fn add(&self, job: WorkerJob) {
        let mut state = self.state.lock().expect("job tracker lock poisoned");
        state.jobs.entry(job.id.clone()).or_insert(job);
        // Publish under the same lock so a slower mutation cannot overwrite a newer one.
        // Mirror operations never acquire this tracker lock.
        let publication = state.notify_mirror();
        drop(state);
        if let Some(publication) = publication {
            publication.report();
        }
    }
    /// Removes a job if present.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    pub fn remove(&self, id: &str) {
        let mut state = self.state.lock().expect("job tracker lock poisoned");
        state.jobs.remove(id);
        let publication = state.notify_mirror();
        drop(state);
        if let Some(publication) = publication {
            publication.report();
        }
    }
    /// Removes every tracked job.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    pub fn clear(&self) {
        let mut state = self.state.lock().expect("job tracker lock poisoned");
        state.jobs.clear();
        let publication = state.notify_mirror();
        drop(state);
        if let Some(publication) = publication {
            publication.report();
        }
    }
    /// Retrieves a snapshot of a job by identifier.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<WorkerJob> {
        self.state
            .lock()
            .expect("job tracker lock poisoned")
            .jobs
            .get(id)
            .cloned()
    }
    /// Returns the number of tracked jobs.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    #[must_use]
    pub fn get_job_count(&self) -> usize {
        self.state
            .lock()
            .expect("job tracker lock poisoned")
            .jobs
            .len()
    }
    /// Tests whether the tracker is empty.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.get_job_count() == 0
    }
    /// Returns comma-separated job IDs, or None when empty.
    ///
    /// # Panics
    /// Panics if an earlier internal panic poisoned the tracker lock.
    #[must_use]
    pub fn get_job_list(&self) -> Option<String> {
        self.state
            .lock()
            .expect("job tracker lock poisoned")
            .job_list()
    }
}

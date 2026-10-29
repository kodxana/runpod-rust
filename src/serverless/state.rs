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
    jobs: Arc<Mutex<BTreeMap<String, WorkerJob>>>,
}
impl JobsProgress {
    /// Creates an independent worker's tracker.
    pub fn new() -> Self {
        Self::default()
    }
    /// Registers a job; duplicate IDs preserve the original job.
    pub fn add(&self, job: WorkerJob) {
        self.jobs
            .lock()
            .expect("job tracker lock poisoned")
            .entry(job.id.clone())
            .or_insert(job);
    }
    /// Removes a job if present.
    pub fn remove(&self, id: &str) {
        self.jobs
            .lock()
            .expect("job tracker lock poisoned")
            .remove(id);
    }
    /// Removes every tracked job.
    pub fn clear(&self) {
        self.jobs.lock().expect("job tracker lock poisoned").clear();
    }
    /// Retrieves a snapshot of a job by identifier.
    pub fn get(&self, id: &str) -> Option<WorkerJob> {
        self.jobs
            .lock()
            .expect("job tracker lock poisoned")
            .get(id)
            .cloned()
    }
    /// Returns the number of tracked jobs.
    pub fn get_job_count(&self) -> usize {
        self.jobs.lock().expect("job tracker lock poisoned").len()
    }
    /// Tests whether the tracker is empty.
    pub fn is_empty(&self) -> bool {
        self.get_job_count() == 0
    }
    /// Returns comma-separated job IDs, or None when empty.
    pub fn get_job_list(&self) -> Option<String> {
        let jobs = self.jobs.lock().expect("job tracker lock poisoned");
        if jobs.is_empty() {
            None
        } else {
            Some(
                jobs.keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
    }
}

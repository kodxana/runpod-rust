use std::sync::{Arc, Mutex};

/// Source SDK capacity in bytes, including the terminating NUL byte.
pub const PING_MIRROR_CAPACITY: usize = 65_536;

/// Bounded job-ID snapshot that can be shared across native threads.
///
/// Clones share one buffer; independently constructed mirrors never share state or files.
/// This replaces Python's process-shared buffer with owned, thread-shared memory. It is
/// not an inter-process transport. Reads and writes are best effort on a poisoned lock.
#[derive(Debug, Clone)]
pub struct PingJobMirror {
    buffer: Arc<Mutex<Box<[u8]>>>,
}
pub(super) enum Publication {
    Written,
    Truncated(usize),
    Unavailable,
}
impl Publication {
    pub(super) fn report(self) {
        match self {
            Self::Written => (),
            Self::Truncated(limit) => tracing::warn!(
                limit,
                "PingJobMirror: job-id snapshot exceeded capacity; truncated"
            ),
            Self::Unavailable => {
                tracing::error!("PingJobMirror.set failed: snapshot lock poisoned");
            }
        }
    }
}
impl Default for PingJobMirror {
    fn default() -> Self {
        Self::with_capacity(PING_MIRROR_CAPACITY)
    }
}
impl PingJobMirror {
    /// Creates an empty snapshot using the SDK's default byte capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an independent fixed-size buffer, including one reserved NUL byte.
    /// Zero capacity stores no text. Allocation failures follow Rust's allocator policy.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(vec![0; capacity].into_boxed_slice())),
        }
    }

    /// Publishes job IDs, preserving the source SDK's byte truncation and NUL behavior.
    ///
    /// Overflow cuts at the last comma within capacity minus one, if present. Without a
    /// comma it keeps that byte prefix, even if it cuts a UTF-8 character; `get` then
    /// returns None. An embedded NUL ends the visible string. None clears the snapshot.
    pub fn set(&self, job_ids: Option<&str>) {
        self.publish(job_ids).report();
    }
    // Return diagnostics so callers can release their own locks before invoking tracing.
    pub(super) fn publish(&self, job_ids: Option<&str>) -> Publication {
        let Ok(mut buffer) = self.buffer.lock() else {
            return Publication::Unavailable;
        };
        let mut data = job_ids.unwrap_or_default().as_bytes();
        let limit = buffer.len().saturating_sub(1);
        let mut result = Publication::Written;
        if data.len() > limit {
            data = &data[..limit];
            if let Some(cut) = data.iter().rposition(|byte| *byte == b',') {
                data = &data[..cut];
            }
            result = Publication::Truncated(limit);
        }
        buffer.fill(0);
        buffer[..data.len()].copy_from_slice(data);
        result
    }

    /// Reads the current text, or None for an empty, invalid UTF-8, or unavailable snapshot.
    pub fn get(&self) -> Option<String> {
        let buffer = self.buffer.lock().ok()?;
        let end = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        if end == 0 {
            return None;
        }
        std::str::from_utf8(&buffer[..end]).ok().map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn poisoned_mirror_is_best_effort_and_does_not_poison_the_tracker() {
        let mirror = super::PingJobMirror::new();
        let broken = mirror.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = broken.buffer.lock().unwrap();
                panic!("injected mirror lock failure");
            })
            .join()
            .is_err()
        );
        let jobs = crate::serverless::JobsProgress::new();
        jobs.set_mirror(Some(mirror.clone()));
        jobs.add(crate::serverless::WorkerJob::new("job", crate::json!(null)));
        assert_eq!(jobs.get_job_count(), 1);
        jobs.remove("job");
        jobs.clear();
        assert_eq!(mirror.get(), None);
    }
}

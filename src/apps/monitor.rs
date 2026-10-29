//! Best-effort worker metrics and container logs while an application call is in flight.
use super::{
    AppsApiClient,
    events::{EventSink, emit},
    logs::{LogStreamOptions, LogsClient},
};
use crate::{Error, Result, Value, client::truthy, control::python_string, json};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use futures_util::StreamExt;
use regex::Regex;
use serde_json::Map;
use std::{
    collections::HashSet,
    fmt,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

/// Interval between best-effort endpoint worker summary polls.
pub const METRICS_POLL_INTERVAL: Duration = Duration::from_secs(2);
const TRACKED_STATES: [&str; 5] = ["initializing", "ready", "running", "throttled", "unhealthy"];

fn timestamp_prefix() -> &'static Regex {
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    PREFIX.get_or_init(|| {
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?\s*")
            .expect("constant timestamp pattern is valid")
    })
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)
}

/// Recognizes an SDK structured log frame, leaving ordinary JSON user output alone.
pub fn worker_frame(line: &str) -> Option<Value> {
    if !line.starts_with('{') {
        return None;
    }
    let frame: Value = serde_json::from_str(line).ok()?;
    (frame.is_object() && frame.get("message").is_some() && frame.get("level").is_some())
        .then_some(frame)
}
/// Keeps user output and SDK warning/error messages, hiding routine SDK noise.
///
/// # Errors
/// Returns malformed SDK-frame errors when its level is not a string.
pub fn filter_line(line: &str) -> Result<Option<String>> {
    if line.starts_with("--- Starting Serverless Worker") {
        return Ok(None);
    }
    let Some(frame) = worker_frame(line) else {
        return Ok(Some(line.into()));
    };
    let level = frame["level"]
        .as_str()
        .ok_or_else(|| Error::InvalidResponse("worker log level must be a string".into()))?;
    Ok(
        matches!(level.to_uppercase().as_str(), "ERROR" | "WARN" | "WARNING")
            .then(|| python_string(&frame["message"])),
    )
}
/// Compares a leading ISO log timestamp with a start time, including equality.
///
/// Nanoseconds are truncated to Python's microsecond precision. Missing or invalid
/// timestamps return false; mixing timezone-aware and timezone-free values is an error.
///
/// # Errors
/// Returns an error when only one timestamp specifies a timezone.
pub fn line_before(raw: &str, since_iso: &str) -> Result<bool> {
    let Some(prefix) = timestamp_prefix().find(raw) else {
        return Ok(false);
    };
    fn parse(value: &str) -> Option<(bool, i64)> {
        if let Ok(date) = DateTime::parse_from_rfc3339(value) {
            return Some((true, date.timestamp_micros()));
        }
        NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
            .ok()
            .map(|date| (false, date.and_utc().timestamp_micros()))
    }
    let (Some((line_aware, line)), Some((since_aware, since))) =
        (parse(prefix.as_str().trim()), parse(since_iso))
    else {
        return Ok(false);
    };
    if line_aware != since_aware {
        return Err(Error::InvalidResponse(
            "can't compare offset-naive and offset-aware datetimes".into(),
        ));
    }
    Ok(line <= since)
}
/// Formats nonzero counts in their input order, or "no workers" when empty.
pub fn format_worker_counts(counts: &Map<String, Value>) -> String {
    let parts: Vec<_> = counts
        .iter()
        .filter(|(_, count)| truthy(count))
        .map(|(state, count)| format!("{} {state}", python_string(count)))
        .collect();
    if parts.is_empty() {
        "no workers".into()
    } else {
        parts.join(", ")
    }
}

struct Task(tokio::task::JoinHandle<()>);
impl Task {
    fn abort(&self) {
        self.0.abort();
    }
    async fn stop(mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct LogState {
    since: String,
    lines_emitted: u64,
    seen: HashSet<(String, String)>,
}
struct LogContext {
    logs: LogsClient,
    pod_id: String,
    name: String,
    events: EventSink,
    endpoint_id: Option<String>,
    state: Mutex<LogState>,
}
impl LogContext {
    fn emit_line(&self, key: String, line: &str) -> Result<()> {
        // Never invoke user callbacks while holding the state lock.
        if !self
            .state
            .lock()
            .expect("log state lock is not poisoned")
            .seen
            .insert((key, line.into()))
        {
            return Ok(());
        }
        if let Some(shown) = filter_line(line)?.filter(|line| !line.is_empty()) {
            emit(
                Some(&self.events),
                "worker_log",
                &[json!(self.name), json!(shown)],
            );
        }
        Ok(())
    }
    async fn snapshot(&self) -> Result<()> {
        let logs = self
            .logs
            .pod_logs(&self.pod_id, "container", Duration::from_secs(30))
            .await?;
        let since = self
            .state
            .lock()
            .expect("log state lock is not poisoned")
            .since
            .clone();
        if let Some(entries) = logs.get("container").filter(|v| truthy(v)) {
            for raw in entries
                .as_array()
                .ok_or_else(|| Error::InvalidResponse("container logs must be an array".into()))?
            {
                let raw = raw.as_str().ok_or_else(|| {
                    Error::InvalidResponse("container log must be a string".into())
                })?;
                if line_before(raw, &since)? {
                    continue;
                }
                let raw = raw.trim_end();
                let line = timestamp_prefix().replace(raw, "");
                if !line.is_empty() {
                    self.emit_line(raw.into(), &line)?;
                }
            }
        }
        Ok(())
    }
    async fn follow(&self) {
        let mut attempt: u64 = 0;
        loop {
            let since = self
                .state
                .lock()
                .expect("log state lock is not poisoned")
                .since
                .clone();
            let mut stream = self.logs.stream_pod_logs(
                &self.pod_id,
                LogStreamOptions {
                    endpoint_id: self.endpoint_id.clone(),
                    log_type: Some("container".into()),
                    tail: 1000,
                    since: Some(since),
                    ..Default::default()
                },
                None,
            );
            let mut streamed = false;
            let result: Result<()> = async {
                while let Some(event) = stream.next().await {
                    let event = event?;
                    streamed = true;
                    attempt = 0;
                    let raw = event
                        .get("line")
                        .filter(|v| truthy(v))
                        .unwrap_or(&Value::Null);
                    let raw = if raw.is_null() {
                        ""
                    } else {
                        raw.as_str().ok_or_else(|| {
                            Error::InvalidResponse("streamed log line must be a string".into())
                        })?
                    }
                    .trim_end();
                    let line = timestamp_prefix().replace(raw, "");
                    if !line.is_empty() {
                        let mut state = self.state.lock().expect("log state lock is not poisoned");
                        state.lines_emitted = state.lines_emitted.saturating_add(1);
                        drop(state);
                        let key = event
                            .get("ts")
                            .filter(|v| truthy(v))
                            .map_or_else(|| raw.to_owned(), python_string);
                        self.emit_line(key, &line)?;
                    }
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                tracing::debug!(pod_id=self.pod_id,attempt=attempt+1,%error,"log stream attach failed");
            }
            if streamed {
                self.state
                    .lock()
                    .expect("log state lock is not poisoned")
                    .since = now();
                continue;
            }
            attempt = attempt.saturating_add(1);
            tokio::time::sleep(if attempt <= 20 {
                Duration::from_millis(500)
            } else {
                Duration::from_secs((attempt - 20).min(15))
            })
            .await;
        }
    }
}

/// Follows one pod's logs with reconnect deduplication and a final snapshot fallback.
///
/// Call stop to recover short-job logs. Dropping the owner aborts its background task.
pub struct PodLogStream {
    context: Arc<LogContext>,
    task: Option<Task>,
}
impl fmt::Debug for PodLogStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PodLogStream")
            .field("pod_id", &self.context.pod_id)
            .field("resource", &self.context.name)
            .field("attached", &self.task.is_some())
            .finish_non_exhaustive()
    }
}
impl PodLogStream {
    /// Binds a pod and captures the earliest timestamp to show in a fallback snapshot.
    pub fn new(
        logs: LogsClient,
        pod_id: impl Into<String>,
        resource_name: impl Into<String>,
        events: EventSink,
        endpoint_id: Option<String>,
    ) -> Self {
        Self {
            context: Arc::new(LogContext {
                logs,
                pod_id: pod_id.into(),
                name: resource_name.into(),
                events,
                endpoint_id,
                state: Mutex::new(LogState {
                    since: now(),
                    lines_emitted: 0,
                    seen: HashSet::new(),
                }),
            }),
            task: None,
        }
    }
    /// Starts following logs once; repeated attachment is a no-op.
    ///
    /// # Errors
    /// Returns an error if called outside a Tokio runtime.
    pub fn attach(&mut self) -> Result<()> {
        if self.task.is_none() {
            let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
                Error::InvalidInput("log attachment requires a Tokio runtime".into())
            })?;
            let context = self.context.clone();
            self.task = Some(Task(runtime.spawn(async move {
                context.follow().await;
            })));
        }
        Ok(())
    }
    fn abort(&self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
    /// Cancels the follower and recovers a snapshot if no nonempty raw line arrived.
    /// Observability errors are logged and suppressed.
    pub async fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.stop().await;
        }
        let needs_snapshot = self
            .context
            .state
            .lock()
            .expect("log state lock is not poisoned")
            .lines_emitted
            == 0;
        if needs_snapshot && let Err(error) = self.context.snapshot().await {
            tracing::debug!(pod_id=self.context.pod_id,%error,"log snapshot failed");
        }
    }
}

#[derive(Default)]
struct Counts {
    assigned: bool,
    last: Option<Map<String, Value>>,
}
struct MonitorContext {
    api: AppsApiClient,
    endpoint_id: String,
    name: String,
    events: EventSink,
    counts: Mutex<Counts>,
}
impl MonitorContext {
    fn report_counts(&self, data: &Value) {
        let Some(workers) = data.get("summary").and_then(Value::as_object) else {
            return;
        };
        let counts: Map<_, _> = TRACKED_STATES
            .into_iter()
            .map(|state| {
                let key = if state == "ready" { "idle" } else { state };
                (
                    state.into(),
                    workers
                        .get(key)
                        .filter(|v| truthy(v))
                        .cloned()
                        .unwrap_or(json!(0)),
                )
            })
            .collect();
        let mut previous = self
            .counts
            .lock()
            .expect("monitor state lock is not poisoned");
        if previous.assigned || previous.last.as_ref() == Some(&counts) {
            return;
        }
        let first = previous.last.is_none();
        previous.last = Some(counts.clone());
        drop(previous);
        if first
            && !["initializing", "throttled", "unhealthy"]
                .iter()
                .any(|key| truthy(&counts[*key]))
        {
            return;
        }
        emit(
            Some(&self.events),
            "worker_status",
            &[json!(self.name), Value::Object(counts)],
        );
    }
    async fn poll_metrics(&self) {
        loop {
            if let Ok(data) = self.api.endpoint_workers(&self.endpoint_id).await {
                self.report_counts(&data);
            }
            tokio::time::sleep(METRICS_POLL_INTERVAL).await;
        }
    }
}

/// Watches endpoint startup metrics and attaches logs when a job is assigned a worker.
/// Background tasks are canceled when this owner is dropped, including during cancellation.
pub struct WorkerMonitor {
    context: Arc<MonitorContext>,
    logs: LogsClient,
    tasks: Vec<Task>,
    streams: Vec<(String, PodLogStream)>,
}
impl fmt::Debug for WorkerMonitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerMonitor")
            .field("endpoint_id", &self.context.endpoint_id)
            .field("resource", &self.context.name)
            .field("workers", &self.streams.len())
            .finish_non_exhaustive()
    }
}
impl WorkerMonitor {
    /// Creates a monitor using explicit control-plane and log clients.
    pub fn new(
        endpoint_id: impl Into<String>,
        resource_name: impl Into<String>,
        events: EventSink,
        api: AppsApiClient,
        logs: LogsClient,
    ) -> Self {
        Self {
            context: Arc::new(MonitorContext {
                api,
                endpoint_id: endpoint_id.into(),
                name: resource_name.into(),
                events,
                counts: Mutex::new(Counts::default()),
            }),
            logs,
            tasks: Vec::new(),
            streams: Vec::new(),
        }
    }
    /// Launches a best-effort metrics poller.
    ///
    /// # Errors
    /// Returns an error if called outside a Tokio runtime.
    pub fn start(&mut self) -> Result<()> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| {
            Error::InvalidInput("worker monitoring requires a Tokio runtime".into())
        })?;
        let context = self.context.clone();
        self.tasks.push(Task(runtime.spawn(async move {
            context.poll_metrics().await;
        })));
        Ok(())
    }
    /// Observes assignment and emits worker_ready once for each worker before attaching logs.
    ///
    /// # Errors
    /// Returns an error if a new log attachment is requested outside a Tokio runtime.
    pub fn on_status(&mut self, data: &Value) -> Result<()> {
        let Some(worker) = data
            .get("workerId")
            .filter(|v| truthy(v))
            .map(python_string)
        else {
            return Ok(());
        };
        if self.streams.iter().any(|(id, _)| id == &worker) {
            return Ok(());
        }
        emit(
            Some(&self.context.events),
            "worker_ready",
            &[json!(self.context.name), json!(worker)],
        );
        let mut stream = PodLogStream::new(
            self.logs.clone(),
            &worker,
            &self.context.name,
            self.context.events.clone(),
            Some(self.context.endpoint_id.clone()),
        );
        stream.attach()?;
        self.streams.push((worker, stream));
        self.context
            .counts
            .lock()
            .expect("monitor state lock is not poisoned")
            .assigned = true;
        Ok(())
    }
    /// Reports changed summary counts until a worker is assigned, suppressing an initial steady pool.
    pub fn report_counts(&self, data: &Value) {
        self.context.report_counts(data);
    }
    /// Stops every background task and collects fallback log snapshots where needed.
    /// Log and metrics failures do not fail the application call.
    pub async fn stop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for (_, stream) in &self.streams {
            stream.abort();
        }
        while let Some(task) = self.tasks.pop() {
            task.stop().await;
        }
        for (_, stream) in &mut self.streams {
            stream.stop().await;
        }
    }
}

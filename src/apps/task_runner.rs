//! Single-execution HTTP hosting for deployed native tasks.
use super::FunctionHandle;
use crate::{Error, Result, Value, json, serverless::capture::Capture};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State as AxumState,
    http::{Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use std::{
    collections::BTreeMap,
    fmt,
    future::{Future, IntoFuture},
    sync::{Arc, Mutex},
    time::Duration,
};
mod termination;
pub use termination::PodTermination;
use tokio::{
    net::TcpListener,
    sync::{Notify, mpsc, oneshot},
    task::JoinSet,
    time::Instant,
};

/// Timeout policy for one task process; execution caps start at the first accepted call.
#[derive(Debug, Clone)]
pub struct TaskRunnerConfig {
    /// Maximum time to initialize the registered task.
    pub init_timeout: Duration,
    /// Optional execution lifetime cap, independent of client deadlines and contact.
    pub execution_timeout: Option<Duration>,
    /// Idle lifetime after initialization when no execution is active.
    pub idle_timeout: Duration,
    /// Idle checks and retry interval after an unacknowledged termination.
    pub watchdog_interval: Duration,
}
impl Default for TaskRunnerConfig {
    fn default() -> Self {
        Self {
            init_timeout: Duration::from_secs(600),
            execution_timeout: None,
            idle_timeout: Duration::from_secs(600),
            watchdog_interval: Duration::from_secs(15),
        }
    }
}

/// Why an owned task HTTP server stopped serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TaskExit {
    /// The caller requested shutdown.
    Shutdown,
    /// The termination callback acknowledged deletion of the task pod.
    Terminated,
}

/// Hosts one deployed task, preserving authentication, readiness and single-execution semantics.
pub struct TaskRunner {
    token: String,
    config: TaskRunnerConfig,
    startup_error: Option<Value>,
}
impl fmt::Debug for TaskRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskRunner")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}
impl TaskRunner {
    /// Creates an isolated runner without loading a function or starting network operations.
    ///
    /// # Errors
    /// Rejects an empty token, zero initialization/execution/check intervals, or overflowing deadlines.
    pub fn new(token: impl Into<String>, config: TaskRunnerConfig) -> Result<Self> {
        let token = token.into();
        if token.is_empty() {
            return Err(Error::InvalidInput("RUNPOD_TASK_TOKEN not set".into()));
        }
        for (key, value) in [
            ("RUNPOD_TASK_INIT_TIMEOUT", Some(config.init_timeout)),
            (
                "RUNPOD_TASK_EXECUTION_TIMEOUT_SECONDS",
                config.execution_timeout,
            ),
            ("watchdog interval", Some(config.watchdog_interval)),
        ] {
            if value
                .is_some_and(|value| value.is_zero() || Instant::now().checked_add(value).is_none())
            {
                return Err(Error::InvalidInput(format!(
                    "{key} must be finite and positive"
                )));
            }
        }
        Ok(Self {
            token,
            config,
            startup_error: None,
        })
    }

    /// Captures worker token and timeout policy without reading unrelated environment settings.
    ///
    /// # Errors
    /// Returns missing-token or invalid idle-timeout errors. Invalid initialization/execution
    /// caps are retained for authenticated bootstrap diagnostics after the listener starts.
    pub fn from_env() -> Result<Self> {
        Self::from_values(
            &[
                "RUNPOD_TASK_TOKEN",
                "RUNPOD_TASK_IDLE_TIMEOUT",
                "RUNPOD_TASK_INIT_TIMEOUT",
                "RUNPOD_TASK_EXECUTION_TIMEOUT_SECONDS",
            ]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect(),
        )
    }

    /// Captures explicit settings; invalid startup caps prevent polling the initializer.
    ///
    /// # Errors
    /// Returns missing-token or invalid idle-timeout errors.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self> {
        let mut runner = Self::new(
            values.get("RUNPOD_TASK_TOKEN").cloned().unwrap_or_default(),
            TaskRunnerConfig::default(),
        )?;
        if let Some(value) = values.get("RUNPOD_TASK_IDLE_TIMEOUT") {
            runner.config.idle_timeout = parse_seconds(value, "RUNPOD_TASK_IDLE_TIMEOUT", false)?;
        }
        for key in [
            "RUNPOD_TASK_INIT_TIMEOUT",
            "RUNPOD_TASK_EXECUTION_TIMEOUT_SECONDS",
        ] {
            if let Some(value) = values.get(key) {
                match parse_seconds(value, key, true) {
                    Ok(value) => {
                        if key == "RUNPOD_TASK_INIT_TIMEOUT" {
                            runner.config.init_timeout = value;
                        } else {
                            runner.config.execution_timeout = Some(value);
                        }
                    }
                    Err(error) => {
                        runner.startup_error = Some(bootstrap_error(format!(
                            "task initialization failed: {error}"
                        )));
                        break;
                    }
                }
            }
        }
        Ok(runner)
    }

    /// Serves an earlier artifact/bootstrap failure without polling the task initializer.
    #[must_use]
    pub fn with_bootstrap_error(mut self, error: &super::bootstrap::PhaseError) -> Self {
        self.startup_error = Some(error.payload());
        self
    }

    /// Serves readiness during initialization, then executes one authenticated JSON task request.
    ///
    /// `initialize` should load and initialize the handle, for example with
    /// [`super::runtime::DeployedContext::load_task`]. `terminate` must acknowledge pod
    /// deletion by returning true; false or an error keeps the runtime alive for retry.
    /// Deadlines survive client disconnects and cannot be extended. This library method
    /// returns [`TaskExit::Terminated`] instead of exiting the host process. The process
    /// entry point must then exit, including when blocking user code is still running.
    /// Use the SDK capture writers for response stdout; ordinary process output is not intercepted.
    ///
    /// # Errors
    /// Returns listener/HTTP failures, a poisoned state lock or a panicked execution task.
    /// Initialization and ordinary handler failures are sent over the HTTP protocol.
    pub async fn serve<F, T, TF, S>(
        self,
        listener: TcpListener,
        initialize: F,
        mut terminate: T,
        shutdown: S,
    ) -> Result<TaskExit>
    where
        F: Future<Output = Result<FunctionHandle>> + Send,
        T: FnMut() -> TF + Send,
        TF: Future<Output = Result<bool>> + Send,
        S: Future<Output = ()> + Send,
    {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut initializing = self.startup_error.is_none();
        let state = match self.startup_error {
            Some(error) => RunnerState {
                phase: Phase::Failed(error),
                last_contact: Some(Instant::now()),
                ..RunnerState::default()
            },
            None => RunnerState::default(),
        };
        let shared = Arc::new(Shared {
            token: self.token,
            config: self.config,
            jobs: sender,
            state: Mutex::new(state),
            wake: Notify::new(),
        });
        let init_deadline = Instant::now() + shared.config.init_timeout;
        let router = Router::new()
            .fallback(http_request)
            .with_state(shared.clone());
        let (stop, stopped) = oneshot::channel();
        let (closing, closed) = tokio::sync::watch::channel(());
        let server = axum::serve(
            super::transport::ClosingListener::new(listener, closed),
            router,
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .into_future();
        tokio::pin!(server, initialize, shutdown);
        let watchdog = watchdog(&shared, &mut terminate);
        tokio::pin!(watchdog);
        let mut jobs = JoinSet::new();
        let mut server_finished = false;
        let outcome = loop {
            tokio::select! {
                result=&mut server=>{server_finished=true;break result.map(|()|TaskExit::Shutdown).map_err(Error::from);},
                ()=&mut shutdown=>break Ok(TaskExit::Shutdown),
                result=&mut watchdog=>break result.map(|()|TaskExit::Terminated),
                result=&mut initialize, if initializing=>{
                    initializing=false;
                    let mut state=shared.lock()?;
                    if Instant::now()>=init_deadline {state.init_expired();}
                    else {
                        state.phase=match result {
                            Ok(handle)=>Phase::Ready(handle),
                            Err(error)=>Phase::Failed(bootstrap_error(format!("task initialization failed: {error}"))),
                        };
                        state.last_contact=Some(Instant::now());
                    }
                    drop(state); shared.wake.notify_one();
                },
                ()=tokio::time::sleep_until(init_deadline), if initializing=>{
                    initializing=false;
                    shared.lock()?.init_expired(); shared.wake.notify_one();
                },
                Some(job)=receiver.recv()=>{jobs.spawn(execute_job(shared.clone(),job));},
                Some(result)=jobs.join_next(), if !jobs.is_empty()=>{
                    match result {Ok(Ok(()))=>{},Ok(Err(error))=>break Err(error),Err(error)=>break Err(error.into())}
                },
            }
        };
        // Closing sockets also wakes incomplete request bodies and blocked response writes.
        // Otherwise graceful HTTP draining could indefinitely delay acknowledged pod deletion.
        drop(closing);
        jobs.abort_all();
        while jobs.join_next().await.is_some() {}
        let _ = stop.send(());
        // The listener may have failed before shutdown; only drain when it is still serving.
        if !server_finished {
            server.await?;
        }
        outcome
    }
}

fn parse_seconds(raw: &str, key: &str, positive: bool) -> Result<Duration> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| {
            value.is_finite()
                && if positive {
                    *value > 0.0
                } else {
                    *value >= 0.0
                }
        })
        .and_then(|value| Duration::try_from_secs_f64(value).ok())
        .filter(|value| Instant::now().checked_add(*value).is_some())
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "{key} must be finite and {}",
                if positive { "positive" } else { "non-negative" }
            ))
        })
}

/// Starts the deployed task process using environment policy, port and pod-scoped deletion credentials.
///
/// Binds all interfaces on `RUNPOD_TASK_PORT` (8080 by default). An acknowledged timeout
/// or idle deletion exits the process with code zero, including any blocking user work.
/// Use [`TaskRunner::serve`] to embed the server without process exit.
///
/// # Errors
/// Returns configuration, bind or server failures; task/bootstrap failures use HTTP responses.
pub async fn start<F>(initialize: F) -> Result<()>
where
    F: Future<Output = Result<FunctionHandle>> + Send,
{
    start_runner(TaskRunner::from_env()?, initialize).await
}

pub(super) async fn start_failed(error: &super::bootstrap::PhaseError) -> Result<()> {
    start_runner(
        TaskRunner::from_env()?.with_bootstrap_error(error),
        std::future::pending(),
    )
    .await
}

async fn start_runner<F>(runner: TaskRunner, initialize: F) -> Result<()>
where
    F: Future<Output = Result<FunctionHandle>> + Send,
{
    let port = std::env::var("RUNPOD_TASK_PORT")
        .unwrap_or_else(|_| "8080".into())
        .trim()
        .parse::<u16>()
        .map_err(|_| Error::InvalidInput("RUNPOD_TASK_PORT must be a port number".into()))?;
    let deletion = PodTermination::from_env()?;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    if runner
        .serve(listener, initialize, || deletion.terminate(), async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?
        == TaskExit::Terminated
    {
        std::process::exit(0);
    }
    Ok(())
}

enum Phase {
    Initializing,
    Ready(FunctionHandle),
    Failed(Value),
}
enum JobState {
    None,
    Inline,
    Running,
    Done(Option<Value>),
}
struct RunnerState {
    phase: Phase,
    job: JobState,
    last_contact: Option<Instant>,
    deadline: Option<Instant>,
    init_timeout: bool,
}
impl Default for RunnerState {
    fn default() -> Self {
        Self {
            phase: Phase::Initializing,
            job: JobState::None,
            last_contact: None,
            deadline: None,
            init_timeout: false,
        }
    }
}
impl RunnerState {
    fn init_expired(&mut self) {
        self.phase = Phase::Failed(bootstrap_error("task initialization timed out".into()));
        self.init_timeout = true;
    }
    fn set_timeout(&mut self, timeout: Option<Duration>) -> bool {
        if let Some(deadline) = timeout.and_then(|timeout| Instant::now().checked_add(timeout))
            && self.deadline.is_none_or(|previous| deadline < previous)
        {
            self.deadline = Some(deadline);
            return true;
        }
        false
    }
    fn result(&self) -> Value {
        match &self.job {
            JobState::None | JobState::Inline => json!({"status":"NONE","response":null}),
            JobState::Running => json!({"status":"RUNNING","response":null}),
            JobState::Done(response) => json!({"status":"DONE","response":response}),
        }
    }
    fn available(&self, public: bool) -> Option<Response> {
        match &self.phase {
            Phase::Initializing => Some(reply(503, json!({"ready":false}))),
            Phase::Failed(error) => {
                let mut value = json!({"ready":false,"error_type":"BootstrapError"});
                if !public {
                    value["error_message"] = error["error_message"].clone();
                }
                Some(reply(500, value))
            }
            Phase::Ready(_) => None,
        }
    }
}
struct Shared {
    token: String,
    config: TaskRunnerConfig,
    state: Mutex<RunnerState>,
    jobs: mpsc::UnboundedSender<Job>,
    wake: Notify,
}
impl Shared {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RunnerState>> {
        self.state
            .lock()
            .map_err(|_| super::AppError::StatePoisoned("task runner").into())
    }
}
struct Job {
    handle: FunctionHandle,
    args: Vec<Value>,
    kwargs: serde_json::Map<String, Value>,
    reply: Option<oneshot::Sender<Value>>,
}

fn reply(code: u16, value: Value) -> Response {
    (
        StatusCode::from_u16(code).expect("fixed HTTP status"),
        Json(value),
    )
        .into_response()
}
fn bootstrap_error(message: String) -> Value {
    let mut error = json!({"error_type":"BootstrapError"});
    error["error_message"] = Value::String(message);
    error
}

async fn http_request(
    AxumState(shared): AxumState<Arc<Shared>>,
    request: Request<Body>,
) -> Response {
    match dispatch(&shared, request).await {
        Ok(response) => response,
        Err(error) => reply(500, json!({"error":error.to_string()})),
    }
}
async fn dispatch(shared: &Shared, request: Request<Body>) -> Result<Response> {
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .to_owned();
    let method = request.method();
    if method == Method::GET && path == "/ping" {
        return Ok(shared
            .lock()?
            .available(true)
            .unwrap_or_else(|| reply(200, json!({"ready":true}))));
    }
    if method == Method::GET && path != "/result" {
        return Ok(reply(404, json!({"error":"not found"})));
    }
    if method != Method::GET && method != Method::POST {
        return Ok(reply(501, json!({"error":"unsupported method"})));
    }
    if request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {}", shared.token))
    {
        return Ok(reply(401, json!({"error":"unauthorized"})));
    }
    shared.lock()?.last_contact = Some(Instant::now());
    if method == Method::POST && !["/execute", "/submit", "/timeout"].contains(&path.as_str()) {
        return Ok(reply(404, json!({"error":"not found"})));
    }
    if let Some(response) = shared.lock()?.available(false) {
        return Ok(response);
    }
    if method == Method::GET {
        return Ok(reply(200, shared.lock()?.result()));
    }
    let Ok(bytes) = to_bytes(request.into_body(), usize::MAX).await else {
        return Ok(reply(400, json!({"error":"invalid request"})));
    };
    let request = if bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) if value.is_object() => value,
            _ => return Ok(reply(400, json!({"error":"invalid request"}))),
        }
    };
    let timeout = match validate(&request, path == "/timeout") {
        Ok(timeout) => timeout,
        Err(error) => return Ok(reply(400, json!({"error":error.to_string()}))),
    };
    if path == "/timeout" {
        if shared.lock()?.set_timeout(timeout) {
            shared.wake.notify_one();
        }
        return Ok(reply(200, json!({"timeout":request["timeout"]})));
    }
    let (handle, changed) = {
        let mut state = shared.lock()?;
        if !matches!(state.job, JobState::None) {
            let error = if path == "/execute" {
                "this task has already been submitted"
            } else {
                "a job is already running"
            };
            return Ok(reply(409, json!({"error":error})));
        }
        let Phase::Ready(handle) = &state.phase else {
            return Ok(reply(503, json!({"ready":false})));
        };
        let handle = handle.clone();
        let changed = state.set_timeout(shared.config.execution_timeout);
        let changed = state.set_timeout(timeout) || changed;
        state.job = if path == "/execute" {
            JobState::Inline
        } else {
            JobState::Running
        };
        (handle, changed)
    };
    if changed {
        shared.wake.notify_one();
    }
    let (sender, receiver) = oneshot::channel();
    let job = Job {
        handle,
        args: request["args"].as_array().cloned().unwrap_or_default(),
        kwargs: request["kwargs"].as_object().cloned().unwrap_or_default(),
        reply: (path == "/execute").then_some(sender),
    };
    shared
        .jobs
        .send(job)
        .map_err(|_| Error::InvalidInput("task runner stopped".into()))?;
    if path == "/execute" {
        Ok(reply(
            200,
            receiver
                .await
                .map_err(|_| Error::InvalidInput("task execution stopped".into()))?,
        ))
    } else {
        let mut response = json!({"status":"RUNNING"});
        if timeout.is_some() {
            response["timeout"] = request["timeout"].clone();
        }
        Ok(reply(200, response))
    }
}

fn validate(request: &Value, timeout_only: bool) -> Result<Option<Duration>> {
    let allowed = if timeout_only {
        &["timeout"][..]
    } else {
        &["args", "kwargs", "serialization_format", "timeout"][..]
    };
    let error = |message: &str| Error::InvalidInput(message.into());
    if request
        .as_object()
        .is_some_and(|map| map.keys().any(|key| !allowed.contains(&key.as_str())))
    {
        return Err(error(
            "deployed tasks accept only json args, kwargs and timeout",
        ));
    }
    if request
        .get("serialization_format")
        .is_some_and(|value| value != "json")
    {
        return Err(error("deployed tasks require json serialization"));
    }
    if request.get("args").is_some_and(|value| !value.is_array()) {
        return Err(error("args must be a json array"));
    }
    if request
        .get("kwargs")
        .is_some_and(|value| !value.is_object())
    {
        return Err(error("kwargs must be a json object"));
    }
    if request.get("timeout").is_none_or(Value::is_null) {
        return Ok(None);
    }
    let timeout = request["timeout"]
        .as_f64()
        .filter(|timeout| timeout.is_finite() && *timeout >= 0.0)
        .and_then(|timeout| Duration::try_from_secs_f64(timeout).ok())
        .filter(|timeout| Instant::now().checked_add(*timeout).is_some())
        .ok_or_else(|| error("timeout must be a finite non-negative number"))?;
    Ok(Some(timeout))
}

async fn execute_job(shared: Arc<Shared>, job: Job) -> Result<()> {
    let capture = Capture::task_stdout();
    let result = capture.scope(job.handle.local(job.args, job.kwargs)).await;
    let response = match result {
        Ok(value) => json!({"success":true,"json_result":value,"stdout":capture.text()}),
        Err(error) => json!({"success":false,"error":error.to_string(),"stdout":capture.text()}),
    };
    shared.lock()?.job = JobState::Done(if job.reply.is_none() {
        Some(response.clone())
    } else {
        None
    });
    if let Some(sender) = job.reply {
        let _ = sender.send(response);
    }
    Ok(())
}

async fn watchdog<T, TF>(shared: &Shared, terminate: &mut T) -> Result<()>
where
    T: FnMut() -> TF,
    TF: Future<Output = Result<bool>>,
{
    loop {
        let (expired, delay) = {
            let state = shared.lock()?;
            let now = Instant::now();
            let expired = state.init_timeout
                || state.deadline.is_some_and(|deadline| now >= deadline)
                || (!matches!(state.job, JobState::Running | JobState::Inline)
                    && state.last_contact.is_some_and(|contact| {
                        now.duration_since(contact) > shared.config.idle_timeout
                    })
                    && !matches!(state.phase, Phase::Initializing));
            let delay = state
                .deadline
                .filter(|deadline| *deadline > now)
                .map_or(shared.config.watchdog_interval, |deadline| {
                    shared.config.watchdog_interval.min(deadline - now)
                });
            (expired, delay)
        };
        if expired {
            match terminate().await {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => tracing::warn!(%error,"task pod deletion was not acknowledged"),
            }
        }
        tokio::select! {()=tokio::time::sleep(delay)=>{},()=shared.wake.notified()=>{}}
    }
}

#[cfg(test)]
mod tests;

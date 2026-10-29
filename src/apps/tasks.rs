//! Ephemeral pod provisioning, runner transport, and task result handling.
use super::{AppError, DataCenter, GpuGroup, ResourceKind, ResourceSpec, images::RuntimeImages};
use super::{
    AppsApiClient,
    api::is_capacity_error,
    volume::{ComputeType, VolumeResolver, attach_pod_mounts},
};
use crate::{Error, Result, Value, client::truthy, control::python_string, json};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::{collections::BTreeSet, time::Duration};
use std::{fmt, future::Future, sync::Arc};
use tokio::{
    sync::{oneshot, watch},
    time::Instant,
};

/// Runner port exposed through the pod HTTP proxy.
pub const TASK_PORT: u16 = 8080;

/// Runtime selection and optional proxy override for private gateways or local testing.
#[derive(Debug, Clone)]
pub struct TaskConfig {
    /// Remote runtime image version/channel.
    pub images: RuntimeImages,
    /// A fixed runner origin; None selects the pod's public Runpod proxy.
    pub proxy_base_url: Option<String>,
    /// Runtime package override for custom images, captured from the process environment.
    pub runtime_package_spec: Option<String>,
    /// SDK package override for custom images, captured from the process environment.
    pub sdk_package_spec: Option<String>,
    /// Maximum time to wait for the runner's unauthenticated ping.
    pub ready_timeout: Duration,
}
impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            images: RuntimeImages::default(),
            proxy_base_url: None,
            runtime_package_spec: std::env::var("RUNPOD_RUNTIME_PACKAGE_SPEC").ok(),
            sdk_package_spec: std::env::var("RUNPOD_PACKAGE_SPEC").ok(),
            ready_timeout: Duration::from_secs(600),
        }
    }
}

/// Lists pinned platform CUDA versions at or above the requested numeric floor.
///
/// # Errors
/// Returns invalid input unless the floor has two nonnegative integer components.
pub fn cuda_versions_at_least(minimum: &str) -> Result<Vec<String>> {
    fn key(value: &str) -> Option<(u32, u32)> {
        let (major, minor) = value.split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    }
    let floor = key(minimum)
        .ok_or_else(|| Error::InvalidInput(format!("invalid CUDA version: {minimum}")))?;
    Ok([
        "11.8", "12.0", "12.1", "12.2", "12.3", "12.4", "12.5", "12.6", "12.7", "12.8", "12.9",
        "13.0",
    ]
    .into_iter()
    .filter(|v| key(v).is_some_and(|k| k >= floor))
    .map(str::to_owned)
    .collect())
}

fn device_names(spec: &ResourceSpec) -> Result<Vec<String>> {
    let selected = spec
        .gpu
        .as_ref()
        .map(super::Selection::values)
        .unwrap_or_default();
    let normalized = selected
        .iter()
        .map(|v| super::gpu::resolve_gpu_string(v))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if normalized.is_empty() || normalized.iter().any(|s| s == "any") {
        return Ok(GpuGroup::all()
            .iter()
            .flat_map(|g| g.device_names())
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect());
    }
    Ok(normalized
        .into_iter()
        .flat_map(|s| match s.parse::<GpuGroup>() {
            Ok(group) if !group.device_names().is_empty() => group
                .device_names()
                .iter()
                .map(ToString::to_string)
                .collect(),
            _ => vec![s],
        })
        .collect())
}

/// Builds the exact deployment input for one task, with an explicit random name suffix.
///
/// # Errors
/// Returns resource, hardware, datacenter, or runtime image validation errors.
pub fn pod_input(
    spec: &ResourceSpec,
    token: &str,
    task_name: &str,
    nonce: &str,
    config: &TaskConfig,
) -> Result<Value> {
    spec.validate()?;
    let mut env = serde_json::Map::new();
    env.insert("RUNPOD_TASK_TOKEN".into(), json!(token));
    env.insert("RUNPOD_TASK_PORT".into(), json!(TASK_PORT.to_string()));
    if let Some(user_env) = &spec.env {
        env.extend(
            user_env
                .iter()
                .map(|(k, v)| (k.clone(), json!(python_string(v)))),
        );
    }
    let mut pod = json!({"name":format!("task-{task_name}-{nonce}"),"imageName":config.images.image_for_spec(spec)?,
        "ports":format!("{TASK_PORT}/http"),"containerDiskInGb":spec.container_disk_gb.unwrap_or(if spec.is_cpu(){10}else{30}),"supportPublicIp":true});
    if spec.image.as_ref().is_some_and(|s| !s.is_empty()) {
        for (key, value) in [
            ("RUNPOD_RUNTIME_PACKAGE_SPEC", &config.runtime_package_spec),
            ("RUNPOD_PACKAGE_SPEC", &config.sdk_package_spec),
        ] {
            if let Some(value) = value.as_ref().filter(|s| !s.is_empty()) {
                env.insert(key.into(), json!(value));
            }
        }
        pod["dockerArgs"] = json!(super::shim::runtime_launcher(ResourceKind::Task));
    }
    pod["env"] = json!(
        env.into_iter()
            .map(|(key, value)| json!({"key":key,"value":value}))
            .collect::<Vec<_>>()
    );
    let locations = if let Some(dc) = spec.datacenter.as_ref().filter(|s| !s.values().is_empty()) {
        dc.values()
            .iter()
            .map(|s| s.parse::<DataCenter>().map(|dc| dc.to_string()))
            .collect::<Result<Vec<_>>>()?
    } else if spec.is_cpu() {
        let mut dcs = Vec::new();
        for instance in spec
            .cpu
            .as_ref()
            .map(super::Selection::values)
            .unwrap_or_default()
        {
            let pool: &[&str] = if instance.starts_with("cpu5") {
                &["EU-RO-1"]
            } else {
                &[
                    "EU-CZ-1", "EU-RO-1", "US-CA-2", "US-IL-1", "US-KS-2", "US-MO-1", "US-MO-2",
                    "US-NC-2", "US-NE-1", "US-WA-1",
                ]
            };
            for dc in pool {
                if !dcs.iter().any(|v| v == dc) {
                    dcs.push((*dc).to_owned());
                }
            }
        }
        dcs
    } else {
        DataCenter::all().iter().map(ToString::to_string).collect()
    };
    pod["dataCenterIds"] = json!(locations);
    if spec.is_cpu() {
        pod["instanceIds"] = json!(
            spec.cpu
                .as_ref()
                .map(super::Selection::values)
                .unwrap_or_default()
        );
    } else {
        pod["gpuTypeIdList"] = json!(device_names(spec)?);
        pod["gpuCount"] = json!(spec.gpu_count);
        if let Some(min) = spec.min_cuda_version.as_ref().filter(|s| !s.is_empty()) {
            pod["allowedCudaVersions"] = json!(cuda_versions_at_least(min.trim())?);
        }
    }
    Ok(pod)
}

/// Decodes a runner response, preferring the pickle result over its JSON alternative.
///
/// # Errors
/// Returns remote execution, malformed response, or pickle/base64 decoding failures.
pub fn unwrap_task_response(response: &Value) -> Result<Value> {
    if !response.is_object() {
        return Err(Error::InvalidResponse(
            "task response must be an object".into(),
        ));
    }
    if !response.get("success").is_some_and(truthy) {
        let error = response
            .get("error")
            .map_or_else(|| "unknown".into(), python_string);
        return Err(AppError::RemoteExecution(format!("task execution failed: {error}")).into());
    }
    if let Some(result) = response.get("result").filter(|v| !v.is_null()) {
        let encoded = result
            .as_str()
            .ok_or_else(|| Error::InvalidResponse("task result must be base64 text".into()))?;
        return super::serialization::deserialize_result(encoded);
    }
    Ok(response.get("json_result").cloned().unwrap_or(Value::Null))
}

#[derive(Default)]
struct ExecutionState {
    pod_id: Option<String>,
    deployment: Option<watch::Receiver<bool>>,
}
struct ExecutionInner {
    spec: ResourceSpec,
    specs: Vec<ResourceSpec>,
    config: TaskConfig,
    api: AppsApiClient,
    http: reqwest::Client,
    token: String,
    state: watch::Sender<ExecutionState>,
    termination: tokio::sync::Mutex<()>,
}

/// One task pod, with shared ownership of its recorded deployment and cleanup state.
///
/// Creation continues if its awaiting future is dropped. Call `terminate` to recover
/// and delete a late-created pod, or use `PodTarget`/`TaskJob` for automatic scoped cleanup.
/// Each execution is single-use; construct a new one for another invocation.
#[derive(Clone)]
pub struct TaskExecution(Arc<ExecutionInner>);
impl fmt::Debug for TaskExecution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskExecution")
            .field("resource", &self.0.spec.name)
            .field("pod_id", &self.pod_id())
            .finish_non_exhaustive()
    }
}
impl TaskExecution {
    /// Creates one execution using the standard runtime and proxy configuration.
    ///
    /// # Errors
    /// Returns TLS initialization or operating-system randomness errors.
    pub fn new(spec: ResourceSpec, api: AppsApiClient) -> Result<Self> {
        Self::with_config(spec, api, TaskConfig::default(), None)
    }

    /// Creates an execution with optional sibling resources for shared-volume placement.
    ///
    /// # Errors
    /// Returns invalid proxy URLs, TLS initialization, or system randomness errors.
    pub fn with_config(
        spec: ResourceSpec,
        api: AppsApiClient,
        config: TaskConfig,
        specs: Option<Vec<ResourceSpec>>,
    ) -> Result<Self> {
        let http = crate::Client::anonymous(
            config
                .proxy_base_url
                .clone()
                .unwrap_or_else(|| "https://api.runpod.io".into()),
        )?
        .http;
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
        Ok(Self(Arc::new(ExecutionInner {
            specs: specs.unwrap_or_else(|| vec![spec.clone()]),
            spec,
            config,
            api,
            http,
            token: URL_SAFE_NO_PAD.encode(bytes),
            state: watch::channel(ExecutionState::default()).0,
            termination: tokio::sync::Mutex::new(()),
        })))
    }

    /// Returns the pod ID, retaining it after a failed termination so cleanup can be retried.
    #[must_use]
    pub fn pod_id(&self) -> Option<String> {
        self.0.state.borrow().pod_id.clone()
    }

    /// Provisions the pod, resolving registry credentials and sibling-aware volume placement.
    ///
    /// # Errors
    /// Returns validation/provisioning errors or an error if this execution was already started.
    pub async fn start(&self) -> Result<()> {
        let mut bytes = [0u8; 4];
        getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
        let nonce = format!("{:08x}", u32::from_be_bytes(bytes));
        let mut pod = pod_input(
            &self.0.spec,
            &self.0.token,
            &self.0.spec.name,
            &nonce,
            &self.0.config,
        )?;
        if let Some(id) = super::registry::resolve_registry_auth(
            self.0.spec.registry_auth.as_deref(),
            &self.0.api,
        )
        .await?
        {
            pod["containerRegistryAuthId"] = json!(id);
        }
        attach_pod_mounts(
            &mut pod,
            &self.0.spec,
            &mut VolumeResolver::new(self.0.api.clone()),
            &self.0.specs,
        )
        .await?;
        let (done, completion) = watch::channel(false);
        let started = self.0.state.send_if_modified(|state| {
            if state.deployment.is_some() {
                return false;
            }
            state.deployment = Some(completion);
            true
        });
        if !started {
            return Err(Error::InvalidInput(
                "task execution was already started".into(),
            ));
        }
        let (sender, receiver) = oneshot::channel();
        let execution = self.clone();
        tokio::spawn(async move {
            let result = execution.deploy_pod(&pod).await.and_then(|pod| {
                let id = crate::client::required_str(&pod, "id")?.to_owned();
                execution
                    .0
                    .state
                    .send_modify(|state| state.pod_id = Some(id.clone()));
                tracing::info!(pod_id=%id,resource=%execution.0.spec.name,"task pod deployed");
                Ok(())
            });
            done.send_replace(true);
            let _ = sender.send(result);
        });
        receiver.await.map_err(|_| {
            Error::InvalidResponse("task deployment stopped before recording its result".into())
        })?
    }

    async fn deploy_pod(&self, pod: &Value) -> Result<Value> {
        let compute = if self.0.spec.is_cpu() {
            ComputeType::Cpu
        } else {
            ComputeType::Gpu
        };
        let original = match self.0.api.deploy_task_pod(pod, compute).await {
            Err(error @ Error::Query { .. }) if is_capacity_error(&error) => error,
            result => return result,
        };
        let dcs = pod["dataCenterIds"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        if dcs.len() < 2 {
            return Err(original);
        }
        for dc in dcs {
            let mut candidate = pod.clone();
            candidate["dataCenterIds"] = json!([dc]);
            match self.0.api.deploy_task_pod(&candidate, compute).await {
                Err(error @ Error::Query { .. }) if is_capacity_error(&error) => {}
                result => return result,
            }
        }
        Err(original)
    }

    fn proxy_url(&self, path: &str) -> Result<String> {
        let id = self
            .pod_id()
            .ok_or_else(|| Error::InvalidInput("task pod is not running".into()))?;
        let base = if let Some(base) = &self.0.config.proxy_base_url {
            base.trim_end_matches('/').to_owned()
        } else {
            if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
                return Err(Error::InvalidResponse(
                    "pod ID cannot be used in a proxy hostname".into(),
                ));
            }
            format!("https://{id}-{TASK_PORT}.proxy.runpod.net")
        };
        Ok(format!("{base}{path}"))
    }

    /// Polls the unauthenticated readiness endpoint with five-second individual requests.
    ///
    /// # Errors
    /// Returns invalid pod state or a deadline error when no successful ping arrives.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let url = self.proxy_url("/ping")?;
        let deadline = deadline(Some(timeout))?;
        while !expired(deadline) {
            if let Ok(response) = self
                .0
                .http
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                && response.status() == reqwest::StatusCode::OK
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(Error::DeadlineExceeded(format!(
            "task pod {} did not become ready within {}s",
            self.pod_id().unwrap_or_default(),
            timeout.as_secs_f64()
        )))
    }

    /// Submits once and polls until completion. None means no total execution deadline.
    ///
    /// # Errors
    /// Returns runner transport, timeout acknowledgement, response, or deadline errors.
    pub async fn execute(&self, request: &Value, timeout: Option<Duration>) -> Result<Value> {
        let mut request = request
            .as_object()
            .cloned()
            .ok_or_else(|| Error::InvalidInput("task request must be an object".into()))?;
        request.insert("timeout".into(), json!(timeout.map(|t| t.as_secs_f64())));
        self.submit(&Value::Object(request)).await?;
        let deadline = deadline(timeout)?;
        loop {
            if expired(deadline) {
                return Err(self.wait_timeout(timeout));
            }
            if let Some(response) = self.poll_result().await? {
                return Ok(response);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Starts the runner's background slot, retrying only explicit proxy 404 responses.
    ///
    /// # Errors
    /// Returns invalid timeouts, transport failures, or missing timeout acknowledgements.
    pub async fn submit(&self, request: &Value) -> Result<()> {
        self.post("/submit", request).await
    }

    /// Sets a runner deadline and requires its acknowledgement before returning.
    ///
    /// # Errors
    /// Returns transport errors or an acknowledgement mismatch.
    pub async fn set_timeout(&self, timeout: Duration) -> Result<()> {
        self.post("/timeout", &json!({"timeout":timeout.as_secs_f64()}))
            .await
    }

    async fn post(&self, path: &str, request: &Value) -> Result<()> {
        if !request.is_object() {
            return Err(Error::InvalidInput("task request must be an object".into()));
        }
        let timeout = request.get("timeout").filter(|v| !v.is_null());
        if timeout.is_some_and(|v| !v.as_f64().is_some_and(|t| t.is_finite() && t >= 0.0)) {
            return Err(Error::InvalidInput(
                "task timeout must be a finite non-negative number".into(),
            ));
        }
        let response = self
            .runner_request(reqwest::Method::POST, path, Some(request))
            .await?;
        if let Some(timeout) = timeout {
            let response: Value = response.json().await?;
            let ack = response.get("timeout").and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_bool().map(|b| if b { 1.0 } else { 0.0 }))
            });
            if !response.is_object() || ack != timeout.as_f64() {
                return Err(Error::InvalidResponse(
                    "task runtime did not acknowledge the timeout; update the task runtime image"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    async fn runner_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::Response> {
        let url = self.proxy_url(path)?;
        for attempt in 0..6 {
            let mut request = self
                .0
                .http
                .request(method.clone(), &url)
                .bearer_auth(&self.0.token)
                .timeout(Duration::from_secs(30));
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request.send().await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND && attempt < 5 {
                drop(response);
                tokio::time::sleep(Duration::from_secs(2 * (attempt + 1))).await;
                continue;
            }
            return Ok(response.error_for_status()?);
        }
        unreachable!("the final attempt always returns")
    }

    /// Fetches a completed runner response, or None while its task remains pending.
    ///
    /// # Errors
    /// Returns HTTP, JSON, or response shape errors; persistent 404s are errors.
    pub async fn poll_result(&self) -> Result<Option<Value>> {
        let data: Value = self
            .runner_request(reqwest::Method::GET, "/result", None)
            .await?
            .json()
            .await?;
        if !data.is_object() {
            return Err(Error::InvalidResponse(
                "task result envelope must be an object".into(),
            ));
        }
        Ok(if data["status"] == "DONE" {
            data.get("response").filter(|v| !v.is_null()).cloned()
        } else {
            None
        })
    }

    /// Waits for pending creation, then deletes the recorded pod with bounded retries.
    ///
    /// A 404 is success. Other errors retain the ID for manual retry. Concurrent
    /// terminators serialize so a successfully deleted pod is never deleted twice.
    ///
    /// # Errors
    /// Returns the final deletion error after retrying only transient failures.
    pub async fn terminate(&self) -> Result<()> {
        let _guard = self.0.termination.lock().await;
        let pending = self.0.state.borrow().deployment.clone();
        if let Some(mut done) = pending {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        let Some(id) = self.pod_id() else {
            return Ok(());
        };
        for attempt in 0..3 {
            let result =
                tokio::time::timeout(Duration::from_secs(5), self.0.api.terminate_pod(&id))
                    .await
                    .unwrap_or_else(|_| {
                        Err(Error::DeadlineExceeded(format!(
                            "timed out terminating task pod {id}"
                        )))
                    });
            match result {
                Ok(()) => {}
                Err(error) if error.status_code() == Some(404) => {}
                Err(error) => {
                    if attempt < 2 && retryable_delete(&error) {
                        tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                        continue;
                    }
                    tracing::warn!(pod_id=%id,%error,"failed to terminate task pod; terminate it in the console");
                    return Err(error);
                }
            }
            self.0.state.send_modify(|state| state.pod_id = None);
            tracing::info!(pod_id=%id,"task pod terminated");
            return Ok(());
        }
        unreachable!("the final attempt always returns")
    }

    fn wait_timeout(&self, timeout: Option<Duration>) -> Error {
        Error::DeadlineExceeded(format!(
            "task on pod {} did not finish in {}s",
            self.pod_id().unwrap_or_default(),
            timeout.unwrap_or_default().as_secs_f64()
        ))
    }
}

fn retryable_delete(error: &Error) -> bool {
    matches!(error.status_code(), Some(429 | 500..=599))
        || match error {
            Error::Http(error) => {
                error.is_connect() || error.is_timeout() || error.is_body() || error.is_request()
            }
            Error::Io(_) | Error::DeadlineExceeded(_) => true,
            Error::Context { source, .. } => retryable_delete(source),
            _ => false,
        }
}
fn deadline(timeout: Option<Duration>) -> Result<Option<Instant>> {
    timeout
        .map(|t| {
            Instant::now().checked_add(t).ok_or_else(|| {
                Error::InvalidInput("task timeout exceeds the monotonic clock range".into())
            })
        })
        .transpose()
}
fn expired(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}

/// Runs cleanup independently, waiting at most thirty seconds without aborting it.
///
/// Dropping this future leaves the spawned cleanup running. The Tokio runtime must
/// remain alive until it finishes; Rust cannot await asynchronous work during Drop.
///
/// # Errors
/// Returns a cleanup failure or deadline error; detached failures are logged.
pub async fn finish_cleanup(
    cleanup: impl Future<Output = Result<()>> + Send + 'static,
) -> Result<()> {
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let result = cleanup.await;
        if let Err(Err(error)) = sender.send(result) {
            tracing::warn!(%error,"background task pod cleanup failed");
        }
    });
    tokio::time::timeout(Duration::from_secs(30), receiver)
        .await
        .map_err(|_| Error::DeadlineExceeded("task pod cleanup is still pending".into()))?
        .map_err(|_| {
            Error::InvalidResponse("task cleanup stopped before returning its result".into())
        })?
}

struct CleanupGuard {
    execution: Option<TaskExecution>,
    stream: Option<super::monitor::PodLogStream>,
    runtime: tokio::runtime::Handle,
}
impl CleanupGuard {
    fn new(execution: TaskExecution) -> Self {
        Self {
            execution: Some(execution),
            stream: None,
            runtime: tokio::runtime::Handle::current(),
        }
    }
    fn disarm(&mut self) {
        self.execution = None;
    }
    async fn finish<T>(&mut self, outcome: Result<T>) -> Result<T> {
        let Some(execution) = self.execution.take() else {
            return outcome;
        };
        let stream = self.stream.take();
        let cleanup = finish_cleanup(cleanup_pod(execution, stream)).await;
        match outcome {
            Err(error) => {
                if let Err(cleanup_error) = cleanup {
                    tracing::warn!(%cleanup_error,original=%error,"task cleanup failed while handling an execution error");
                }
                Err(error)
            }
            Ok(value) => cleanup.map(|()| value),
        }
    }
}
impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Some(execution) = self.execution.take() {
            let stream = self.stream.take();
            self.runtime.spawn(async move {
                if let Err(error) = cleanup_pod(execution, stream).await {
                    tracing::warn!(%error,"background task pod cleanup failed after cancellation");
                }
            });
        }
    }
}
async fn cleanup_pod(
    execution: TaskExecution,
    mut stream: Option<super::monitor::PodLogStream>,
) -> Result<()> {
    if let Some(stream) = &mut stream {
        stream.stop().await;
    }
    execution.terminate().await
}

/// A spawned task handle whose `wait` and `cancel` operations own pod cleanup.
///
/// Keep the runtime alive after canceling a wait future so its detached cleanup can
/// finish. Dropping an unused handle alone does not terminate a submitted task.
#[derive(Debug)]
pub struct TaskJob {
    execution: TaskExecution,
    result: Value,
    done: bool,
}
impl TaskJob {
    /// Wraps an execution whose task has already been submitted.
    #[must_use]
    pub fn new(execution: TaskExecution) -> Self {
        Self {
            execution,
            result: Value::Null,
            done: false,
        }
    }
    /// Returns the pod's current identifier, including after failed cleanup.
    #[must_use]
    pub fn pod_id(&self) -> Option<String> {
        self.execution.pod_id()
    }
    /// Waits for and caches the result, terminating the pod on every exit path.
    ///
    /// # Errors
    /// Returns execution/transport/deadline failures, preserving them if cleanup also fails.
    pub async fn wait(&mut self, timeout: Option<Duration>) -> Result<Value> {
        let mut cleanup = CleanupGuard::new(self.execution.clone());
        let outcome = async {
            let deadline = deadline(timeout)?;
            if !self.done
                && let Some(timeout) = timeout
            {
                self.execution.set_timeout(timeout).await?;
            }
            while !self.done {
                if expired(deadline) {
                    return Err(self.execution.wait_timeout(timeout));
                }
                if let Some(response) = self.execution.poll_result().await? {
                    self.result = unwrap_task_response(&response)?;
                    self.done = true;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(self.result.clone())
        }
        .await;
        cleanup.finish(outcome).await
    }
    /// Terminates the task, marking it done only after cleanup succeeds.
    ///
    /// # Errors
    /// Returns pod cleanup errors, retaining its identifier for a subsequent retry.
    pub async fn cancel(&mut self) -> Result<()> {
        let execution = self.execution.clone();
        finish_cleanup(async move { execution.terminate().await }).await?;
        self.done = true;
        Ok(())
    }
}

/// A reusable destination that provisions a new ephemeral pod for each invocation.
///
/// Source metadata replaces Python reflection. Values use the native pickle codec;
/// remote image selection is explicit and does not require a local interpreter.
#[derive(Clone)]
pub struct PodTarget {
    spec: ResourceSpec,
    specs: Option<Vec<ResourceSpec>>,
    function: super::targets::FunctionDefinition,
    api: AppsApiClient,
    config: TaskConfig,
    events: Option<super::events::EventSink>,
    logs: super::logs::LogsClient,
}
impl fmt::Debug for PodTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PodTarget")
            .field("resource", &self.spec.name)
            .field("function", &self.function.name)
            .finish_non_exhaustive()
    }
}
impl PodTarget {
    /// Creates a target using standard runtime image, proxy, and log service settings.
    ///
    /// # Errors
    /// Returns client or log-service configuration errors.
    pub fn new(
        client: crate::Client,
        spec: ResourceSpec,
        function: super::targets::FunctionDefinition,
    ) -> Result<Self> {
        Self::with_config(client, spec, function, TaskConfig::default(), None)
    }
    /// Creates a target with custom runner settings and optional sibling resources.
    ///
    /// # Errors
    /// Returns client or log-service configuration errors.
    pub fn with_config(
        client: crate::Client,
        spec: ResourceSpec,
        function: super::targets::FunctionDefinition,
        config: TaskConfig,
        specs: Option<Vec<ResourceSpec>>,
    ) -> Result<Self> {
        Ok(Self {
            spec,
            specs,
            function,
            api: AppsApiClient::new(client.clone())?,
            config,
            events: None,
            logs: super::logs::LogsClient::new(client)?,
        })
    }
    /// Attaches optional dispatch, status, readiness, log, and completion observers.
    pub fn set_events(&mut self, events: Option<super::events::EventSink>) {
        self.events = events;
    }
    /// Overrides log routing, useful for private gateways and local integration tests.
    pub fn set_logs_client(&mut self, logs: super::logs::LogsClient) {
        self.logs = logs;
    }

    /// Builds a source-bearing `FunctionRequest` without a queue input wrapper.
    ///
    /// # Errors
    /// Returns missing source, file, or serialization errors.
    pub async fn build_payload(
        &self,
        args: &[Value],
        kwargs: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        use super::{
            protocol::FunctionRequest,
            serialization::{get_function_source, serialize_args, serialize_kwargs},
        };
        let source = self.function.source.as_ref().ok_or_else(|| {
            Error::InvalidInput(format!(
                "task function '{}' requires module source",
                self.function.name
            ))
        })?;
        FunctionRequest {
            function_name: self.function.name.clone(),
            function_code: Some(get_function_source(source).await?),
            args: serialize_args(args)?
                .into_iter()
                .map(Value::String)
                .collect(),
            kwargs: serialize_kwargs(kwargs)?,
            dependencies: self.spec.dependencies.clone(),
            system_dependencies: self.spec.system_dependencies.clone(),
            accelerate_downloads: self.spec.accelerate_downloads,
            ..FunctionRequest::default()
        }
        .to_input()
    }

    fn execution(&self) -> Result<TaskExecution> {
        TaskExecution::with_config(
            self.spec.clone(),
            self.api.clone(),
            self.config.clone(),
            self.specs.clone(),
        )
    }

    /// Provisions, waits, executes, collects logs, and terminates a pod for one call.
    ///
    /// Dropping the future starts independent cleanup, including a pending creation.
    /// Keep the Tokio runtime alive so that cleanup can finish.
    ///
    /// # Errors
    /// Returns setup, runner, execution, or cleanup failures. An execution error takes
    /// precedence when both execution and cleanup fail; the cleanup failure is logged.
    pub async fn invoke(&self, payload: &Value, timeout: Option<Duration>) -> Result<Value> {
        use super::events::emit;
        let name = &self.spec.name;
        let hardware = if let Some(cpu) = self.spec.cpu.as_ref().filter(|s| !s.values().is_empty())
        {
            cpu.values().join(",")
        } else if let Some(gpu) = self.spec.gpu.as_ref().filter(|s| !s.values().is_empty()) {
            gpu.values()
                .iter()
                .map(|s| super::gpu::resolve_gpu_string(s))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(",")
        } else {
            "any".into()
        };
        emit(self.events.as_ref(), "dispatch", &[json!(name)]);
        emit(
            self.events.as_ref(),
            "task_status",
            &[
                json!(name),
                json!(format!("provisioning pod on {hardware}")),
            ],
        );
        let start = Instant::now();
        let execution = self.execution()?;
        let mut cleanup = CleanupGuard::new(execution.clone());
        let outcome = async {
            execution.start().await?;
            if let Some(id) = execution.pod_id() {
                let short = id.chars().take(12).collect::<String>();
                emit(
                    self.events.as_ref(),
                    "task_status",
                    &[
                        json!(name),
                        json!(format!("pod {short} waiting for runtime")),
                    ],
                );
                if let Some(events) = &self.events {
                    let stream = cleanup.stream.insert(super::monitor::PodLogStream::new(
                        self.logs.clone(),
                        id,
                        name,
                        events.clone(),
                        None,
                    ));
                    stream.attach()?;
                }
            }
            execution.wait_ready(self.config.ready_timeout).await?;
            emit(
                self.events.as_ref(),
                "worker_ready",
                &[json!(name), json!(execution.pod_id().unwrap_or_default())],
            );
            unwrap_task_response(&execution.execute(payload, timeout).await?)
        }
        .await;
        if outcome.is_err() {
            emit(
                self.events.as_ref(),
                "request_failed",
                &[json!(name), json!(start.elapsed().as_secs_f64())],
            );
        }
        let result = cleanup.finish(outcome).await?;
        emit(
            self.events.as_ref(),
            "request_completed",
            &[json!(name), json!(start.elapsed().as_secs_f64())],
        );
        Ok(result)
    }

    /// Starts a task in the background and transfers cleanup ownership to its job.
    ///
    /// # Errors
    /// Returns provisioning, readiness, or submission failures after attempting cleanup.
    pub async fn submit(&self, payload: &Value) -> Result<TaskJob> {
        let execution = self.execution()?;
        let mut cleanup = CleanupGuard::new(execution.clone());
        let outcome = async {
            execution.start().await?;
            execution.wait_ready(self.config.ready_timeout).await?;
            execution.submit(payload).await
        }
        .await;
        if let Err(error) = outcome {
            return cleanup.finish(Err(error)).await;
        }
        cleanup.disarm();
        Ok(TaskJob::new(execution))
    }
}

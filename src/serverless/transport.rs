use super::{ConcurrencyModifier, JobsProgress, WorkerJob};
use crate::{Error, Result, Value, json};
use std::{fmt, time::Duration};

/// Worker webhook addresses, concurrency limits, and output options.
#[derive(Clone)]
pub struct WorkerConfig {
    /// Worker identifier substituted into platform URL templates.
    pub worker_id: String,
    /// Raw worker authorization value, distinct from a user API key.
    pub api_key: String,
    /// Job-take URL template with `$ID` representing the worker.
    pub job_get_url: Option<String>,
    /// Final-output template with `$ID` representing the request.
    pub job_done_url: Option<String>,
    /// Partial-output template.
    pub job_stream_url: Option<String>,
    /// Worker heartbeat template.
    pub ping_url: Option<String>,
    /// Interval between heartbeats.
    pub ping_interval: Duration,
    /// Initial concurrency, before any configured modifier is applied.
    pub concurrency: usize,
    /// Optional dynamic concurrency policy, evaluated at each intake iteration.
    pub concurrency_modifier: Option<ConcurrencyModifier>,
    /// How long a full worker waits for all slots before accepting a partial refill.
    pub full_wait_interval: Duration,
    /// Recheck interval while a requested queue resize waits for existing work to finish.
    pub scale_wait_interval: Duration,
    /// Aggregate streamed values into the final output array.
    pub return_aggregate_stream: bool,
    /// Request worker refresh after each completed job.
    pub refresh_worker: bool,
    /// Optional timeout for the entire prestart phase.
    pub prestart_timeout: Option<Duration>,
    /// Timeout for the one fallback claim when startup failed before any request was held.
    pub prestart_claim_timeout: Duration,
    /// Total deadline for reporting a startup failure before returning it to the caller.
    pub prestart_failure_report_timeout: Duration,
    /// Timeout for each job-take or stop-channel long poll.
    pub request_timeout: Duration,
    /// Optional pause after empty intake or a recoverable intake failure; Python defaults to zero.
    pub idle_interval: Duration,
}
impl fmt::Debug for WorkerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerConfig")
            .field("worker_id", &self.worker_id)
            .field("api_key", &"[REDACTED]")
            .field("webhook_urls", &"[REDACTED]")
            .field("concurrency", &self.concurrency)
            .field("return_aggregate_stream", &self.return_aggregate_stream)
            .field("refresh_worker", &self.refresh_worker)
            .finish_non_exhaustive()
    }
}
impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: "local_worker".into(),
            api_key: String::new(),
            job_get_url: None,
            job_done_url: None,
            job_stream_url: None,
            ping_url: None,
            ping_interval: Duration::from_secs(10),
            concurrency: 1,
            concurrency_modifier: None,
            full_wait_interval: Duration::from_secs(1),
            scale_wait_interval: Duration::from_secs(1),
            return_aggregate_stream: false,
            refresh_worker: false,
            prestart_timeout: None,
            prestart_claim_timeout: Duration::from_secs(10),
            prestart_failure_report_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(90),
            idle_interval: Duration::ZERO,
        }
    }
}
impl WorkerConfig {
    /// Reads the platform's RUNPOD worker environment variables.
    ///
    /// # Errors
    /// Returns invalid input for malformed heartbeat configuration.
    pub fn from_env() -> Result<Self> {
        let value = |name| std::env::var(name).ok();
        let mut config = Self {
            worker_id: value("RUNPOD_POD_ID").unwrap_or_else(|| "local_worker".into()),
            api_key: value("RUNPOD_AI_API_KEY").unwrap_or_default(),
            job_get_url: value("RUNPOD_WEBHOOK_GET_JOB"),
            job_done_url: value("RUNPOD_WEBHOOK_POST_OUTPUT"),
            job_stream_url: value("RUNPOD_WEBHOOK_POST_STREAM"),
            ping_url: value("RUNPOD_WEBHOOK_PING"),
            ..Self::default()
        };
        if let Some(ms) = value("RUNPOD_PING_INTERVAL") {
            let ms = ms.parse::<u64>().map_err(|_| {
                Error::InvalidInput("RUNPOD_PING_INTERVAL must be milliseconds".into())
            })?;
            config.ping_interval = Duration::from_millis(ms);
        }
        Ok(config)
    }
}

/// Whether a final result completes a single-result or streaming invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    /// Standard output or a partial-progress update.
    Single,
    /// Final result of a streamed invocation.
    Stream,
}

/// Worker-side protocol client without global state or user API credentials.
#[derive(Debug, Clone)]
pub struct WorkerTransport {
    http: reqwest::Client,
    config: WorkerConfig,
}
impl WorkerTransport {
    /// Creates a webhook transport from explicit worker configuration.
    ///
    /// # Errors
    /// Rejects zero scheduler/heartbeat intervals and invalid header values.
    pub fn new(config: WorkerConfig) -> Result<Self> {
        if config.full_wait_interval.is_zero()
            || config.scale_wait_interval.is_zero()
            || config.ping_interval.is_zero()
        {
            return Err(Error::InvalidInput(
                "worker scheduler and ping intervals must be positive".into(),
            ));
        }
        let mut headers = reqwest::header::HeaderMap::new();
        let mut token = reqwest::header::HeaderValue::from_str(&config.api_key)
            .map_err(|_| Error::InvalidInput("invalid worker authorization header".into()))?;
        token.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, token);
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(config.request_timeout)
            .build()?;
        Ok(Self { http, config })
    }
    /// Returns the configuration, with credentials redacted by Debug.
    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }
    fn worker_url(&self, template: &str) -> String {
        template.replace("$RUNPOD_POD_ID", &self.config.worker_id)
    }
    /// Derives the dedicated stop channel while retaining query routing parameters.
    pub fn job_stop_url(&self) -> Option<String> {
        let url = self
            .config
            .job_get_url
            .as_ref()?
            .replace("$ID", &self.config.worker_id);
        url.contains("/job-take/")
            .then(|| url.replace("/job-take/", "/job-stop/"))
    }
    /// Gets one or more jobs, treating 204/400 and malformed JSON as an empty poll.
    ///
    /// # Errors
    /// Returns request errors, throttling, or an object missing required job fields.
    pub async fn get_job(
        &self,
        num_jobs: usize,
        progress: &JobsProgress,
    ) -> Result<Vec<WorkerJob>> {
        let template = self
            .config
            .job_get_url
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("job-take URL is not configured".into()))?;
        let mut url = template.replace("$ID", &self.config.worker_id);
        if num_jobs > 1 {
            url = url.replace("/job-take/", "/job-take-batch/");
        }
        let mut request = self.http.get(url);
        if num_jobs > 1 {
            request = request.query(&[("batch_size", num_jobs.to_string())]);
        }
        let response = request
            .query(&[(
                "job_in_progress",
                if progress.is_empty() { "0" } else { "1" },
            )])
            .send()
            .await?;
        if matches!(response.status().as_u16(), 204 | 400) {
            return Ok(Vec::new());
        }
        let response = response.error_for_status()?;
        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next() == Some("application/json"));
        if !is_json {
            return Ok(Vec::new());
        }
        let bytes = response.bytes().await?;
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(Vec::new());
        };
        let jobs = match value {
            Value::Object(ref object) => {
                if !object.contains_key("id") || !object.contains_key("input") {
                    return Err(Error::InvalidResponse(
                        "Job has missing field(s): id or input.".into(),
                    ));
                }
                vec![value]
            }
            Value::Array(jobs) => jobs,
            _ => return Ok(Vec::new()),
        };
        jobs.into_iter()
            .map(|job| serde_json::from_value(job).map_err(Error::from))
            .collect()
    }
    /// Long-polls cancellation IDs, filtering non-string entries like the Python SDK.
    ///
    /// # Errors
    /// Returns HTTP or transport errors; malformed payloads produce an empty list.
    pub async fn get_stop_signals(&self) -> Result<Vec<String>> {
        let Some(url) = self.job_stop_url() else {
            return Ok(Vec::new());
        };
        let response = self.http.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(Vec::new());
        }
        let response = response.error_for_status()?;
        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next() == Some("application/json"));
        if !is_json {
            return Ok(Vec::new());
        }
        let bytes = response.bytes().await?;
        let payload: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok(payload
            .get("jobsToStop")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect())
    }
    async fn transmit(
        &self,
        template: Option<&str>,
        job: &WorkerJob,
        data: &Value,
        kind: ResultKind,
    ) -> Result<()> {
        let template = template
            .ok_or_else(|| Error::InvalidInput("result webhook URL is not configured".into()))?;
        let url = self.worker_url(template).replace("$ID", &job.id);
        let serialized = serde_json::to_vec(data)?;
        for attempt in 0..3 {
            let result = self
                .http
                .post(&url)
                .query(&[(
                    "isStream",
                    if kind == ResultKind::Stream {
                        "true"
                    } else {
                        "false"
                    },
                )])
                .header("X-Request-ID", &job.id)
                .header("charset", "utf-8")
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(serialized.clone())
                .send()
                .await;
            let result = match result {
                Ok(response) => response.error_for_status(),
                Err(error) => Err(error),
            };
            match result {
                Ok(response) => {
                    response.bytes().await?;
                    return Ok(());
                }
                Err(error) if attempt < 2 && error.status().is_none_or(|s| s.is_server_error()) => {
                    tokio::time::sleep(Duration::from_secs(if attempt == 0 { 1 } else { 2 })).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(Error::InvalidResponse(
            "result delivery exhausted retries".into(),
        ))
    }
    /// Delivers a final result, with three attempts for transport and server failures.
    ///
    /// # Errors
    /// Returns the last delivery failure instead of silently losing output.
    pub async fn send_result(&self, data: &Value, job: &WorkerJob, kind: ResultKind) -> Result<()> {
        self.transmit(self.config.job_done_url.as_deref(), job, data, kind)
            .await
    }
    /// Sends a partial stream result.
    ///
    /// # Errors
    /// Returns the last delivery failure.
    pub async fn stream_result(&self, data: &Value, job: &WorkerJob) -> Result<()> {
        self.transmit(
            self.config.job_stream_url.as_deref(),
            job,
            data,
            ResultKind::Single,
        )
        .await
    }
    /// Sends a status IN_PROGRESS update through the final-output webhook.
    ///
    /// # Errors
    /// Returns the last delivery failure.
    pub async fn progress_update(&self, job: &WorkerJob, progress: Value) -> Result<()> {
        self.send_result(
            &json!({"status":"IN_PROGRESS","output":progress}),
            job,
            ResultKind::Single,
        )
        .await
    }
    /// Sends one heartbeat containing the current job snapshot.
    ///
    /// # Errors
    /// Returns transport errors; absent configuration makes this a no-op.
    pub async fn ping(&self, progress: &JobsProgress) -> Result<()> {
        let Some(template) = &self.config.ping_url else {
            return Ok(());
        };
        if self.config.api_key.is_empty() {
            return Ok(());
        }
        let mut request = self
            .http
            .get(self.worker_url(template))
            .query(&[("runpod_version", env!("CARGO_PKG_VERSION"))])
            .timeout(self.config.ping_interval.saturating_mul(2));
        if let Some(ids) = progress.get_job_list() {
            request = request.query(&[("job_id", ids)]);
        }
        request.send().await?.error_for_status()?;
        Ok(())
    }
}

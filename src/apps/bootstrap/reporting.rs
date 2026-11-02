use super::PhaseError;
use crate::{Error, Result, Value, client::truthy, json};
use axum::{
    Json, Router,
    extract::State,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use std::{
    collections::BTreeMap,
    fmt,
    future::{Future, IntoFuture},
    time::Duration,
};
use tokio::net::TcpListener;

/// Reports the same bootstrap failure to every queue job until the caller stops it.
///
/// This minimal transport works before application registration and core-worker startup.
/// It uses the raw worker authorization value and the upstream webhook envelope, including
/// its form content type for a JSON body containing a JSON-encoded error string.
pub struct QueueErrorReporter {
    client: reqwest::Client,
    get_url: String,
    post_url: String,
    api_key: String,
}
impl fmt::Debug for QueueErrorReporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueErrorReporter").finish_non_exhaustive()
    }
}
impl QueueErrorReporter {
    /// Captures the job/output webhooks and raw worker API key without network operations.
    ///
    /// # Errors
    /// Returns missing-webhook or HTTP client configuration errors.
    pub fn from_env() -> Result<Self> {
        Self::from_values(
            &[
                "RUNPOD_WEBHOOK_GET_JOB",
                "RUNPOD_WEBHOOK_POST_OUTPUT",
                "RUNPOD_AI_API_KEY",
            ]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
            .collect(),
        )
    }

    /// Uses explicit webhook settings; the API key may be absent or empty.
    ///
    /// # Errors
    /// Returns an error if either webhook is absent/empty, or client configuration fails.
    /// URL/header failures are retried by [`Self::run`], matching the source loop.
    pub fn from_values(values: &BTreeMap<String, String>) -> Result<Self> {
        let get_url = values
            .get("RUNPOD_WEBHOOK_GET_JOB")
            .cloned()
            .unwrap_or_default();
        let post_url = values
            .get("RUNPOD_WEBHOOK_POST_OUTPUT")
            .cloned()
            .unwrap_or_default();
        if get_url.is_empty() || post_url.is_empty() {
            return Err(Error::InvalidInput(
                "no job webhook env; cannot report bootstrap error".into(),
            ));
        }
        Ok(Self {
            client: reqwest::Client::builder().build()?,
            get_url,
            post_url,
            api_key: values.get("RUNPOD_AI_API_KEY").cloned().unwrap_or_default(),
        })
    }

    /// Continues reporting jobs across transport/protocol errors until shutdown resolves.
    ///
    /// GETs have a ninety-second timeout and reports have thirty seconds. Non-200 successful
    /// polls delay two seconds; failures delay five seconds. Empty/missing job IDs and successful
    /// reports immediately poll again. Shutdown cancels pending I/O or backoff without posting
    /// a synthetic completion. A report already accepted remotely cannot be recalled.
    ///
    /// # Errors
    /// Returns payload serialization failures. Per-request failures are logged and retried.
    pub async fn run<S>(&self, error: &PhaseError, shutdown: S) -> Result<()>
    where
        S: Future<Output = ()> + Send,
    {
        let payload =
            serde_json::to_vec(&json!({"error":serde_json::to_string(&error.payload())?}))?;
        let reporting = async {
            loop {
                let delay = match self.cycle(&payload).await {
                    Ok(delay) => delay,
                    Err(error) => {
                        tracing::warn!(%error,"bootstrap error reporting failed; retrying");
                        Duration::from_secs(5)
                    }
                };
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }
        };
        tokio::select! {()=shutdown=>{},()=reporting=>{}}
        Ok(())
    }

    async fn cycle(&self, payload: &[u8]) -> Result<Duration> {
        let response = self
            .client
            .get(&self.get_url)
            .header("Authorization", &self.api_key)
            .timeout(Duration::from_secs(90))
            .send()
            .await?
            .error_for_status()?;
        if response.status() != StatusCode::OK {
            return Ok(Duration::from_secs(2));
        }
        let bytes = response.bytes().await?;
        let job: Value = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes)?
        };
        let job = job
            .as_object()
            .ok_or_else(|| Error::InvalidResponse("queue job must be an object".into()))?;
        let Some(id) = job.get("id").filter(|id| truthy(id)) else {
            return Ok(Duration::ZERO);
        };
        let id = id
            .as_str()
            .ok_or_else(|| Error::InvalidResponse("queue job id must be a string".into()))?;
        self.client
            .post(self.post_url.replace("$ID", id))
            .header("Authorization", &self.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .timeout(Duration::from_secs(30))
            .body(payload.to_vec())
            .send()
            .await?
            .error_for_status()?;
        tracing::info!(job_id = id, "reported bootstrap error");
        Ok(Duration::ZERO)
    }
}

/// Serves a bootstrap error with HTTP 500 for GET, POST, PUT, DELETE and PATCH on every path.
///
/// No authentication or body parsing is performed, matching the API bootstrap responder.
/// Other methods receive 501; their native JSON diagnostic replaces Python's HTML page.
/// Shutdown closes accepted connections, including incomplete request bodies. Dropping the
/// serving future also drops the caller's shutdown future and closes its accepted connections.
///
/// # Errors
/// Returns listener or HTTP serving failures.
pub async fn serve_api_error<S>(
    listener: TcpListener,
    error: &PhaseError,
    shutdown: S,
) -> Result<()>
where
    S: Future<Output = ()> + Send,
{
    let router = Router::new()
        .fallback(api_response)
        .with_state(error.payload());
    let (closing, closed) = tokio::sync::watch::channel(());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = axum::serve(
        super::super::transport::ClosingListener::new(listener, closed),
        router,
    )
    .with_graceful_shutdown(async {
        let _ = stopped.await;
    })
    .into_future();
    tokio::pin!(server);
    // Keep caller-owned shutdown state here: Axum spawns its signal future separately.
    tokio::select! {
        result=&mut server=>result?,
        ()=shutdown=>{
            drop(closing);
            let _=stop.send(());
            server.await?;
        }
    }
    Ok(())
}

async fn api_response(State(error): State<Value>, method: Method) -> Response {
    if [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
    ]
    .contains(&method)
    {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
    } else {
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({"error":"unsupported method"})),
        )
            .into_response()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorRoute {
    Queue,
    Api,
    Task,
}
impl ErrorRoute {
    fn from_kind(kind: Option<&str>) -> Self {
        match kind {
            Some("api") => Self::Api,
            Some("task") => Self::Task,
            _ => Self::Queue,
        }
    }
}

/// Starts the environment-selected bootstrap error responder without loading application code.
///
/// `RUNPOD_RUNTIME_KIND=task` uses token-protected task readiness/error responses and deletion
/// policy. `api` listens on `PORT` (80 by default); every other value uses queue webhooks.
/// Runs until Ctrl+C, or exits the task process after acknowledged pod deletion. For embedding,
/// use [`QueueErrorReporter::run`], [`serve_api_error`], or a task runner with a bootstrap error.
///
/// # Errors
/// Returns missing webhook/token, invalid port/policy, client, bind or serving failures.
pub async fn report_error(error: &PhaseError) -> Result<()> {
    tracing::error!(%error,"worker bootstrap failed");
    match ErrorRoute::from_kind(std::env::var("RUNPOD_RUNTIME_KIND").ok().as_deref()) {
        ErrorRoute::Task => super::super::task_runner::start_failed(error).await,
        ErrorRoute::Api => {
            let port = std::env::var("PORT")
                .unwrap_or_else(|_| "80".into())
                .trim()
                .parse::<u16>()
                .map_err(|_| Error::InvalidInput("PORT must be a port number".into()))?;
            serve_api_error(
                TcpListener::bind(("0.0.0.0", port)).await?,
                error,
                shutdown(),
            )
            .await
        }
        ErrorRoute::Queue => QueueErrorReporter::from_env()?.run(error, shutdown()).await,
    }
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests;

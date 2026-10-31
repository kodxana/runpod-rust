//! Development HTTP worker and the endpoint-specific realtime route.
mod validation;

use super::{
    Handler, HandlerKind, JobsProgress, PrestartHooks, WorkerConfig, WorkerJob, WorkerTransport,
    build_prestart_failed_payload,
    capture::{Capture, CaptureMode},
    handler::{handler_error, normalize_output},
    run_job_generator_with_capture,
};
use crate::{Error, Result, Value, client::truthy, json};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use futures_util::StreamExt;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::net::TcpListener;

/// Native development API with deferred `/run` jobs, synchronous results, and realtime calls.
///
/// Trackers are owned per instance; use [`Self::with_progress`] to explicitly share queued jobs.
/// Construction starts no tasks. [`Self::serve`] starts heartbeats and gates requests on prestart.
/// Simulation `/stream` collects a complete JSON response; it does not use SSE or chunked output.
#[derive(Debug)]
pub struct WorkerApi {
    state: ApiState,
    transport: WorkerTransport,
    hooks: PrestartHooks,
}

#[derive(Debug)]
struct ApiState {
    handler: Handler,
    progress: JobsProgress,
    capture_mode: CaptureMode,
    endpoint_id: Option<String>,
    webhook: reqwest::Client,
}

impl WorkerApi {
    /// Create a development API using an independent job tracker.
    ///
    /// # Errors
    /// Returns invalid worker configuration or HTTP client initialization errors.
    pub fn new(config: WorkerConfig, handler: Handler) -> Result<Self> {
        Self::with_progress(config, handler, JobsProgress::new())
    }

    /// Create an API sharing the supplied tracker with other instances or observers.
    ///
    /// # Errors
    /// Returns invalid worker configuration or HTTP client initialization errors.
    pub fn with_progress(
        config: WorkerConfig,
        handler: Handler,
        progress: JobsProgress,
    ) -> Result<Self> {
        let capture_mode = config.capture_mode;
        Ok(Self {
            transport: WorkerTransport::new(config)?,
            hooks: PrestartHooks::new(),
            state: ApiState {
                handler,
                progress,
                capture_mode,
                endpoint_id: None,
                webhook: webhook_client()?,
            },
        })
    }

    /// Enable `/{endpoint_id}/realtime`; an empty identifier disables this route.
    #[must_use]
    pub fn with_endpoint_id(mut self, id: impl Into<String>) -> Self {
        let id = id.into();
        self.state.endpoint_id = (!id.is_empty()).then_some(id);
        self
    }

    /// Shared observation of queued jobs and active realtime calls.
    #[must_use]
    pub fn progress(&self) -> JobsProgress {
        self.state.progress.clone()
    }

    /// Register startup hooks before handing ownership to the server.
    pub fn prestart_hooks_mut(&mut self) -> &mut PrestartHooks {
        &mut self.hooks
    }

    /// Bind and run a hosted development server until a process shutdown signal.
    /// Prestart failures force-exit with code 1, matching the process entrypoint.
    /// `concurrency` selects processes in Python; this native server supports one process.
    /// Its Tokio runtime still handles concurrent HTTP requests.
    ///
    /// # Errors
    /// Returns unsupported process counts, invalid configuration, bind, or signal errors.
    pub async fn start(self, host: &str, port: u16, concurrency: usize) -> Result<()> {
        if concurrency > 1 {
            let message = if self.hooks.has_prestart_hooks() {
                "Prestart hooks require rp_api_concurrency=1 in hosted API mode."
            } else {
                "The development API supports one server process; use Tokio for request concurrency."
            };
            return Err(Error::InvalidInput(message.into()));
        }
        let listener = TcpListener::bind((host, port)).await?;
        #[cfg(unix)]
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let shutdown = async move {
            #[cfg(unix)]
            tokio::select! {
                _ = terminate.recv() => {},
                result = tokio::signal::ctrl_c() => {if let Err(error) = result {tracing::warn!(%error,"Ctrl-C listener failed");}}
            }
            #[cfg(not(unix))]
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::warn!(%error,"Ctrl-C listener failed");
            }
        };
        super::worker::finish_process_start(self.serve(listener, shutdown).await)
    }

    /// Bind a realtime process on all interfaces; registered prestart hooks are rejected.
    ///
    /// # Errors
    /// Returns unsupported hooks/process counts, bind errors, or signal errors.
    pub async fn start_realtime(self, port: u16, concurrency: usize) -> Result<()> {
        if self.hooks.has_prestart_hooks() {
            return Err(Error::InvalidInput(
                "Prestart hooks are not supported in realtime mode.".into(),
            ));
        }
        self.start("0.0.0.0", port, concurrency).await
    }

    /// Serve an already-bound listener until shutdown, then drain active HTTP requests.
    /// Heartbeats include queued and realtime jobs, and run during prestart.
    /// A failed startup returns before accepting any HTTP requests.
    /// Pending simulation jobs remain in the shared tracker after shutdown.
    /// Callback tasks are best effort and may outlive the listener, within the caller's runtime.
    ///
    /// # Errors
    /// Returns named startup failures or listener errors. This embedding API never exits the process.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        let Self {
            state,
            transport,
            hooks,
        } = self;
        let progress = state.progress.clone();
        let heartbeat = async {
            loop {
                if let Err(error) = transport.ping(&progress).await {
                    tracing::warn!(%error, "worker heartbeat failed");
                }
                tokio::time::sleep(transport.config().ping_interval).await;
            }
        };
        let server = async {
            let config = transport.config();
            let captured = Capture::new();
            let invocation = hooks.run_prestart_phase(config.prestart_timeout);
            let startup = async {
                if config.capture_mode == CaptureMode::Off {
                    invocation.await
                } else {
                    captured.scope(invocation).await
                }
            };
            let mut shutdown = Box::pin(shutdown);
            let result = tokio::select! {
                result = startup => result,
                () = &mut shutdown => return Ok(()),
            };
            if let Err(error) = result {
                let report =
                    build_prestart_failed_payload(&error, &captured.text(), &config.worker_id);
                tracing::error!("prestart_failed | {report}");
                return Err(Error::Prestart(error));
            }
            let router = Router::new().fallback(dispatch).with_state(Arc::new(state));
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown)
                .await?;
            Ok(())
        };
        tokio::select! {result = server => result, () = heartbeat => unreachable!("heartbeat loops until dropped")}
    }
}

#[derive(Clone, Copy)]
enum Route<'a> {
    Run,
    RunSync,
    Stream(&'a str),
    Status(&'a str),
    Realtime,
    Docs,
    DocsRedirect,
    Schema,
    Redoc,
    OAuth,
}

impl ApiState {
    fn route<'a>(&self, path: &'a str) -> Option<Route<'a>> {
        match path {
            "/" => Some(Route::Docs),
            "/docs" => Some(Route::DocsRedirect),
            "/openapi.json" => Some(Route::Schema),
            "/redoc" => Some(Route::Redoc),
            "/docs/oauth2-redirect" => Some(Route::OAuth),
            "/run" => Some(Route::Run),
            "/runsync" => Some(Route::RunSync),
            _ => {
                if let Some(id) = &self.endpoint_id
                    && path == format!("/{id}/realtime")
                {
                    return Some(Route::Realtime);
                }
                if let Some(id) = path
                    .strip_prefix("/status/")
                    .filter(|s| !s.is_empty() && !s.contains('/'))
                {
                    return Some(Route::Status(id));
                }
                path.strip_prefix("/stream/")
                    .filter(|s| !s.is_empty() && !s.contains('/'))
                    .map(Route::Stream)
            }
        }
    }

    async fn single(&self, job: WorkerJob) -> Value {
        let (mut stream, capture) = self.handler.clone().captured(job, self.capture_mode);
        match stream.next().await {
            Some(Ok(value)) => normalize_output(value),
            Some(Err(error)) => handler_error(&error, &capture.text()),
            None => normalize_output(Value::Null),
        }
    }

    async fn collected(&self, job: WorkerJob, wrapped: bool) -> std::result::Result<Value, ()> {
        let mut stream =
            run_job_generator_with_capture(job, self.capture_mode, self.handler.clone());
        let mut values = Vec::new();
        while let Some(item) = stream.next().await {
            let Some(output) = item.get("output") else {
                return Err(());
            };
            values.push(if wrapped {
                json!({"output":output})
            } else {
                output.clone()
            });
        }
        Ok(Value::Array(values))
    }

    async fn output(&self, job: WorkerJob) -> std::result::Result<Value, ()> {
        if self.handler.kind() == HandlerKind::Streaming {
            Ok(json!({"output":self.collected(job, false).await?}))
        } else {
            Ok(self.single(job).await)
        }
    }

    fn callback(&self, url: Option<String>, payload: &Value) {
        let Some(url) = url.filter(|url| !url.is_empty()) else {
            return;
        };
        let payload = payload.clone();
        let client = self.webhook.clone();
        tokio::spawn(async move {
            deliver_webhook(&client, &url, &payload).await;
        });
    }

    fn completed(&self, id: &str, mut result: Value, webhook: Option<String>) -> Response {
        if let Some(error) = result.get("error").filter(|value| truthy(value)) {
            return if error.is_string() {
                Json(json!({"id":id,"status":"FAILED","error":error})).into_response()
            } else {
                internal_error()
            };
        }
        // Python schedules its callback before indexing output, even when that lookup fails.
        self.callback(webhook, &result);
        let Some(output) = result
            .as_object_mut()
            .and_then(|object| object.remove("output"))
        else {
            return internal_error();
        };
        let mut response = json!({"id":id,"status":"COMPLETED"});
        if !output.is_null() {
            response["output"] = output;
        }
        Json(response).into_response()
    }
}

fn simulation_job(id: String, input: Value) -> WorkerJob {
    let mut job = WorkerJob::new(id, input);
    // TestJob.__dict__ includes this null, unlike the production/realtime Job object.
    job.metadata.insert("webhook".into(), Value::Null);
    job
}

async fn dispatch(State(state): State<Arc<ApiState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = percent_encoding::percent_decode_str(parts.uri.path()).decode_utf8_lossy();
    let Some(route) = state.route(&path) else {
        let alternate = if path.ends_with('/') {
            path.trim_end_matches('/').to_owned()
        } else {
            format!("{path}/")
        };
        if state.route(&alternate).is_some() {
            let suffix = parts
                .uri
                .query()
                .map(|q| format!("?{q}"))
                .unwrap_or_default();
            let encoded_path = parts.uri.path();
            let encoded = if path.ends_with('/') {
                encoded_path.trim_end_matches('/').to_owned()
            } else {
                format!("{encoded_path}/")
            };
            let host = parts
                .headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok());
            let target = if let Some(host) = host {
                format!(
                    "{}://{host}{encoded}{suffix}",
                    parts.uri.scheme_str().unwrap_or("http")
                )
            } else {
                format!("{encoded}{suffix}")
            };
            return Redirect::temporary(&target).into_response();
        }
        return (StatusCode::NOT_FOUND, Json(json!({"detail":"Not Found"}))).into_response();
    };
    let read_route = matches!(
        route,
        Route::Docs | Route::DocsRedirect | Route::Schema | Route::Redoc | Route::OAuth
    );
    let head_allowed = read_route && !matches!(route, Route::DocsRedirect);
    if if read_route {
        parts.method != Method::GET && !(head_allowed && parts.method == Method::HEAD)
    } else {
        parts.method != Method::POST
    } {
        let allow = if read_route {
            if head_allowed { "GET, HEAD" } else { "GET" }
        } else {
            "POST"
        };
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, allow)],
            Json(json!({"detail":"Method Not Allowed"})),
        )
            .into_response();
    }
    match route {
        Route::DocsRedirect => Redirect::temporary("/").into_response(),
        Route::Docs => Html(DOCS).into_response(),
        Route::Redoc => Html(REDOC).into_response(),
        Route::OAuth => Html(OAUTH).into_response(),
        Route::Schema => Json(openapi(state.endpoint_id.as_deref())).into_response(),
        Route::Run | Route::RunSync | Route::Realtime => {
            let bytes = match to_bytes(body, usize::MAX).await {
                Ok(bytes) => bytes,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"detail":"There was an error parsing the body"})),
                    )
                        .into_response();
                }
            };
            let realtime = matches!(route, Route::Realtime);
            let parsed = match validation::request(&bytes, &parts.headers, realtime) {
                Ok(value) => value,
                Err(details) => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(json!({"detail":details})),
                    )
                        .into_response();
                }
            };
            if realtime {
                let job = WorkerJob::new(parsed["id"].as_str().unwrap(), parsed["input"].clone());
                let id = job.id.clone();
                state.progress.add(WorkerJob::new(&id, Value::Null));
                let result = state.output(job).await;
                state.progress.remove(&id);
                return match result {
                    Ok(value) => Json(value).into_response(),
                    Err(()) => internal_error(),
                };
            }
            let id = match random_id() {
                Ok(id) => id,
                Err(_) => return internal_error(),
            };
            let webhook = parsed["webhook"].as_str().map(str::to_owned);
            if matches!(route, Route::Run) {
                let mut job = WorkerJob::new(&id, parsed["input"].clone());
                job.webhook = webhook;
                state.progress.add(job);
                return Json(json!({"id":id,"status":"IN_PROGRESS"})).into_response();
            }
            match state
                .output(simulation_job(id.clone(), parsed["input"].clone()))
                .await
            {
                Ok(output) => state.completed(&id, output, webhook),
                Err(()) => internal_error(),
            }
        }
        Route::Status(id) | Route::Stream(id) => {
            let Some(stashed) = state.progress.get(id) else {
                return Json(json!({"id":id,"status":"FAILED","error":"Job ID not found"}))
                    .into_response();
            };
            let job = simulation_job(stashed.id.clone(), stashed.input);
            if matches!(route, Route::Stream(_)) {
                if state.handler.kind() != HandlerKind::Streaming {
                    return Json(json!({"id":id,"status":"FAILED","error":"Stream not supported, handler must be a generator."})).into_response();
                }
                let values = match state.collected(job, true).await {
                    Ok(values) => values,
                    Err(()) => return internal_error(),
                };
                state.progress.remove(id);
                state.callback(stashed.webhook, &values);
                Json(json!({"id":id,"status":"COMPLETED","stream":values})).into_response()
            } else {
                let output = match state.output(job).await {
                    Ok(output) => output,
                    Err(()) => return internal_error(),
                };
                state.progress.remove(id);
                state.completed(id, output, stashed.webhook)
            }
        }
    }
}

fn internal_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
}

/// Post JSON to a development-worker callback, returning whether delivery succeeded.
/// No worker credentials are attached. Redirects are followed with a ten-second overall
/// request deadline. Transport and HTTP failures are logged and return `false`.
pub async fn send_webhook(url: &str, payload: &Value) -> bool {
    match webhook_client() {
        Ok(client) => deliver_webhook(&client, url, payload).await,
        Err(error) => {
            tracing::warn!(%error, "worker webhook client failed");
            false
        }
    }
}

fn webhook_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(30))
        .retry(reqwest::retry::never())
        .build()?)
}

async fn deliver_webhook(client: &reqwest::Client, url: &str, payload: &Value) -> bool {
    let result = async {
        client
            .post(url)
            .json(payload)
            .send()
            .await?
            .error_for_status()
    }
    .await;
    match result {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!(%error, %url, "worker webhook failed");
            false
        }
    }
}

fn random_id() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    Ok(format!(
        "test-{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn openapi(endpoint: Option<&str>) -> Value {
    let mut schema: Value = serde_json::from_str(include_str!("openapi.json"))
        .expect("embedded OpenAPI schema is valid JSON");
    schema["info"]["version"] = json!(env!("CARGO_PKG_VERSION"));
    let paths = schema["paths"].as_object_mut().unwrap();
    let mut realtime = paths.remove("/endpoint/realtime").unwrap();
    if let Some(id) = endpoint {
        let suffix: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        realtime["post"]["operationId"] = json!(format!("_realtime_{suffix}_realtime_post"));
        paths.insert(format!("/{id}/realtime"), realtime);
    } else {
        schema["components"]["schemas"]
            .as_object_mut()
            .unwrap()
            .remove("Job");
    }
    schema
}

const DOCS: &str = r#"<!doctype html><html><head><title>Runpod | Development Worker API - Swagger UI</title><link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css"></head><body><div id="swagger-ui"></div><script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script><script>SwaggerUIBundle({url:'/openapi.json',dom_id:'#swagger-ui',deepLinking:true,oauth2RedirectUrl:window.location.origin+'/docs/oauth2-redirect',presets:[SwaggerUIBundle.presets.apis,SwaggerUIBundle.SwaggerUIStandalonePreset]})</script></body></html>"#;
const REDOC: &str = r#"<!doctype html><html><head><title>Runpod | Development Worker API - ReDoc</title></head><body><redoc spec-url="/openapi.json"></redoc><script src="https://cdn.jsdelivr.net/npm/redoc@2/bundles/redoc.standalone.js"></script></body></html>"#;
const OAUTH: &str = include_str!("oauth.html");

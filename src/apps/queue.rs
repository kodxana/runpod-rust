use super::{AppError, targets::unwrap_job_output};
use crate::{
    Client, Error, Result, Value,
    client::{segment, truthy},
    json,
};
use futures_util::stream::BoxStream;
use reqwest::{
    Method,
    header::{HeaderMap, HeaderValue},
};
use std::time::Duration;

/// Default application invocation timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// Terminal statuses for Apps queue jobs, including cancellation.
pub const FINAL_STATUSES: [&str; 4] = ["COMPLETED", "FAILED", "CANCELLED", "TIMED_OUT"];
/// Sentinel endpoint used to resolve application resources on the server.
pub const SENTINEL_ID: &str = "flash";
const RETRYABLE_STATUSES: [u16; 8] = [429, 500, 502, 503, 504, 520, 522, 524];

/// Stateless routing identity for an application resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAddress {
    /// Application name.
    pub app: String,
    /// Deployment environment.
    pub environment: String,
    /// Resource name or backend ID.
    pub resource: String,
}
impl ResourceAddress {
    /// Creates an application/environment/resource routing identity.
    pub fn new(
        app: impl Into<String>,
        environment: impl Into<String>,
        resource: impl Into<String>,
    ) -> Self {
        Self {
            app: app.into(),
            environment: environment.into(),
            resource: resource.into(),
        }
    }
}

/// Status callback invoked for admission and every observed job response.
pub type StatusObserver<'a> = dyn FnMut(&Value) -> Result<()> + Send + 'a;

/// Queue data-plane client with endpoint-specific headers and load-balancer routing.
#[derive(Debug, Clone)]
pub struct QueueClient {
    client: Client,
    endpoint_id: String,
    base_url: String,
    lb_base_url: String,
    headers: HeaderMap,
    address: Option<ResourceAddress>,
}
impl QueueClient {
    pub(super) fn client(&self) -> &Client {
        &self.client
    }
    /// Uses a client connection pool and an explicit endpoint identifier.
    ///
    /// # Errors
    /// Returns malformed service-address errors.
    pub fn new(client: Client, endpoint_id: impl Into<String>) -> Result<Self> {
        let endpoint_id = endpoint_id.into();
        if endpoint_id.is_empty()
            || !endpoint_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(Error::InvalidInput(
                "endpoint ID must contain only ASCII letters, digits, '-' or '_'".into(),
            ));
        }
        let base_url = format!(
            "{}/{}",
            client.config.endpoint_base_url.trim_end_matches('/'),
            segment(&endpoint_id)
        );
        let url = reqwest::Url::parse(&client.config.endpoint_base_url)
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let domain = url
            .host_str()
            .ok_or_else(|| Error::InvalidInput("endpoint URL is missing a host".into()))?;
        let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        let lb_base_url = format!("https://{endpoint_id}.{domain}{port}");
        Ok(Self {
            client,
            endpoint_id,
            base_url,
            lb_base_url,
            headers: HeaderMap::new(),
            address: None,
        })
    }
    pub(super) fn set_address(&mut self, address: ResourceAddress) -> Result<()> {
        for (name, value) in [
            ("X-Flash-App", &address.app),
            ("X-Flash-Environment", &address.environment),
            ("X-Flash-Endpoint", &address.resource),
        ] {
            self.headers.insert(
                name,
                HeaderValue::from_str(value)
                    .map_err(|_| Error::InvalidInput(format!("invalid {name} header")))?,
            );
        }
        self.address = Some(address);
        Ok(())
    }
    /// Returns the bound endpoint ID.
    pub fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }
    /// Returns the queue route root.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Overrides the load-balancer service root, useful for private gateways and local workers.
    ///
    /// # Errors
    /// Rejects non-HTTP(S), credential-bearing, queried, or fragmented service URLs.
    pub fn set_load_balancer_url(&mut self, url: impl Into<String>) -> Result<()> {
        let url = url.into();
        let parsed = reqwest::Url::parse(&url).map_err(|e| Error::InvalidInput(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(Error::InvalidInput(
                "load-balancer URL must be HTTP(S) without credentials, query, or fragment".into(),
            ));
        }
        self.lb_base_url = url.trim_end_matches('/').into();
        Ok(())
    }

    fn not_found(&self) -> Error {
        AppError::EndpointNotFound {
            app_name: self
                .address
                .as_ref()
                .map(|a| a.app.clone())
                .unwrap_or_default(),
            resource_name: self
                .address
                .as_ref()
                .map(|a| a.resource.clone())
                .unwrap_or_default(),
        }
        .into()
    }

    async fn request_json(
        &self,
        method: Method,
        url: &str,
        payload: Option<&Value>,
        timeout: Duration,
        retry_not_found: bool,
    ) -> Result<Value> {
        let transient = matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
        let attempts = if retry_not_found {
            7
        } else if transient {
            4
        } else {
            1
        };
        for attempt in 0..attempts {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * (1 << (attempt - 1)))).await;
            }
            let result = self
                .request_once(method.clone(), url, payload, timeout)
                .await;
            let retry = match &result {
                Err(Error::App(AppError::EndpointNotFound { .. })) => retry_not_found,
                Err(Error::Query {
                    status_code: Some(status),
                    ..
                }) => transient && RETRYABLE_STATUSES.contains(status),
                Err(Error::Http(e)) => {
                    transient && (e.is_connect() || e.is_timeout() || e.is_request())
                }
                _ => false,
            };
            if retry && attempt + 1 < attempts {
                continue;
            }
            return result;
        }
        unreachable!("at least one queue request attempt always returns")
    }

    async fn request_once(
        &self,
        method: Method,
        url: &str,
        payload: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let mut request = self
            .client
            .http
            .request(method.clone(), url)
            .headers(self.headers.clone());
        if !timeout.is_zero() {
            request = request.timeout(timeout);
        }
        if let Some(body) = payload.filter(|v| !v.is_null()) {
            request = request.json(body);
        }
        let response = request.send().await?;
        if response.status().as_u16() == 404 {
            return Err(self.not_found());
        }
        let status = response.status().as_u16();
        if status >= 400 {
            let text = response.text().await?;
            return Err(Error::Query {
                message: if text.is_empty() {
                    format!("request failed with status {status}")
                } else {
                    text
                },
                query: format!("{method} {url}"),
                status_code: Some(status),
                errors: Box::new(json!([])),
            });
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if content_type != "application/json" && !content_type.ends_with("+json") {
            return Err(Error::InvalidResponse(format!(
                "expected JSON content type, received {content_type}"
            )));
        }
        Ok(response.json().await?)
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        payload: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let retry_not_found = ["status/", "stream/", "cancel/", "retry/"]
            .iter()
            .any(|prefix| path.starts_with(prefix));
        self.request_json(
            method,
            &format!("{}/{path}", self.base_url),
            payload,
            timeout,
            retry_not_found,
        )
        .await
    }

    /// Submits a queue job once and returns its admission response.
    ///
    /// # Errors
    /// Returns routing, HTTP, or malformed response errors without replaying an ambiguous submission.
    pub async fn run(&self, payload: &Value, timeout: Duration) -> Result<Value> {
        self.call(Method::POST, "run", Some(payload), timeout).await
    }

    /// Uses runsync first and polls the same job when the synchronous window expires.
    ///
    /// # Errors
    /// Returns request, observation, missing-job-ID, or deadline errors.
    pub async fn invoke(
        &self,
        payload: &Value,
        timeout: Duration,
        on_status: Option<&mut StatusObserver<'_>>,
    ) -> Result<Value> {
        let start = tokio::time::Instant::now();
        let wait_ms = if on_status.is_some() {
            1000
        } else {
            timeout.as_millis().saturating_sub(1000).clamp(1000, 90000)
        };
        let data = self
            .call(
                Method::POST,
                &format!("runsync?wait={wait_ms}"),
                Some(payload),
                timeout,
            )
            .await?;
        self.wait(data, timeout.saturating_sub(start.elapsed()), on_status)
            .await
    }

    /// Fetches job status, tolerating temporary job propagation failures.
    ///
    /// # Errors
    /// Returns request or decoding errors after bounded read retries.
    pub async fn status(&self, id: &str) -> Result<Value> {
        self.call(
            Method::GET,
            &format!("status/{}", segment(id)),
            None,
            DEFAULT_TIMEOUT,
        )
        .await
    }
    /// Cancels a job, retrying only explicit not-found propagation responses.
    ///
    /// # Errors
    /// Returns request errors; ambiguous mutation failures are not retried.
    pub async fn cancel(&self, id: &str) -> Result<Value> {
        self.call(
            Method::POST,
            &format!("cancel/{}", segment(id)),
            None,
            DEFAULT_TIMEOUT,
        )
        .await
    }
    /// Retries a failed job, retrying only explicit not-found propagation responses.
    ///
    /// # Errors
    /// Returns request errors; ambiguous mutation failures are not retried.
    pub async fn retry(&self, id: &str) -> Result<Value> {
        self.call(
            Method::POST,
            &format!("retry/{}", segment(id)),
            None,
            DEFAULT_TIMEOUT,
        )
        .await
    }

    /// Polls to a terminal state and observes admission plus every subsequent response.
    ///
    /// # Errors
    /// Returns request, observation, missing-job-ID, or deadline errors.
    pub async fn wait(
        &self,
        mut data: Value,
        timeout: Duration,
        mut on_status: Option<&mut StatusObserver<'_>>,
    ) -> Result<Value> {
        if let Some(observer) = &mut on_status {
            observer(&data)?;
        }
        let start = tokio::time::Instant::now();
        let mut interval = Duration::from_millis(500);
        let max_interval = Duration::from_secs(if on_status.is_some() { 1 } else { 5 });
        while !is_terminal(&data) {
            let status = data
                .get("status")
                .map(super::super::control::python_string)
                .unwrap_or_else(|| "UNKNOWN".into());
            let id = data
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    AppError::RemoteExecution(format!(
                        "non-terminal job response is missing a job id (status: {status})"
                    ))
                })?;
            if start.elapsed() >= timeout {
                return Err(Error::DeadlineExceeded(format!(
                    "job {id} did not complete within {}s (last status: {status})",
                    timeout.as_secs_f64()
                )));
            }
            tokio::time::sleep(interval).await;
            interval = interval.mul_f64(1.5).min(max_interval);
            data = self
                .call(
                    Method::GET,
                    &format!("status/{}", segment(id)),
                    None,
                    Duration::from_secs(30),
                )
                .await?;
            if let Some(observer) = &mut on_status {
                observer(&data)?;
            }
        }
        Ok(data)
    }

    /// Drains stream pages until the first terminal page, retaining chunks on that page.
    pub fn stream(&self, id: &str, timeout: Duration) -> BoxStream<'static, Result<Value>> {
        let client = self.clone();
        let id = id.to_owned();
        Box::pin(async_stream::try_stream! {
            let start=tokio::time::Instant::now();
            loop {
                if start.elapsed()>=timeout {Err(Error::DeadlineExceeded(format!("job {id} stream did not complete within {}s",timeout.as_secs_f64())))?;}
                let data=client.call(Method::GET,&format!("stream/{}",segment(&id)),None,DEFAULT_TIMEOUT).await?;
                if data["status"]=="FAILED" || data.get("error").is_some_and(truthy) {unwrap_job_output(&data)?;}
                if let Some(chunks)=data.get("stream").filter(|v|truthy(v)) {
                    for chunk in chunks.as_array().ok_or_else(||Error::InvalidResponse("stream must contain an array".into()))? {yield chunk.get("output").cloned().unwrap_or(Value::Null);}
                }
                if is_terminal(&data) {break;}
            }
        })
    }

    /// Calls an HTTP route on the endpoint load balancer, returning JSON or plain text.
    ///
    /// # Errors
    /// Returns invalid paths, routing errors, or HTTP failures; route calls are not replayed.
    pub async fn http(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        if !path.starts_with('/') {
            return Err(Error::InvalidInput(
                "HTTP route path must start with '/'".into(),
            ));
        }
        let method = Method::from_bytes(method.to_uppercase().as_bytes())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let mut request = self
            .client
            .http
            .request(method, format!("{}{path}", self.lb_base_url))
            .headers(self.headers.clone());
        if !timeout.is_zero() {
            request = request.timeout(timeout);
        }
        if let Some(body) = body.filter(|v| !v.is_null()) {
            request = request.json(body);
        }
        let response = request.send().await?;
        if response.status().as_u16() == 404
            && self.address.as_ref().is_some_and(|a| !a.app.is_empty())
        {
            return Err(self.not_found());
        }
        let text = response.error_for_status()?.text().await?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub(super) async fn sync_source(&self, payload: &Value, timeout: Duration) -> Result<()> {
        self.request_json(
            Method::POST,
            &format!("{}/_runpod/sync", self.lb_base_url),
            Some(payload),
            timeout,
            false,
        )
        .await?;
        Ok(())
    }
}

pub(super) fn is_terminal(data: &Value) -> bool {
    data.get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| FINAL_STATUSES.contains(&s))
}

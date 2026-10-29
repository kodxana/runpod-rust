//! Authenticated REST and GraphQL transports with explicit configuration.
use crate::{Error, Result, Value, json};
use reqwest::{
    Method,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use std::{collections::HashSet, fmt, time::Duration};

/// Connection settings and credentials for a client.
#[derive(Clone)]
pub struct ClientConfig {
    /// API token; Debug deliberately redacts this field.
    pub api_key: String,
    /// Control-plane root, normally `https://api.runpod.io`.
    pub api_base_url: String,
    /// Job API root, normally `https://api.runpod.ai/v2`.
    pub endpoint_base_url: String,
    /// Default control-plane request timeout.
    pub timeout: Duration,
    /// Interval between job status and stream requests.
    pub poll_interval: Duration,
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("api_key", &"[REDACTED]")
            .field("api_base_url", &self.api_base_url)
            .field("endpoint_base_url", &self.endpoint_base_url)
            .field("timeout", &self.timeout)
            .field("poll_interval", &self.poll_interval)
            .finish()
    }
}

impl ClientConfig {
    /// Uses an explicit token and conventional Runpod service addresses.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_base_url: "https://api.runpod.io".into(),
            endpoint_base_url: "https://api.runpod.ai/v2".into(),
            timeout: Duration::from_secs(30),
            poll_interval: Duration::from_secs(1),
        }
    }
    /// Resolves credentials and base URL overrides from the environment.
    ///
    /// # Errors
    /// Returns an authentication error when `RUNPOD_API_KEY` is absent or empty.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("RUNPOD_API_KEY")
            .map_err(|_| Error::Authentication("No API key provided".into()))?;
        if key.is_empty() {
            return Err(Error::Authentication("No API key provided".into()));
        }
        let mut config = Self::new(key);
        if let Ok(value) = std::env::var("RUNPOD_API_BASE_URL") {
            config.api_base_url = value;
        }
        if let Ok(value) = std::env::var("RUNPOD_ENDPOINT_BASE_URL") {
            config.endpoint_base_url = value;
        }
        Ok(config)
    }
}

/// A reusable asynchronous connection pool with per-instance authentication.
#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) config: ClientConfig,
    pub(crate) request_id: Option<String>,
}

impl Client {
    /// Creates a client using an explicit API token.
    ///
    /// # Errors
    /// Fails for an empty or invalid token, invalid URL, or TLS initialization failure.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::with_config(ClientConfig::new(api_key))
    }
    /// Creates a client using environment configuration.
    ///
    /// # Errors
    /// Fails when credentials are absent or configuration is invalid.
    pub fn from_env() -> Result<Self> {
        Self::with_config(ClientConfig::from_env()?)
    }
    /// Creates a client with configurable service roots and timeouts.
    ///
    /// # Errors
    /// Fails for an empty or invalid token, invalid URL, or TLS initialization failure.
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        if config.api_key.is_empty() {
            return Err(Error::Authentication("No API key provided".into()));
        }
        for value in [&config.api_base_url, &config.endpoint_base_url] {
            let url = reqwest::Url::parse(value).map_err(|e| Error::InvalidInput(e.to_string()))?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::InvalidInput(
                    "service URL must be HTTP(S) without credentials, query, or fragment".into(),
                ));
            }
        }
        let mut headers = HeaderMap::new();
        let mut token = HeaderValue::from_str(&format!("Bearer {}", config.api_key))
            .map_err(|_| Error::InvalidInput("invalid API key header".into()))?;
        token.set_sensitive(true);
        headers.insert(AUTHORIZATION, token);
        headers.insert("Accept", HeaderValue::from_static("application/json"));
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("runpod-rust/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(config.timeout)
            .build()?;
        Ok(Self {
            http,
            config,
            request_id: None,
        })
    }
    /// Binds this client's credentials and connection pool to an endpoint.
    pub fn endpoint(&self, id: impl Into<String>) -> crate::Endpoint {
        crate::Endpoint::new(id, self.clone())
    }

    /// Sends a REST request without automatically replaying mutations.
    ///
    /// # Errors
    /// Returns authentication, HTTP, API query, or JSON decoding errors.
    pub async fn run_rest_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        params: &[(&str, &str)],
    ) -> Result<Value> {
        self.request(
            &self.config.api_base_url,
            method,
            path,
            body,
            params,
            self.config.timeout,
        )
        .await
    }

    pub(crate) async fn request(
        &self,
        base: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
        params: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Value> {
        let method = Method::from_bytes(method.to_uppercase().as_bytes())
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
        let url = format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut request = self
            .http
            .request(method.clone(), url)
            .query(params)
            .timeout(timeout);
        if let Some(id) = &self.request_id {
            request = request.header("X-Request-ID", id);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        Self::read_response(response, &format!("{method} {path}")).await
    }

    async fn read_response(response: reqwest::Response, query: &str) -> Result<Value> {
        let status = response.status().as_u16();
        if status == 401 {
            return Err(Error::Authentication(
                "Unauthorized request, please check your API key.".into(),
            ));
        }
        let bytes = response.bytes().await?;
        if status >= 400 {
            let text = String::from_utf8_lossy(&bytes);
            let payload: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let message = payload
                .get("detail")
                .filter(|v| truthy(v))
                .or_else(|| payload.get("title").filter(|v| truthy(v)))
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                .unwrap_or_else(|| {
                    if text.is_empty() {
                        format!("Request failed with status {status}")
                    } else {
                        text.into_owned()
                    }
                });
            return Err(Error::Query {
                message,
                query: query.into(),
                status_code: Some(status),
                errors: payload.get("errors").cloned().unwrap_or_else(|| json!([])),
            });
        }
        if status == 204 || bytes.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Collects cursor pages, retrying transport failures up to four times per page.
    ///
    /// # Errors
    /// Fails on HTTP errors, malformed pages, missing/repeated cursors, or exhausted retries.
    pub async fn run_rest_paginated_request(
        &self,
        path: &str,
        collection: &str,
    ) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        let mut cursors = HashSet::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_deref()
                .map(|c| vec![("cursor", c)])
                .unwrap_or_default();
            let mut attempt = 0;
            let page = loop {
                match self.run_rest_request("GET", path, None, &params).await {
                    Err(Error::Http(error))
                        if attempt < 3
                            && (error.is_connect()
                                || error.is_timeout()
                                || error.is_body()
                                || error.is_request()) =>
                    {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                        attempt += 1;
                    }
                    response => break response?,
                }
            };
            let invalid = |message: String| Error::Query {
                message,
                query: format!("GET {path}"),
                status_code: None,
                errors: json!([]),
            };
            let entries = page
                .get(collection)
                .and_then(Value::as_array)
                .filter(|v| v.iter().all(Value::is_object))
                .ok_or_else(|| invalid(format!("invalid {collection} list response")))?;
            let pagination = page
                .get("pagination")
                .and_then(Value::as_object)
                .ok_or_else(|| invalid(format!("invalid {collection} pagination response")))?;
            let has_next = pagination
                .get("hasNextPage")
                .and_then(Value::as_bool)
                .ok_or_else(|| invalid(format!("invalid {collection} pagination response")))?;
            result.extend(entries.iter().cloned());
            if !has_next {
                return Ok(result);
            }
            let next = pagination
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    invalid(format!(
                        "{collection} pagination is missing a valid nextCursor"
                    ))
                })?;
            if !cursors.insert(next.to_owned()) {
                return Err(invalid(format!(
                    "{collection} pagination repeated a cursor"
                )));
            }
            cursor = Some(next.to_owned());
        }
    }

    /// Executes an authenticated GraphQL operation with optional variables.
    ///
    /// # Errors
    /// Returns transport, authentication, JSON, or GraphQL query errors.
    pub async fn run_graphql_query(&self, query: &str, variables: Option<&Value>) -> Result<Value> {
        let mut payload = json!({"query":query});
        if let Some(variables) = variables.filter(|v| truthy(v)) {
            payload["variables"] = variables.clone();
        }
        let response = self
            .run_rest_request("POST", "/graphql", Some(&payload), &[])
            .await?;
        if let Some(errors) = response.get("errors") {
            return Err(Error::Query {
                message: errors
                    .get(0)
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("GraphQL request failed")
                    .into(),
                query: query.into(),
                status_code: None,
                errors: errors.clone(),
            });
        }
        Ok(response)
    }
}

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64() != Some(0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}

pub(crate) fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidResponse(format!("missing string field {key}")))
}

pub(crate) fn segment(value: &str) -> String {
    const PATH: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(value, PATH).to_string()
}

//! Anonymous browser-approval and email-code login without opening a browser or storing credentials.
use super::{AppError, AppsApiClient};
use crate::{Client, Error, Result, Value, client::segment, json};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::{future::Future, sync::LazyLock, time::Duration};
use tokio::time::Instant;

// Match aiohttp's application/json and structured +json media-type check.
static JSON_TYPE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^application/(?:[\w.+-]+?\+)?json").expect("constant media-type expression")
});

/// Native browser approval timing and console address.
#[derive(Debug, Clone)]
pub struct BrowserLoginOptions {
    /// Overall polling budget, checked after each returned status (default ten minutes).
    pub timeout: Duration,
    /// Time between pending responses (default two seconds).
    pub poll_interval: Duration,
    /// Console root, captured from `RUNPOD_CONSOLE_URL` or the standard Runpod console.
    pub console_url: String,
}
impl Default for BrowserLoginOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(600),
            poll_interval: Duration::from_secs(2),
            console_url: std::env::var("RUNPOD_CONSOLE_URL")
                .unwrap_or_else(|_| "https://console.runpod.io".into()),
        }
    }
}
/// Fallible UI callback; called once after creation and before the first poll.
pub type LoginUrlObserver<'a> = dyn FnMut(&str) -> Result<()> + Send + 'a;

/// Formats the upstream approval URL, retaining the console root and request ID verbatim.
#[must_use]
pub fn auth_url(request_id: &str, console_url: &str) -> String {
    format!("{console_url}/flash/login?request={request_id}")
}
fn login_error(message: impl Into<String>) -> Error {
    AppError::Login(message.into()).into()
}

/// Polls anonymous approval requests and returns the granted key without persisting it.
///
/// A returned approval wins even at a zero/expired deadline. A consumed request with a key
/// succeeds; without one it fails. Pending requests sleep for the full polling interval.
/// Dropping the future stops polling. The callback may display/open the URL in caller code.
///
/// Expiration timestamps use RFC 3339 with an explicit offset. Malformed or unsupported
/// timestamps are ignored; elapsed polling uses a monotonic clock after the callback.
/// # Errors
/// Returns request/callback errors, missing initialization, denied/expired/consumed states,
/// or a login timeout. Non-string credential fields are rejected.
pub async fn browser_login(
    api: &AppsApiClient,
    options: &BrowserLoginOptions,
    on_url: Option<&mut LoginUrlObserver<'_>>,
) -> Result<String> {
    let request = api.create_auth_request().await?;
    let id = request
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| login_error("auth request failed to initialize"))?;
    if let Some(on_url) = on_url {
        on_url(&auth_url(id, &options.console_url))?;
    }
    let timeout = parse_expires_at(request.get("expiresAt").and_then(Value::as_str)).map_or(
        options.timeout,
        |expires| {
            (expires - Utc::now())
                .to_std()
                .unwrap_or(Duration::ZERO)
                .min(options.timeout)
        },
    );
    let started = Instant::now();
    loop {
        let state = api.get_auth_request_status(id).await?;
        let status = state.get("status").and_then(Value::as_str).unwrap_or("");
        if matches!(status, "APPROVED" | "CONSUMED")
            && let Some(key) = state.get("apiKey").filter(|v| crate::client::truthy(v))
        {
            return key
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| login_error("login response did not contain a string API key"));
        }
        match status {
            "DENIED" => return Err(login_error("login denied")),
            "EXPIRED" => return Err(login_error("login expired")),
            "CONSUMED" => return Err(login_error("login request was already used")),
            _ => {}
        }
        if started.elapsed() >= timeout {
            return Err(login_error("login timed out"));
        }
        tokio::time::sleep(options.poll_interval).await;
    }
}
fn parse_expires_at(value: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}

/// Isolated anonymous email-code authentication transport.
///
/// This connection never imports existing account keys, cookies, or netrc credentials.
/// Redirects and automatic request retries are disabled, including after ambiguous failures.
#[derive(Debug, Clone)]
pub struct EmailLoginClient {
    client: Client,
}
impl EmailLoginClient {
    /// Constructs a separate anonymous pool for the supplied API root, preserving a path prefix.
    /// # Errors
    /// Returns invalid URL or TLS initialization errors.
    pub fn new(api_base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            client: Client::anonymous(api_base_url)?,
        })
    }
    /// Captures only the API service override; no credentials are loaded.
    /// # Errors
    /// Returns invalid URL or TLS initialization errors.
    pub fn from_env() -> Result<Self> {
        Self::new(
            std::env::var("RUNPOD_API_BASE_URL").unwrap_or_else(|_| "https://api.runpod.io".into()),
        )
    }
    /// Changes the per-request timeout (default thirty seconds).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.client.config.timeout = timeout;
        self
    }

    /// Exchanges an emailed code for a key bound to a fresh proof-of-possession verifier.
    ///
    /// The asynchronous reader is called only after successful initialization. Rejected codes
    /// prompt again using the same attempt and verifier; all other errors stop immediately.
    /// Reader errors and cancellation send no further requests. The returned key is not saved.
    /// # Errors
    /// Returns sanitized login errors, random-source errors, or the caller's reader error.
    pub async fn agent_login<F, Fut>(&self, email: &str, mut read_code: F) -> Result<String>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<String>>,
    {
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(std::io::Error::other)?;
        let verifier = URL_SAFE_NO_PAD.encode(random);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let path = "/v2/signup/authentication-attempts";
        let attempt = self
            .post_attempt(
                path,
                &json!({"email":email,"keyName":"runpod-cli-agent","codeChallenge":challenge}),
                AttemptKind::Create,
            )
            .await
            .map_err(AttemptFailure::into_error)?;
        let id = attempt
            .get("attemptId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| login_error("auth attempt failed to initialize"))?;
        let path = format!("{path}/{}/verify", segment(id));
        let result = loop {
            let code = read_code().await?;
            match self
                .post_attempt(
                    &path,
                    &json!({"code":code,"codeVerifier":verifier}),
                    AttemptKind::Verify,
                )
                .await
            {
                Ok(result) => break result,
                Err(AttemptFailure::Rejected) => continue,
                Err(error) => return Err(error.into_error()),
            }
        };
        result
            .get("credential")
            .and_then(|v| v.get("apiKey"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| login_error("email sign-in response did not contain an API key"))
    }
    async fn post_attempt(
        &self,
        path: &str,
        body: &Value,
        kind: AttemptKind,
    ) -> std::result::Result<Value, AttemptFailure> {
        let url = format!(
            "{}/{}",
            self.client.config.api_base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut request = self
            .client
            .http
            .post(url)
            .json(body)
            .timeout(self.client.config.timeout);
        if kind == AttemptKind::Create {
            request = request.header(
                "Idempotency-Key",
                idempotency_key().map_err(AttemptFailure::Error)?,
            );
        }
        let response = request
            .send()
            .await
            .map_err(|_| AttemptFailure::connection())?;
        let expected = if kind == AttemptKind::Create {
            201
        } else {
            200
        };
        let status = response.status().as_u16();
        if status != expected {
            let message = match (status, kind) {
                (403, _) => {
                    "email sign-in is not allowed for this account; sign in through the console and use runpod login --api-key"
                }
                (404, AttemptKind::Create) => "email sign-in is unavailable on this API",
                (404, AttemptKind::Verify) => {
                    "login attempt expired, was already used, or is locked; start login again"
                }
                (422, AttemptKind::Create) => "email sign-in request rejected; check --email",
                (422, AttemptKind::Verify) => return Err(AttemptFailure::Rejected),
                (429, _) => "too many login attempts; wait before trying again",
                _ => {
                    return Err(AttemptFailure::Error(login_error(format!(
                        "email sign-in failed (HTTP {status})"
                    ))));
                }
            };
            return Err(AttemptFailure::Error(login_error(message)));
        }
        let content_type = response
            .headers()
            .get("Content-Type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !JSON_TYPE.is_match(&content_type) {
            return Err(AttemptFailure::connection());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AttemptFailure::connection())?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| AttemptFailure::invalid_response())?;
        if !value.is_object() {
            return Err(AttemptFailure::invalid_response());
        }
        Ok(value)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum AttemptKind {
    Create,
    Verify,
}
enum AttemptFailure {
    Rejected,
    Error(Error),
}
impl AttemptFailure {
    fn connection() -> Self {
        Self::Error(login_error(
            "could not complete email sign-in; check your connection",
        ))
    }
    fn invalid_response() -> Self {
        Self::Error(login_error("invalid response from the email sign-in API"))
    }
    fn into_error(self) -> Error {
        match self {
            Self::Rejected => login_error("verification code rejected"),
            Self::Error(e) => e,
        }
    }
}
fn idempotency_key() -> Result<String> {
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(std::io::Error::other)?;
    random[6] = (random[6] & 0x0f) | 0x40;
    random[8] = (random[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(random));
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

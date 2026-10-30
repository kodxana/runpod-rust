//! Downloads with the Python SDK's address, redirect, and byte-limit checks.
//!
//! Every hop resolves once, validates **all** answers, and connects to those
//! addresses in order. DNS overrides retain the original TLS name and Host
//! header. Automatic redirects, retries, cookies, and implicit proxies are off.
//! Dropping a response/stream cancels its body and releases its single-use client.
//! No account credentials are imported. Explicit request headers, like Python's,
//! are sent on every redirect; do not include secrets for untrusted URLs.
//!
//! The IP classification is pinned for compatibility; see [`is_safe_address`].
//! Unlike Python's raw response, [`SafeResponse`] cannot bypass its body cap.
mod address;
mod proxy;

pub use address::is_safe_address;
pub use proxy::ProxySettings;

use crate::{Error, Result};
use futures_util::{StreamExt, future::BoxFuture, stream::BoxStream};
use reqwest::{
    Certificate, StatusCode, Url,
    header::{CONTENT_LENGTH, HOST, HeaderMap, LOCATION},
};
use std::{
    collections::BTreeMap,
    fmt, io,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Default download limit, matching upstream's 5 GiB.
pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// A policy rejection. Deliberately distinct from [`Error::Http`] so network
/// retry handlers cannot swallow unsafe URLs or excessive bodies.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SsrfError {
    /// Only HTTP and HTTPS are allowed, even when private addresses are enabled.
    #[error("blocked URL scheme {0:?}")]
    BlockedScheme(String),
    /// The URL has no usable host, port, or HTTP authority.
    #[error("invalid download URL: {0}")]
    InvalidUrl(String),
    /// DNS lookup failed.
    #[error("could not resolve host {host:?}: {source}")]
    Resolve {
        /// Host that could not be resolved.
        host: String,
        /// Resolver failure, preserved as an error source.
        source: io::Error,
    },
    /// DNS succeeded without any candidate addresses.
    #[error("no addresses resolved for host {0:?}")]
    NoAddresses(String),
    /// At least one DNS answer was unsafe; the whole result is rejected.
    #[error("host {host:?} resolves to non-public address {address}")]
    NonPublic {
        /// Original DNS name.
        host: String,
        /// Rejected address.
        address: IpAddr,
    },
    /// A proxy would invalidate the address pinning guarantee.
    #[error(
        "refusing to fetch through a proxy: IP pinning cannot be enforced via a proxy; set RUNPOD_ALLOW_PRIVATE_DOWNLOAD_URLS=true to allow it"
    )]
    Proxy,
    /// Redirect status without a usable Location header.
    #[error("redirect without Location header")]
    MissingLocation,
    /// More redirects than the configured bound.
    #[error("exceeded maximum redirects ({0})")]
    TooManyRedirects(usize),
    /// Declared or consumed body size exceeded the bound.
    #[error("download exceeds size cap: >{limit} bytes")]
    SizeLimit {
        /// Maximum permitted bytes.
        limit: u64,
        /// Content-Length when the rejection occurred before body consumption.
        declared: Option<u64>,
    },
}

/// An explicit opt-in for internal-network downloads and configured proxies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddressPolicy {
    /// Apply the upstream public-address checks to every DNS result.
    #[default]
    PublicOnly,
    /// Permit internal addresses and proxies; scheme and size checks still apply.
    AllowPrivate,
}

/// Owned download policy. [`Default`] captures environment settings once.
/// Use [`Self::from_values`] for deterministic configuration without globals.
#[derive(Clone)]
pub struct DownloadOptions {
    /// Address policy, defaulting to `PublicOnly` unless explicitly opted out.
    pub address_policy: AddressPolicy,
    /// Positive environment value or 5 GiB by default; explicit zero is allowed.
    pub max_bytes: u64,
    /// Maximum redirect responses to follow, default 5.
    pub max_redirects: usize,
    /// Connect/header deadline and per-body-read timeout, default 30 seconds.
    /// Reqwest starts its initial read deadline before connection establishment.
    pub timeout: Duration,
    /// Explicit headers, retained across redirects. Host is always recomputed.
    pub headers: HeaderMap,
    /// Explicit environment proxy snapshot, with `NO_PROXY` exclusions.
    pub proxies: ProxySettings,
    /// Additional trust roots, without disabling certificate or hostname checks.
    pub root_certificates: Vec<Certificate>,
}

impl fmt::Debug for DownloadOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadOptions")
            .field("address_policy", &self.address_policy)
            .field("max_bytes", &self.max_bytes)
            .field("max_redirects", &self.max_redirects)
            .field("timeout", &self.timeout)
            .field("header_names", &self.headers.keys())
            .field("proxies", &self.proxies)
            .field("root_certificates", &self.root_certificates.len())
            .finish()
    }
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self::from_values(&environment_values())
    }
}

fn environment_values() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

impl DownloadOptions {
    /// Capture relevant settings from an explicit environment map. Boolean opt-in
    /// accepts 1/true/yes (case insensitive). Byte limits accept positive ASCII
    /// integers fitting u64; invalid, nonpositive, or overflowing values use 5 GiB.
    #[must_use]
    pub fn from_values(environment: &BTreeMap<String, String>) -> Self {
        let allow_private = environment
            .get("RUNPOD_ALLOW_PRIVATE_DOWNLOAD_URLS")
            .is_some_and(|v| {
                matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
            });
        Self {
            address_policy: if allow_private {
                AddressPolicy::AllowPrivate
            } else {
                AddressPolicy::PublicOnly
            },
            max_bytes: parse_max_bytes(
                environment
                    .get("RUNPOD_MAX_DOWNLOAD_BYTES")
                    .map(String::as_str),
            ),
            max_redirects: 5,
            timeout: Duration::from_secs(30),
            headers: HeaderMap::new(),
            proxies: ProxySettings::from_values(environment),
            root_certificates: Vec::new(),
        }
    }
}

/// Configured download byte cap; defaults to 5 GiB.
#[must_use]
pub fn max_download_bytes() -> u64 {
    parse_max_bytes(std::env::var("RUNPOD_MAX_DOWNLOAD_BYTES").ok().as_deref())
}

fn parse_max_bytes(raw: Option<&str>) -> u64 {
    raw.and_then(|s| {
        let s = s.trim().strip_prefix('+').unwrap_or(s.trim());
        // Python's int accepts separators only between digits.
        if s.split('_')
            .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        s.replace('_', "").parse::<u64>().ok().filter(|n| *n > 0)
    })
    .unwrap_or(DEFAULT_MAX_BYTES)
}

/// Asynchronous DNS lookup used by the downloader. Answers are always validated
/// before dialing; implementations must not open connections themselves.
pub trait Resolver: Send + Sync {
    /// Resolve a host, preserving candidate order. Duplicate IPs are removed later.
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>;
}

#[derive(Debug)]
struct SystemResolver;
impl Resolver for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host, port))
                .await?
                .map(|a| a.ip())
                .collect())
        })
    }
}

/// A reusable policy and resolver, creating a separate pinned client per attempt.
#[derive(Clone)]
pub struct SafeDownloader {
    options: DownloadOptions,
    resolver: Arc<dyn Resolver>,
}

impl fmt::Debug for SafeDownloader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SafeDownloader")
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl SafeDownloader {
    /// Use the platform's DNS resolver with an explicit policy.
    #[must_use]
    pub fn new(options: DownloadOptions) -> Self {
        Self {
            options,
            resolver: Arc::new(SystemResolver),
        }
    }

    /// Supply a resolver (e.g. service discovery or a deterministic test resolver).
    #[must_use]
    pub fn with_resolver(options: DownloadOptions, resolver: Arc<dyn Resolver>) -> Self {
        Self { options, resolver }
    }

    /// Inspect the captured policy without changing an in-flight operation.
    #[must_use]
    pub fn options(&self) -> &DownloadOptions {
        &self.options
    }

    /// Resolve, deduplicate, and fail closed if any answer violates policy.
    /// IP literals are parsed directly: a resolver cannot replace a literal with
    /// a different address than the HTTP client would actually dial.
    ///
    /// # Errors
    /// Returns `Error::Ssrf` for failed/empty DNS or any rejected address.
    pub async fn resolve_and_validate(&self, host: &str, port: u16) -> Result<Vec<IpAddr>> {
        let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
            vec![ip]
        } else {
            self.resolver
                .resolve(host, port)
                .await
                .map_err(|source| SsrfError::Resolve {
                    host: host.to_owned(),
                    source,
                })?
        };
        let mut unique = Vec::new();
        for ip in addresses {
            if self.options.address_policy == AddressPolicy::PublicOnly && !address::is_safe_ip(ip)
            {
                return Err(SsrfError::NonPublic {
                    host: host.to_owned(),
                    address: ip,
                }
                .into());
            }
            if !unique.contains(&ip) {
                unique.push(ip);
            }
        }
        if unique.is_empty() {
            return Err(SsrfError::NoAddresses(host.to_owned()).into());
        }
        Ok(unique)
    }

    /// Fetch headers with validation and redirect checks. HTTP error statuses are
    /// returned unchanged; use [`SafeResponse::error_for_status`] if desired.
    /// Body consumption is lazy and always capped, including decompressed bytes.
    ///
    /// # Errors
    /// Returns `Error::Ssrf` for policy rejection and `Error::Http` for transport
    /// failures. HTTP 4xx/5xx statuses are returned as readable responses.
    pub async fn safe_get(&self, url: &str) -> Result<SafeResponse> {
        let mut current = url.to_owned();
        for _ in 0..=self.options.max_redirects {
            let parsed = ParsedUrl::new(&current)?;
            let proxy =
                self.options
                    .proxies
                    .selected(&parsed.url, &parsed.host, parsed.explicit_port);
            if proxy.is_some() && self.options.address_policy == AddressPolicy::PublicOnly {
                return Err(SsrfError::Proxy.into());
            }
            let addresses = self.resolve_and_validate(&parsed.host, parsed.port).await?;
            let (client, response) = self
                .get_via_first_reachable_ip(&parsed, &addresses, proxy)
                .await?;
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .filter(|v| !v.is_empty())
                    .ok_or(SsrfError::MissingLocation)?
                    .to_owned();
                drop(response);
                drop(client);
                current = parsed
                    .url
                    .join(&location)
                    .map_err(|e| SsrfError::InvalidUrl(e.to_string()))?
                    .into();
                continue;
            }
            if let Some(size) = response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                && size > self.options.max_bytes
            {
                return Err(SsrfError::SizeLimit {
                    limit: self.options.max_bytes,
                    declared: Some(size),
                }
                .into());
            }
            return Ok(SafeResponse {
                response,
                client,
                max_bytes: self.options.max_bytes,
            });
        }
        Err(SsrfError::TooManyRedirects(self.options.max_redirects).into())
    }

    async fn get_via_first_reachable_ip(
        &self,
        parsed: &ParsedUrl,
        addresses: &[IpAddr],
        proxy: Option<&str>,
    ) -> Result<(reqwest::Client, reqwest::Response)> {
        let mut last_error = None;
        for &ip in addresses {
            let connected = Arc::new(AtomicBool::new(false));
            let marker = Arc::clone(&connected);
            let mut builder = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(self.options.timeout)
                .read_timeout(self.options.timeout)
                .pool_max_idle_per_host(0)
                .referer(false)
                .resolve(&parsed.host, SocketAddr::new(ip, parsed.port))
                .connector_layer(tower::util::MapResponseLayer::new(move |connection| {
                    marker.store(true, Ordering::Release);
                    connection
                }));
            for certificate in &self.options.root_certificates {
                builder = builder.add_root_certificate(certificate.clone());
            }
            if let Some(proxy) = proxy {
                builder = builder.proxy(reqwest::Proxy::all(proxy)?);
            }
            let client = builder.build()?;
            let mut headers = self.options.headers.clone();
            headers.insert(
                HOST,
                parsed
                    .authority
                    .parse()
                    .map_err(|_| SsrfError::InvalidUrl("invalid Host header".into()))?,
            );
            let result = client.get(parsed.url.clone()).headers(headers).send().await;
            match result {
                Ok(response) => return Ok((client, response)),
                // The read-header timer can expire before reqwest's connector
                // timer. Track connection completion rather than treating every
                // timeout as a connection error (which would retry a slow server).
                Err(error)
                    if error.is_connect()
                        || (error.is_timeout() && !connected.load(Ordering::Acquire)) =>
                {
                    last_error = Some(error);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(last_error
            .map(Error::Http)
            .unwrap_or_else(|| SsrfError::NoAddresses(parsed.host.clone()).into()))
    }
}

impl Default for SafeDownloader {
    fn default() -> Self {
        Self::new(DownloadOptions::default())
    }
}

/// Fetch with a fresh environment snapshot and the standard DNS resolver.
///
/// # Errors
/// See [`SafeDownloader::safe_get`] for policy and transport failures.
pub async fn safe_get(url: &str) -> Result<SafeResponse> {
    SafeDownloader::default().safe_get(url).await
}

/// Resolve with a fresh environment snapshot and the standard DNS resolver.
///
/// # Errors
/// See [`SafeDownloader::resolve_and_validate`] for DNS/address rejections.
pub async fn resolve_and_validate(host: &str, port: u16) -> Result<Vec<IpAddr>> {
    SafeDownloader::default()
        .resolve_and_validate(host, port)
        .await
}

/// Response and its single-use transport, with no uncapped body access.
#[derive(Debug)]
pub struct SafeResponse {
    response: reqwest::Response,
    client: reqwest::Client,
    max_bytes: u64,
}

impl SafeResponse {
    /// HTTP status without automatically rejecting 4xx/5xx replies.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.response.status()
    }
    /// Response headers. Reqwest removes encoding/length headers on decompression.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }
    /// Final URL after validated redirects.
    #[must_use]
    pub fn url(&self) -> &Url {
        self.response.url()
    }
    /// Connected peer, useful for diagnostics and pinning checks.
    #[must_use]
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.response.remote_addr()
    }
    /// Convert an HTTP failure status into the normal HTTP error variant.
    ///
    /// # Errors
    /// Returns `Error::Http` for a 4xx or 5xx response, releasing its body.
    pub fn error_for_status(self) -> Result<Self> {
        self.response.error_for_status_ref()?;
        Ok(self)
    }
    /// Read the entire capped, decompressed body. Prefer streaming for large files.
    ///
    /// # Errors
    /// Returns `Error::Ssrf` when the body exceeds the cap, or `Error::Http` on a
    /// body-read/decompression failure. The transport is released on either error.
    pub async fn bytes(self) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        let mut stream = iter_content_capped(self, 64 * 1024, None)?;
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk?);
        }
        Ok(body)
    }
}

/// Consume a response in nonempty chunks no larger than `chunk_size`. An optional
/// extra limit can tighten, but never disable, the response's cap. Empty chunks
/// are skipped; errors terminate the stream and release transport resources.
///
/// # Errors
/// Returns `Error::InvalidInput` for zero chunk size. The returned stream yields
/// `Error::Ssrf` on exceeding its cap or `Error::Http` on body-read failure.
pub fn iter_content_capped(
    response: SafeResponse,
    chunk_size: usize,
    max_bytes: Option<u64>,
) -> Result<BoxStream<'static, Result<Vec<u8>>>> {
    if chunk_size == 0 {
        return Err(Error::InvalidInput("chunk_size must be positive".into()));
    }
    let limit = max_bytes.map_or(response.max_bytes, |n| n.min(response.max_bytes));
    Ok(Box::pin(async_stream::stream! {
        let SafeResponse { response, client, .. } = response;
        let mut stream = response.bytes_stream();
        let mut total = 0_u64;
        while let Some(bytes) = stream.next().await {
            let bytes = match bytes {
                Ok(bytes) => bytes,
                Err(error) => {
                    drop(stream);
                    drop(client);
                    yield Err(error.into());
                    return;
                }
            };
            for chunk in bytes.chunks(chunk_size) {
                if chunk.len() as u64 > limit - total {
                    drop(stream);
                    drop(client);
                    yield Err(SsrfError::SizeLimit { limit, declared: None }.into());
                    return;
                }
                total += chunk.len() as u64;
                yield Ok(chunk.to_vec());
            }
        }
        drop(stream);
        drop(client);
    }))
}

/// Build an HTTP Host authority without userinfo, retaining IPv6 brackets and
/// explicitly supplied default ports. Uses the same normalized host as dialing.
///
/// # Errors
/// Returns `Error::Ssrf` for a non-HTTP(S), malformed, or ambiguous URL.
pub fn host_header_for(url: &str) -> Result<String> {
    Ok(ParsedUrl::new(url)?.authority)
}

struct ParsedUrl {
    url: Url,
    host: String,
    port: u16,
    explicit_port: Option<u16>,
    authority: String,
}

impl ParsedUrl {
    fn new(raw: &str) -> Result<Self> {
        let scheme = raw
            .split_once(':')
            .map_or("", |(s, _)| s)
            .to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") {
            return Err(SsrfError::BlockedScheme(scheme).into());
        }
        // Require an ordinary HTTP authority before WHATWG normalization. This
        // also rejects ambiguous backslash/control-character host spellings.
        let authority = raw
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
            .filter(|s| !s.is_empty() && !s.contains('\\') && !s.chars().any(char::is_control))
            .ok_or_else(|| SsrfError::InvalidUrl("URL has no unambiguous host".into()))?;
        let authority = authority.rsplit('@').next().unwrap_or("");
        let explicit_port = if authority.starts_with('[') {
            authority
                .split_once(']')
                .and_then(|(_, tail)| tail.strip_prefix(':'))
        } else {
            authority.split_once(':').map(|(_, port)| port)
        }
        .filter(|port| !port.is_empty())
        .map(str::parse::<u16>)
        .transpose()
        .map_err(|_| SsrfError::InvalidUrl("invalid port".into()))?;
        let url = Url::parse(raw).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
        let host = url
            .host_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| SsrfError::InvalidUrl("URL has no host".into()))?;
        let authority = explicit_port.map_or_else(|| host.to_owned(), |p| format!("{host}:{p}"));
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| SsrfError::InvalidUrl("invalid port".into()))?;
        Ok(Self {
            url,
            host,
            port,
            explicit_port,
            authority,
        })
    }
}

//! Finite pod log snapshots and cursor-aware server-sent event streams.
use crate::{
    Client, Error, Result, Value,
    client::{segment, truthy},
    json,
};
use futures_util::{Stream, StreamExt, stream::BoxStream};
use std::time::Duration;

/// Maximum duration of a log stream, including connection establishment.
pub const STREAM_TIMEOUT: Duration = Duration::from_secs(3600);
const SOCKET_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Selection, backfill window, and resumption settings for a log stream.
#[derive(Debug, Clone)]
pub struct LogStreamOptions {
    /// Owning endpoint for serverless worker logs; None selects a dedicated pod.
    pub endpoint_id: Option<String>,
    /// "all" uses HAPI; a source or None uses REST, with None requesting both sources.
    pub log_type: Option<String>,
    /// Number of historical lines to request.
    pub tail: i64,
    /// Optional earliest timestamp for backfill.
    pub since: Option<String>,
    /// Last successfully delivered event cursor from a previous stream.
    pub last_event_id: Option<String>,
    /// Ends an established stream after waiting this long for a complete line.
    pub idle_timeout: Option<Duration>,
}
impl Default for LogStreamOptions {
    fn default() -> Self {
        Self {
            endpoint_id: None,
            log_type: Some("all".into()),
            tail: 100,
            since: None,
            last_event_id: None,
            idle_timeout: None,
        }
    }
}

/// Called immediately before a complete valid event is delivered; errors stop the stream.
pub type EventIdObserver = Box<dyn FnMut(&str) -> Result<()> + Send + 'static>;

/// Log access using an authenticated snapshot pool and a stream pool with socket-read deadlines.
#[derive(Debug, Clone)]
pub struct LogsClient {
    client: Client,
    stream_http: reqwest::Client,
    hapi_base: String,
}
impl LogsClient {
    /// Resolves HAPI from RUNPOD_HAPI_URL or <https://hapi.runpod.net>.
    ///
    /// # Errors
    /// Returns invalid HAPI service configuration errors.
    pub fn new(client: Client) -> Result<Self> {
        Self::with_hapi_base(
            client,
            std::env::var("RUNPOD_HAPI_URL").unwrap_or_else(|_| "https://hapi.runpod.net".into()),
        )
    }
    /// Uses an explicit HAPI root for private gateways or local services.
    ///
    /// # Errors
    /// Rejects service URLs with unsupported schemes, credentials, queries, or fragments.
    pub fn with_hapi_base(client: Client, hapi_base: impl Into<String>) -> Result<Self> {
        let hapi_base = hapi_base.into();
        let url =
            reqwest::Url::parse(&hapi_base).map_err(|e| Error::InvalidInput(e.to_string()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::InvalidInput(
                "HAPI URL must be HTTP(S) without credentials, query, or fragment".into(),
            ));
        }
        let stream_http = client.http_with_read_timeout(SOCKET_READ_TIMEOUT)?;
        Ok(Self {
            client,
            stream_http,
            hapi_base: hapi_base.trim_end_matches('/').into(),
        })
    }
    /// Gets a finite snapshot, replacing false-like source values with empty lists.
    ///
    /// # Errors
    /// Returns request, content-type, JSON, or malformed snapshot errors.
    pub async fn pod_logs(&self, pod_id: &str, log_type: &str, timeout: Duration) -> Result<Value> {
        let mut request = self
            .client
            .http
            .get(format!(
                "{}/v1/pod/{}/logs",
                self.hapi_base,
                segment(pod_id)
            ))
            .query(&[("type", log_type)]);
        if !timeout.is_zero() {
            request = request.timeout(timeout);
        }
        let response = request.send().await?.error_for_status()?;
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
            return Err(Error::InvalidResponse(
                "log snapshot must have a JSON content type".into(),
            ));
        }
        let mut data: Value = response.json().await?;
        let sources = data
            .as_object_mut()
            .ok_or_else(|| Error::InvalidResponse("log snapshot must be an object".into()))?;
        for value in sources.values_mut() {
            if !truthy(value) {
                *value = json!([]);
            }
        }
        Ok(data)
    }
    /// Lazily follows a pod or worker log stream; invalid/incomplete events do not advance the cursor.
    ///
    /// Idle timeout starts after headers. HTTP/connection failures are yielded as errors;
    /// idle completion ends normally. Dropping the stream cancels its connection.
    pub fn stream_pod_logs(
        &self,
        pod_id: &str,
        options: LogStreamOptions,
        mut on_event_id: Option<EventIdObserver>,
    ) -> BoxStream<'static, Result<Value>> {
        let client = self.clone();
        let pod_id = pod_id.to_owned();
        Box::pin(async_stream::try_stream! {
            let mut params = vec![("tail", options.tail.to_string())];
            let url = if options.log_type.as_deref() == Some("all") {
                params.extend([("stream", "true".into()), ("type", "all".into())]);
                format!("{}/v1/pod/{}/logs", client.hapi_base, segment(&pod_id))
            } else {
                if let Some(source) = options.log_type.as_ref().filter(|s| !s.is_empty()) {
                    params.push(("source", source.clone()));
                }
                let base = client.client.config.api_base_url.trim_end_matches('/');
                if let Some(endpoint) = &options.endpoint_id {
                    format!("{base}/v2/serverless/{}/workers/{}/logs", segment(endpoint), segment(&pod_id))
                } else {
                    format!("{base}/v2/pods/{}/logs", segment(&pod_id))
                }
            };
            if let Some(since) = options.since.as_ref().filter(|s| !s.is_empty()) {
                params.push(("since", since.clone()));
            }
            let mut request = client.stream_http.get(url).query(&params)
                .header("Accept", "text/event-stream").timeout(STREAM_TIMEOUT);
            if let Some(cursor) = options.last_event_id.as_ref().filter(|s| !s.is_empty()) {
                request = request.header("Last-Event-ID", cursor);
            }
            let response = request.send().await?.error_for_status()?;
            let mut reader = LineReader::new(response.bytes_stream());
            let mut cursor: Option<String> = None;
            let mut data = Vec::new();
            loop {
                if options.idle_timeout == Some(Duration::ZERO) { break; }
                let raw = if let Some(idle) = options.idle_timeout {
                    match tokio::time::timeout(idle, reader.next_line()).await {
                        Err(_) => break,
                        Ok(Err(Error::Http(error))) if error.is_timeout() => break,
                        Ok(result) => result?,
                    }
                } else {
                    reader.next_line().await?
                };
                let Some(raw) = raw else { break; };
                let decoded = String::from_utf8_lossy(&raw);
                let line = decoded.trim_end_matches(['\r', '\n']);
                if !line.is_empty() {
                    let (field, value) = line.split_once(':').unwrap_or((line, ""));
                    let value = value.strip_prefix(' ').unwrap_or(value);
                    if field == "id" && !value.contains('\0') {
                        cursor = Some(value.to_owned());
                    } else if field == "data" {
                        data.push(value.to_owned());
                    }
                    continue;
                }
                if data.is_empty() { continue; }
                let result = serde_json::from_str::<Value>(&data.join("\n"));
                data.clear();
                let Ok(event) = result else { continue; };
                if let (Some(cursor), Some(observer)) = (&cursor, &mut on_event_id) {
                    observer(cursor)?;
                }
                yield event;
            }
        })
    }
}

struct LineReader<S> {
    stream: S,
    buffer: Vec<u8>,
    start: usize,
    scan: usize,
    eof: bool,
}
impl<S, B> LineReader<S>
where
    S: Stream<Item = std::result::Result<B, reqwest::Error>> + Unpin,
    B: AsRef<[u8]>,
{
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            start: 0,
            scan: 0,
            eof: false,
        }
    }
    async fn next_line(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(index) = self.buffer[self.scan..].iter().position(|b| *b == b'\n') {
                let end = self.scan + index + 1;
                let line = self.buffer[self.start..end].to_vec();
                self.start = end;
                self.scan = end;
                return Ok(Some(line));
            }
            self.scan = self.buffer.len();
            if self.eof {
                let result =
                    (self.start < self.buffer.len()).then(|| self.buffer[self.start..].to_vec());
                self.buffer.clear();
                self.start = 0;
                self.scan = 0;
                return Ok(result);
            }
            if self.start > 0 {
                self.buffer.drain(..self.start);
                self.scan -= self.start;
                self.start = 0;
            }
            match self.stream.next().await {
                Some(chunk) => self.buffer.extend_from_slice(chunk?.as_ref()),
                None => self.eof = true,
            }
        }
    }
}

/// Formats system then container log tails, preserving Python's zero/negative slice behavior.
///
/// # Errors
/// Returns invalid snapshot errors if a nonempty source is not a list of strings.
pub fn tail_summary(logs: &Value, lines: i64) -> Result<String> {
    let mut parts = Vec::new();
    for source in ["system", "container"] {
        let Some(entries) = logs.get(source).filter(|v| truthy(v)) else {
            continue;
        };
        let entries = entries
            .as_array()
            .ok_or_else(|| Error::InvalidResponse("log source must be an array".into()))?;
        let count = usize::try_from(lines.unsigned_abs()).unwrap_or(usize::MAX);
        let start = if lines > 0 {
            entries.len().saturating_sub(count)
        } else if lines < 0 {
            count.min(entries.len())
        } else {
            0
        };
        let heading = if lines < 0 {
            lines.to_string()
        } else {
            count.min(entries.len()).to_string()
        };
        parts.push(format!("--- {source} (last {heading}) ---"));
        for entry in &entries[start..] {
            parts.push(
                entry
                    .as_str()
                    .ok_or_else(|| Error::InvalidResponse("log line must be a string".into()))?
                    .into(),
            );
        }
    }
    Ok(if parts.is_empty() {
        "(no logs available)".into()
    } else {
        parts.join("\n")
    })
}

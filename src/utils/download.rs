//! File downloads with ordered batch results, bounded concurrency, and ZIP extraction.
//!
//! Network/policy checks come from [`super::ssrf`]. Batch network failures are retried
//! three times and become `None`; policy and filesystem failures propagate. Completed
//! files belong to the caller. Failed/canceled writes may leave partial files, as in
//! Python. ZIP extraction sanitizes names and writes symlink entries as ordinary files.
//! Its expanded size is not limited by the network byte cap (matching upstream).

use super::ssrf::{self, DownloadOptions, Resolver, SafeDownloader, SafeResponse};
use crate::{Error, Result};
use futures_util::{StreamExt, stream};
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    io::{Read, Write},
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

/// Choose 1 KiB, 1 MiB, or 10 MiB chunks at the inclusive 1 MiB/1 GiB boundaries.
#[must_use]
pub fn calculate_chunk_size(file_size: i128) -> usize {
    if file_size <= 1024 * 1024 {
        1024
    } else if file_size <= 1024 * 1024 * 1024 {
        1024 * 1024
    } else {
        10 * 1024 * 1024
    }
}

/// Read a signed ASCII Content-Length for chunk selection. Missing, malformed,
/// and out-of-i128 values return zero. This does not replace streaming size checks.
#[must_use]
pub fn parse_content_length(headers: &HeaderMap) -> i128 {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            let s = s.trim();
            let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
            if unsigned
                .split('_')
                .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
            {
                return None;
            }
            s.replace('_', "").parse().ok()
        })
        .unwrap_or(0)
}

/// Preserve the source's simple semicolon/key=value parsing. Keys are lowercased,
/// repeated keys use their last value, and surrounding double quotes are removed.
/// Quoted semicolons and RFC 5987 `filename*` are not specially interpreted.
#[must_use]
pub fn extract_disposition_params(content_disposition: &str) -> BTreeMap<String, String> {
    content_disposition
        .split(';')
        .filter_map(|part| {
            part.trim().split_once('=').map(|(key, value)| {
                (
                    key.trim().to_lowercase(),
                    value.trim().trim_matches('"').to_owned(),
                )
            })
        })
        .collect()
}

/// One URL or a list with optional absent inputs, matching Python's batch input.
#[derive(Clone, Debug)]
pub enum DownloadUrls {
    /// A single URL, still returning a one-element batch result.
    Single(String),
    /// Input-order list; `None` performs no request and returns `None`.
    Multiple(Vec<Option<String>>),
}
impl From<&str> for DownloadUrls {
    fn from(value: &str) -> Self {
        Self::Single(value.to_owned())
    }
}
impl From<String> for DownloadUrls {
    fn from(value: String) -> Self {
        Self::Single(value)
    }
}
impl From<Vec<String>> for DownloadUrls {
    fn from(value: Vec<String>) -> Self {
        Self::Multiple(value.into_iter().map(Some).collect())
    }
}
impl From<Vec<Option<String>>> for DownloadUrls {
    fn from(value: Vec<Option<String>>) -> Self {
        Self::Multiple(value)
    }
}

/// Downloaded file metadata. Serialized field names match the Python dictionary.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadedFile {
    /// Absolute path of the downloaded archive or file.
    pub file_path: PathBuf,
    /// Original extension without dots (case is retained).
    #[serde(rename = "type")]
    pub file_type: String,
    /// Content-Disposition filename, falling back to the original URL basename.
    pub original_name: String,
    /// Absolute extraction directory for lowercase `.zip` inputs only.
    pub extracted_path: Option<PathBuf>,
}

/// Transport and scheduling configuration, captured once for an owned downloader.
#[derive(Clone, Debug)]
pub struct FileDownloadOptions {
    /// Address, proxy, headers, and byte-limit policy. Timeouts below take precedence.
    pub download: DownloadOptions,
    /// Concurrent batch requests; defaults to min(32, available CPUs + 4).
    pub concurrency: NonZeroUsize,
    /// Batch request timeout, default five seconds.
    pub batch_timeout: Duration,
    /// Single-file request timeout, default thirty seconds.
    pub file_timeout: Duration,
    /// Full-jitter retry ceilings are this duration and twice this duration.
    /// Default one second; zero disables waiting (useful for deterministic tests).
    pub retry_base_delay: Duration,
}
impl Default for FileDownloadOptions {
    fn default() -> Self {
        Self {
            download: DownloadOptions::default(),
            concurrency: std::thread::available_parallelism()
                .unwrap_or(NonZeroUsize::MIN)
                .saturating_add(4)
                .min(NonZeroUsize::new(32).unwrap_or(NonZeroUsize::MIN)),
            batch_timeout: Duration::from_secs(5),
            file_timeout: Duration::from_secs(30),
            retry_base_delay: Duration::from_secs(1),
        }
    }
}

/// Downloader rooted at an explicit project directory, without changing process cwd.
#[derive(Clone)]
pub struct FileDownloader {
    root: PathBuf,
    options: FileDownloadOptions,
    resolver: Option<Arc<dyn Resolver>>,
}
impl fmt::Debug for FileDownloader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileDownloader")
            .field("root", &self.root)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}
impl FileDownloader {
    /// Capture an absolute root and the default environment/options. Creates no files.
    /// # Errors
    /// Returns an I/O error if the absolute root cannot be determined.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            root: absolute_lexical(&std::path::absolute(root)?),
            options: FileDownloadOptions::default(),
            resolver: None,
        })
    }
    /// Replace transport and scheduling options before starting operations.
    #[must_use]
    pub fn with_options(mut self, options: FileDownloadOptions) -> Self {
        self.options = options;
        self
    }
    /// Use an explicit resolver; the transport still validates all returned addresses.
    #[must_use]
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }
    /// Absolute project root used for `jobs` and `job_files`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn transport(&self, timeout: Duration) -> SafeDownloader {
        let mut options = self.options.download.clone();
        options.timeout = timeout;
        options
            .headers
            .entry(USER_AGENT)
            .or_insert(HeaderValue::from_static(concat!(
                "runpod-rust/",
                env!("CARGO_PKG_VERSION"),
                " (https://runpod.io; support@runpod.io)"
            )));
        match &self.resolver {
            Some(resolver) => SafeDownloader::with_resolver(options, Arc::clone(resolver)),
            None => SafeDownloader::new(options),
        }
    }

    /// Download a batch into `jobs/{job_id}/downloaded_files`, preserving input order.
    /// All scheduled items finish before returning a policy/filesystem error; the first
    /// such error in input order wins. Three exhausted HTTP attempts return `None`.
    ///
    /// `job_id` is a trusted filesystem path, as in Python: absolute paths and parent
    /// components can select another directory. Validate untrusted IDs before calling.
    ///
    /// # Errors
    /// Returns policy, filesystem, or random-source errors. Cancellation drops active
    /// network operations; an already submitted filesystem write may finish afterwards.
    pub async fn download_files_from_urls(
        &self,
        job_id: &str,
        urls: impl Into<DownloadUrls>,
    ) -> Result<Vec<Option<PathBuf>>> {
        let directory = absolute_lexical(&std::path::absolute(
            self.root.join("jobs").join(job_id).join("downloaded_files"),
        )?);
        tokio::fs::create_dir_all(&directory).await?;
        let urls = match urls.into() {
            DownloadUrls::Single(url) => vec![Some(url)],
            DownloadUrls::Multiple(urls) => urls,
        };
        let transport = self.transport(self.options.batch_timeout);
        let mut results: Vec<_> = stream::iter(urls.into_iter().enumerate().map(|(index, url)| {
            let directory = &directory;
            let transport = &transport;
            async move {
                (
                    index,
                    self.download_one(transport, directory, url.as_deref())
                        .await,
                )
            }
        }))
        .buffer_unordered(self.options.concurrency.get())
        .collect()
        .await;
        results.sort_unstable_by_key(|(index, _)| *index);
        results.into_iter().map(|(_, result)| result).collect()
    }

    async fn download_one(
        &self,
        transport: &SafeDownloader,
        directory: &Path,
        url: Option<&str>,
    ) -> Result<Option<PathBuf>> {
        let Some(url) = url else {
            return Ok(None);
        };
        let name = random_name()?;
        for attempt in 0..3 {
            let result = async {
                let response = transport.safe_get(url).await?.error_for_status()?;
                let disposition = disposition(response.headers());
                let filename = disposition
                    .as_deref()
                    .map(extract_disposition_params)
                    .and_then(|mut params| params.remove("filename"))
                    .unwrap_or_default();
                let extension = extension(&filename);
                let extension = if extension.is_empty() {
                    extension_of_url(url)
                } else {
                    extension.to_owned()
                };
                let path = directory.join(format!("{name}{extension}"));
                write_response(response, &path).await?;
                Ok(std::path::absolute(path)?)
            }
            .await;
            match result {
                Ok(path) => return Ok(Some(path)),
                Err(Error::Http(error)) => {
                    if attempt == 2 {
                        tracing::warn!(status = ?error.status(), "File download failed after three attempts");
                        return Ok(None);
                    }
                    retry_sleep(self.options.retry_base_delay.saturating_mul(1 << attempt)).await?;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the third attempt always returns")
    }

    /// Download one file into `job_files`, returning metadata and extracting lowercase ZIPs.
    /// This operation does not retry. HTTP status is checked before writing or extracting.
    ///
    /// # Errors
    /// Returns policy, transport, filesystem, random-source, or ZIP errors. Partial files
    /// remain on failure, matching upstream. ZIP extraction runs on a blocking worker;
    /// cancellation requests that it stop at its next entry/read boundary.
    pub async fn file(&self, url: &str) -> Result<DownloadedFile> {
        let directory = self.root.join("job_files");
        tokio::fs::create_dir_all(&directory).await?;
        let response = self
            .transport(self.options.file_timeout)
            .safe_get(url)
            .await?
            .error_for_status()?;
        let disposition = disposition(response.headers());
        let mut original_name = disposition
            .as_deref()
            .map(extract_disposition_params)
            .and_then(|mut params| params.remove("filename"))
            .unwrap_or_default();
        if original_name.is_empty() {
            original_name = basename(url_path(url)).to_owned();
        }
        let file_type = extension(&original_name).replace('.', "");
        let name = random_name()?;
        let file_path = directory.join(format!("{name}.{file_type}"));
        write_response(response, &file_path).await?;
        let extracted_path = if file_type == "zip" {
            let path = directory.join(&name);
            tokio::fs::create_dir(&path).await?;
            let cancellation = CancelExtraction(Arc::new(AtomicBool::new(false)));
            let canceled = Arc::clone(&cancellation.0);
            let archive = file_path.clone();
            let target = path.clone();
            tokio::task::spawn_blocking(move || extract_zip(&archive, &target, &canceled))
                .await??;
            Some(path)
        } else {
            None
        };
        Ok(DownloadedFile {
            file_path: std::path::absolute(file_path)?,
            file_type,
            original_name,
            extracted_path,
        })
    }
}

/// Download a batch relative to the current directory with captured environment defaults.
/// # Errors
/// See [`FileDownloader::download_files_from_urls`].
pub async fn download_files_from_urls(
    job_id: &str,
    urls: impl Into<DownloadUrls>,
) -> Result<Vec<Option<PathBuf>>> {
    FileDownloader::new(std::env::current_dir()?)?
        .download_files_from_urls(job_id, urls)
        .await
}
/// Download a single file relative to the current directory with captured defaults.
/// # Errors
/// See [`FileDownloader::file`].
pub async fn file(url: &str) -> Result<DownloadedFile> {
    FileDownloader::new(std::env::current_dir()?)?
        .file(url)
        .await
}

async fn write_response(response: SafeResponse, path: &Path) -> Result<()> {
    let chunk_size = calculate_chunk_size(parse_content_length(response.headers()));
    let mut stream = ssrf::iter_content_capped(response, chunk_size, None)?;
    let mut output = tokio::fs::File::create(path).await?;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => output.write_all(&chunk).await?,
            Err(error) => {
                // Match Python's file-context exit: finish buffered writes before
                // reporting a read/policy failure and exposing its partial file.
                output.flush().await?;
                return Err(error);
            }
        }
    }
    output.flush().await?;
    Ok(())
}
fn disposition(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_DISPOSITION)
        .map(|v| v.as_bytes().iter().map(|b| char::from(*b)).collect())
}
fn basename(path: &str) -> &str {
    path.rsplit(|c| c == '/' || (cfg!(windows) && c == '\\'))
        .next()
        .unwrap_or("")
}
fn extension(path: &str) -> &str {
    let name = basename(path);
    match name.rfind('.') {
        Some(index) if name[..index].chars().any(|c| c != '.') => &name[index..],
        _ => "",
    }
}
fn url_path(url: &str) -> &str {
    let url = url.split(['?', '#']).next().unwrap_or("");
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest);
    authority.find('/').map_or("", |index| &authority[index..])
}
fn extension_of_url(url: &str) -> String {
    extension(url_path(url)).to_owned()
}
fn absolute_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
fn random_name() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
async fn retry_sleep(ceiling: Duration) -> Result<()> {
    let nanos = u64::try_from(ceiling.as_nanos()).unwrap_or(u64::MAX);
    if nanos > 0 {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
        tokio::time::sleep(Duration::from_nanos(u64::from_le_bytes(bytes) % nanos)).await;
    }
    Ok(())
}

struct CancelExtraction(Arc<AtomicBool>);
impl Drop for CancelExtraction {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
fn check_canceled(canceled: &AtomicBool) -> Result<()> {
    if canceled.load(Ordering::Acquire) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "ZIP extraction canceled",
        )
        .into());
    }
    Ok(())
}
fn extract_zip(archive: &Path, target: &Path, canceled: &AtomicBool) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    let mut entries = (0..archive.len())
        .map(|index| Ok((archive.by_index_data(index)?.central_header_start(), index)))
        .collect::<zip::result::ZipResult<Vec<_>>>()?;
    // The ZIP crate coalesces duplicate raw names. Ordering by each name's last
    // central record preserves Python's final-write precedence when sanitized
    // names collide, including a/../b vs a/b and ./name vs name.
    entries.sort_unstable_by_key(|(offset, _)| *offset);
    let mut buffer = vec![0; 64 * 1024];
    for (_, index) in entries {
        check_canceled(canceled)?;
        let mut entry = archive.by_index(index)?;
        let name = entry.name()?.into_owned();
        let name = name.split('\0').next().unwrap_or("");
        let is_directory = name.ends_with('/') || (cfg!(windows) && name.ends_with('\\'));
        let relative = zip_relative_path(name);
        if relative.as_os_str().is_empty() && !is_directory {
            return Err(Error::InvalidInput("Empty ZIP filename.".into()));
        }
        let path = target.join(relative);
        if is_directory {
            std::fs::create_dir_all(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Python extracts link entries as their data bytes, not as filesystem links.
        let mut output = std::fs::File::create(path)?;
        loop {
            check_canceled(canceled)?;
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
        }
    }
    Ok(())
}
fn zip_relative_path(name: &str) -> PathBuf {
    let name = if cfg!(windows) {
        name.replace('\\', "/")
    } else {
        name.to_owned()
    };
    // Windows drops drive/UNC roots before sanitizing; POSIX treats them as names.
    let name = if cfg!(windows) {
        if name.starts_with("//") {
            let start = if name
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/UNC/"))
            {
                8
            } else {
                2
            };
            name[start..].splitn(3, '/').nth(2).unwrap_or("")
        } else if name.chars().nth(1) == Some(':') {
            &name[name
                .char_indices()
                .nth(2)
                .map_or(name.len(), |(index, _)| index)..]
        } else {
            &name
        }
    } else {
        &name
    };
    name.split('/')
        .filter(|part| !matches!(*part, "" | "." | ".."))
        .filter_map(|part| {
            let part = if cfg!(windows) {
                part.replace([':', '<', '>', '|', '"', '?', '*'], "_")
                    .trim_end_matches([' ', '.'])
                    .to_owned()
            } else {
                part.to_owned()
            };
            if part.is_empty() { None } else { Some(part) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canceled_zip_worker_writes_no_members() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input.zip");
        std::fs::write(&input, include_bytes!("../../tests/fixtures/download.zip")).unwrap();
        let output = root.path().join("extracted");
        std::fs::create_dir(&output).unwrap();
        let result = extract_zip(&input, &output, &AtomicBool::new(true));
        assert!(matches!(result, Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::Interrupted));
        assert!(std::fs::read_dir(output).unwrap().next().is_none());
    }
}

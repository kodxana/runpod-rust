//! Native S3 uploads and the Python SDK's local-file fallback.
//!
//! Paths, filenames, prefixes, and job IDs are trusted input, as in the Python SDK.
//! A root controls relative file access; it is not a filesystem sandbox. Missing
//! bucket credentials select local fallback. Once configured, S3 errors propagate
//! and never silently switch an upload to disk. No Runpod API key is used here.
//!
//! The AWS SDK supplies SigV4, HTTPS, retryable bodies, and standard HTTP retries.
//! Transfers start multipart at 25 KiB, using parts of at least 5 MiB. Dropping an
//! in-progress multipart transfer schedules best-effort abort on the Tokio runtime;
//! keep that runtime alive to let cleanup finish. Failed uploads await abort.

mod extra;
mod transfer;

use crate::{Error, Result, Value};
use aws_sdk_s3::{
    config::{Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use chrono::{DateTime, Local};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub use aws_sdk_s3::Client as S3Client;
pub use transfer::multipart_chunk_size;

/// Explicit S3-compatible endpoint credentials (the Python dictionary names).
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketCredentials {
    pub endpoint_url: String,
    pub access_id: String,
    pub access_secret: String,
    /// Used by `bucket_upload`; other helpers use their separate bucket argument.
    pub bucket_name: Option<String>,
}
impl std::fmt::Debug for BucketCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BucketCredentials").finish_non_exhaustive()
    }
}

/// Python TransferConfig defaults. The configured chunk size is adjusted to S3 limits.
#[derive(Clone, Debug)]
pub struct TransferConfig {
    pub multipart_threshold: u64,
    pub multipart_chunksize: u64,
    pub max_concurrency: NonZeroUsize,
}
impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            multipart_threshold: 25 * 1024,
            multipart_chunksize: 25 * 1024,
            max_concurrency: std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// Owned configuration. Environment is captured once, with no process-global mutation.
#[derive(Clone)]
pub struct UploadOptions {
    pub credentials: Option<BucketCredentials>,
    /// BUCKET_ENDPOINT_URL, BUCKET_ACCESS_KEY_ID, BUCKET_SECRET_ACCESS_KEY, AWS_DEFAULT_REGION.
    pub environment: BTreeMap<String, String>,
    pub transfer: TransferConfig,
    /// Override the native region fallback (otherwise AWS_DEFAULT_REGION, then us-east-1).
    pub region: Option<String>,
    /// Optional clock for month-based buckets and reproducible presigned URLs.
    pub now: Option<SystemTime>,
}
impl std::fmt::Debug for UploadOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UploadOptions")
            .field("transfer", &self.transfer)
            .field("region", &self.region)
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}
impl Default for UploadOptions {
    fn default() -> Self {
        let environment = [
            "BUCKET_ENDPOINT_URL",
            "BUCKET_ACCESS_KEY_ID",
            "BUCKET_SECRET_ACCESS_KEY",
            "AWS_DEFAULT_REGION",
        ]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect();
        Self {
            credentials: None,
            environment,
            transfer: TransferConfig::default(),
            region: None,
            now: None,
        }
    }
}
impl UploadOptions {
    fn credentials(&self) -> Option<BucketCredentials> {
        self.credentials
            .clone()
            .or_else(|| {
                Some(BucketCredentials {
                    endpoint_url: self.environment.get("BUCKET_ENDPOINT_URL")?.clone(),
                    access_id: self.environment.get("BUCKET_ACCESS_KEY_ID")?.clone(),
                    access_secret: self.environment.get("BUCKET_SECRET_ACCESS_KEY")?.clone(),
                    bucket_name: None,
                })
            })
            .filter(|c| {
                !c.endpoint_url.is_empty() && !c.access_id.is_empty() && !c.access_secret.is_empty()
            })
    }
    fn default_region(&self) -> &str {
        self.region
            .as_deref()
            .or_else(|| {
                self.environment
                    .get("AWS_DEFAULT_REGION")
                    .map(String::as_str)
            })
            .unwrap_or("us-east-1")
    }
}

/// Transfer arguments with the original boto3 ExtraArgs names and JSON value types.
/// Dates accept RFC3339/HTTP date strings or Unix seconds; metadata values must be strings.
#[derive(Clone, Default)]
pub struct UploadRequest {
    pub bucket_name: Option<String>,
    pub prefix: Option<String>,
    pub extra_args: BTreeMap<String, Value>,
}
impl std::fmt::Debug for UploadRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UploadRequest")
            .field("extra_arg_names", &self.extra_args.keys())
            .finish_non_exhaustive()
    }
}

/// An S3 failure. Display omits response bodies, credentials, keys, and endpoint URLs.
#[derive(Debug, thiserror::Error)]
#[error("S3 {operation} failed")]
pub struct UploadError {
    operation: &'static str,
    status_code: Option<u16>,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl UploadError {
    #[must_use]
    pub fn status_code(&self) -> Option<u16> {
        self.status_code
    }
    #[must_use]
    pub fn operation(&self) -> &'static str {
        self.operation
    }
    fn sdk<E>(operation: &'static str, error: aws_sdk_s3::error::SdkError<E>) -> Error
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        let status_code = error.raw_response().map(|r| r.status().as_u16());
        Self {
            operation,
            status_code,
            source: Box::new(error),
        }
        .into()
    }
    fn local(
        operation: &'static str,
        error: impl std::error::Error + Send + Sync + 'static,
    ) -> Error {
        Self {
            operation,
            status_code: None,
            source: Box::new(error),
        }
        .into()
    }
}

/// Preserve the source's string-based extraction, including its DigitalOcean quirk.
#[must_use]
pub fn extract_region_from_url(endpoint_url: &str) -> Option<String> {
    if let Some((_, rest)) = endpoint_url.split_once(".s3.") {
        return Some(rest.split('.').next().unwrap_or("").to_owned());
    }
    // Python urlparse preserves netloc case and includes credentials and explicit ports.
    let authority = endpoint_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .or_else(|| endpoint_url.strip_prefix("//"))
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or("");
    if authority.ends_with(".digitaloceanspaces.com") {
        return endpoint_url.split('.').nth(1).map(str::to_owned);
    }
    None
}

fn client(credentials: &BucketCredentials, region: &str) -> Result<S3Client> {
    let parsed = reqwest::Url::parse(&credentials.endpoint_url)
        .map_err(|_| Error::InvalidInput("invalid bucket endpoint URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(Error::InvalidInput(
            "bucket endpoint must use HTTP or HTTPS".into(),
        ));
    }
    Ok(S3Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new(region.to_owned()))
            .credentials_provider(Credentials::new(
                &credentials.access_id,
                &credentials.access_secret,
                None,
                None,
                "runpod-bucket",
            ))
            .endpoint_url(&credentials.endpoint_url)
            .force_path_style(true)
            // botocore max_attempts=3 means 3 retries, not 3 total attempts.
            .retry_config(RetryConfig::standard().with_max_attempts(4))
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(60))
                    .read_timeout(Duration::from_secs(60))
                    .build(),
            )
            .build(),
    ))
}

/// Native equivalent of get_boto_client. The Rust dependency is available at build time.
pub fn get_boto_client(
    bucket_creds: Option<&BucketCredentials>,
) -> Result<(Option<S3Client>, TransferConfig)> {
    let options = UploadOptions {
        credentials: bucket_creds.cloned(),
        ..UploadOptions::default()
    };
    Ok((configured_client(&options)?, options.transfer))
}
fn configured_client(options: &UploadOptions) -> Result<Option<S3Client>> {
    options
        .credentials()
        .map(|creds| {
            let region = extract_region_from_url(&creds.endpoint_url);
            client(
                &creds,
                region
                    .as_deref()
                    .unwrap_or_else(|| options.default_region()),
            )
        })
        .transpose()
}

/// Async upload utilities using an owned root, environment snapshot, and S3 client.
#[derive(Clone, Debug)]
pub struct Uploader {
    root: PathBuf,
    options: UploadOptions,
    s3: Option<S3Client>,
}
impl Uploader {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        Self::with_options(root, UploadOptions::default())
    }
    pub fn with_options(root: impl AsRef<Path>, options: UploadOptions) -> Result<Self> {
        let s3 = configured_client(&options)?;
        Ok(Self {
            root: std::path::absolute(root)?,
            options,
            s3,
        })
    }
    /// Supply an explicitly configured AWS client, for custom transport or credential providers.
    #[must_use]
    pub fn with_client(mut self, client: S3Client) -> Self {
        self.s3 = Some(client);
        self
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    #[must_use]
    pub fn s3_client(&self) -> Option<&S3Client> {
        self.s3.as_ref()
    }
    fn bucket(&self, bucket: Option<&str>) -> String {
        bucket
            .filter(|b| !b.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let now: DateTime<Local> = self.options.now.unwrap_or_else(SystemTime::now).into();
                now.format("%m-%y").to_string()
            })
    }
    async fn presign(&self, s3: &S3Client, bucket: &str, key: &str) -> Result<String> {
        let mut config = PresigningConfig::builder().expires_in(Duration::from_secs(604_800));
        if let Some(now) = self.options.now {
            config = config.start_time(now);
        }
        let config = config
            .build()
            .map_err(|e| UploadError::local("presign configuration", e))?;
        Ok(s3
            .get_object()
            .bucket(bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(|e| UploadError::sdk("presign", e))?
            .uri()
            .to_owned())
    }
    /// Copy or write the local fallback, returning the original relative path spelling.
    /// Only `directory` is created; parents inside `file_name` are not created.
    pub async fn save_to_local_fallback(
        &self,
        file_name: &str,
        source_path: Option<&Path>,
        file_data: Option<&[u8]>,
        directory: &str,
    ) -> Result<String> {
        tracing::warn!("No bucket endpoint set; saving upload to disk");
        tokio::fs::create_dir_all(self.root.join(directory)).await?;
        let location = format!("{directory}/{file_name}");
        let target = self.root.join(&location);
        if let Some(source) = source_path.filter(|p| !p.as_os_str().is_empty()) {
            let source = self.root.join(source);
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileTypeExt;
                for path in [&source, &target] {
                    match tokio::fs::metadata(path).await {
                        Ok(meta) if meta.file_type().is_fifo() => {
                            return Err(Error::InvalidInput("cannot copy a named pipe".into()));
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            // copyfile refuses identical files/hard links before opening the destination.
            let from = source.clone();
            let to = target.clone();
            let same =
                tokio::task::spawn_blocking(move || match same_file::is_same_file(from, to) {
                    Ok(same) => Ok(same),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(e) => Err(e),
                })
                .await??;
            if same {
                return Err(Error::InvalidInput(
                    "source and destination are the same file".into(),
                ));
            }
            let mut source = tokio::fs::File::open(source).await?;
            let mut target = tokio::fs::File::create(target).await?;
            let copied = tokio::io::copy(&mut source, &mut target).await;
            tokio::io::AsyncWriteExt::flush(&mut target).await?;
            copied?;
        } else if let Some(data) = file_data {
            tokio::fs::write(target, data).await?;
        } else {
            return Err(Error::InvalidInput(
                "Either source_path or file_data must be provided".into(),
            ));
        }
        Ok(location)
    }
    pub async fn upload_file_to_bucket(
        &self,
        file_name: &str,
        file_location: impl AsRef<Path>,
        request: &UploadRequest,
    ) -> Result<String> {
        let Some(s3) = &self.s3 else {
            return self
                .save_to_local_fallback(
                    file_name,
                    Some(file_location.as_ref()),
                    None,
                    "local_upload",
                )
                .await;
        };
        let path = self.root.join(file_location);
        let size = tokio::fs::metadata(&path).await?.len();
        let bucket = self.bucket(request.bucket_name.as_deref());
        let key = object_key(file_name, request.prefix.as_deref());
        transfer::upload(
            s3,
            &bucket,
            &key,
            transfer::Source::File(&path),
            size,
            &request.extra_args,
            &self.options.transfer,
        )
        .await?;
        self.presign(s3, &bucket, &key).await
    }
    pub async fn upload_in_memory_object(
        &self,
        file_name: &str,
        file_data: &[u8],
        request: &UploadRequest,
    ) -> Result<String> {
        let Some(s3) = &self.s3 else {
            return self
                .save_to_local_fallback(file_name, None, Some(file_data), "local_upload")
                .await;
        };
        let bucket = self.bucket(request.bucket_name.as_deref());
        let key = object_key(file_name, request.prefix.as_deref());
        transfer::upload(
            s3,
            &bucket,
            &key,
            transfer::Source::Memory(file_data),
            file_data.len() as u64,
            &request.extra_args,
            &self.options.transfer,
        )
        .await?;
        self.presign(s3, &bucket, &key).await
    }
    /// The optional borrowed result slot replaces Python's shared list/index pair.
    pub async fn upload_image(
        &self,
        job_id: &str,
        image_location: impl AsRef<Path>,
        result_slot: Option<&mut Option<String>>,
        bucket_name: Option<&str>,
    ) -> Result<String> {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
        let image_name = format!("{:08x}", u32::from_be_bytes(bytes));
        let path = image_location.as_ref();
        let text = path
            .to_str()
            .ok_or_else(|| Error::InvalidInput("image path must be Unicode".into()))?;
        let extension = super::download::extension(text);
        let name = format!("{image_name}{extension}");
        let output = tokio::fs::read(self.root.join(path)).await?;
        let result = if let Some(s3) = &self.s3 {
            let bucket = self.bucket(bucket_name);
            let key = format!("{job_id}/{name}");
            s3.put_object()
                .bucket(&bucket)
                .key(&key)
                .body(ByteStream::from(output))
                .content_type(format!("image/{}", extension.trim_start_matches('.')))
                .checksum_algorithm(aws_sdk_s3::types::ChecksumAlgorithm::Crc32)
                .send()
                .await
                .map_err(|e| UploadError::sdk("put image", e))?;
            self.presign(s3, &bucket, &key).await?
        } else {
            self.save_to_local_fallback(&name, None, Some(&output), "simulated_uploaded")
                .await?
        };
        if let Some(slot) = result_slot {
            *slot = Some(result.clone());
        }
        Ok(result)
    }
    /// All files run concurrently; individual failures become None and order is retained.
    pub async fn files<P: AsRef<Path>>(
        &self,
        job_id: &str,
        file_list: &[P],
    ) -> Vec<Option<String>> {
        join_all(file_list.iter().map(|path| async move {
            match self.upload_image(job_id, path, None, None).await {
                Ok(url) => Some(url),
                Err(_) => {
                    tracing::warn!("image upload failed");
                    None
                }
            }
        }))
        .await
    }
    /// Sequential custom-bucket uploads preserve full input path keys and raw returned URLs.
    /// Unlike the other helpers, this uses the default region, not URL-derived region.
    pub async fn bucket_upload(
        &self,
        job_id: &str,
        file_list: &[String],
        credentials: &BucketCredentials,
    ) -> Result<Vec<String>> {
        let s3 = client(credentials, self.options.default_region())?;
        let bucket = credentials
            .bucket_name
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("bucketName is required".into()))?;
        let mut urls = Vec::with_capacity(file_list.len());
        for path in file_list {
            let body = ByteStream::from_path(self.root.join(path))
                .await
                .map_err(|e| UploadError::local("read upload", e))?;
            s3.put_object()
                .bucket(bucket)
                .key(format!("{job_id}/{path}"))
                .body(body)
                .checksum_algorithm(aws_sdk_s3::types::ChecksumAlgorithm::Crc32)
                .send()
                .await
                .map_err(|e| UploadError::sdk("put bucket file", e))?;
            urls.push(format!(
                "{}/{bucket}/{job_id}/{path}",
                credentials.endpoint_url
            ));
        }
        Ok(urls)
    }
}

fn object_key(name: &str, prefix: Option<&str>) -> String {
    prefix
        .filter(|p| !p.is_empty())
        .map_or_else(|| name.to_owned(), |p| format!("{p}/{name}"))
}

pub async fn save_to_local_fallback(
    file_name: &str,
    source_path: Option<&Path>,
    file_data: Option<&[u8]>,
    directory: &str,
) -> Result<String> {
    // Local fallback must remain callable even if the environment has an invalid S3 URL.
    Uploader::with_options(
        std::env::current_dir()?,
        UploadOptions {
            environment: BTreeMap::new(),
            ..UploadOptions::default()
        },
    )?
    .save_to_local_fallback(file_name, source_path, file_data, directory)
    .await
}
pub async fn upload_file_to_bucket(
    file_name: &str,
    file_location: impl AsRef<Path>,
    bucket_creds: Option<BucketCredentials>,
    request: &UploadRequest,
) -> Result<String> {
    Uploader::with_options(
        std::env::current_dir()?,
        UploadOptions {
            credentials: bucket_creds,
            ..UploadOptions::default()
        },
    )?
    .upload_file_to_bucket(file_name, file_location, request)
    .await
}
pub async fn upload_in_memory_object(
    file_name: &str,
    file_data: &[u8],
    bucket_creds: Option<BucketCredentials>,
    request: &UploadRequest,
) -> Result<String> {
    Uploader::with_options(
        std::env::current_dir()?,
        UploadOptions {
            credentials: bucket_creds,
            ..UploadOptions::default()
        },
    )?
    .upload_in_memory_object(file_name, file_data, request)
    .await
}
pub async fn upload_image(
    job_id: &str,
    image_location: impl AsRef<Path>,
    result_slot: Option<&mut Option<String>>,
    bucket_name: Option<&str>,
) -> Result<String> {
    Uploader::new(std::env::current_dir()?)?
        .upload_image(job_id, image_location, result_slot, bucket_name)
        .await
}
pub async fn files<P: AsRef<Path>>(job_id: &str, file_list: &[P]) -> Vec<Option<String>> {
    join_all(file_list.iter().map(|path| async move {
        match upload_image(job_id, path, None, None).await {
            Ok(url) => Some(url),
            Err(_) => {
                tracing::warn!("image upload failed");
                None
            }
        }
    }))
    .await
}
pub async fn bucket_upload(
    job_id: &str,
    file_list: &[String],
    credentials: &BucketCredentials,
) -> Result<Vec<String>> {
    let mut options = UploadOptions::default();
    options.environment.remove("BUCKET_ENDPOINT_URL");
    Uploader::with_options(std::env::current_dir()?, options)?
        .bucket_upload(job_id, file_list, credentials)
        .await
}

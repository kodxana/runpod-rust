use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Url;
use tokio::task::JoinHandle;
use tokio::{runtime::Runtime, task};
use uuid::Uuid;
use zip::ZipArchive;

#[async_trait]
trait Downloader {
    async fn download_file(&self, url: &str) -> Result<String>;
    async fn download_files_from_urls(&self, job_id: &str, urls: &[&str]) -> Vec<String>;
}

pub struct RpDownloader {
    client: reqwest::Client,
}

impl RpDownloader {
    pub fn new() -> Self {
        RpDownloader {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Downloader for RpDownloader {
    async fn download_file(&self, url: &str) -> Result<String> {
        let response = self.client.get(url).send().await?;

        let content_disposition = response
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();

        let file_name = if !content_disposition.is_empty() {
            content_disposition
        } else {
            Path::new(&Url::parse(url)?.path())
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or_default()
        };

        let output_file_path = format!("job_files/{}", Uuid::new_v4());
        let mut output_file = fs::File::create(&output_file_path)?;
        output_file.write_all(&response.bytes().await?)?;

        Ok(output_file_path)
    }

    async fn download_files_from_urls(&self, job_id: &str, urls: &[&str]) -> Vec<String> {
        let client = Arc::new(self.client.clone());
        let mut tasks: Vec<JoinHandle<Result<String>>> = Vec::new();

        for url in urls.iter() {
            let client = Arc::clone(&client);
            let url = url.to_string();
            tasks.push(task::spawn(async move {
                let response = client.get(&url).send().await?;
                let bytes = response.bytes().await?;

                let output_file_path = format!("job_files/{}", Uuid::new_v4());
                let mut output_file = fs::File::create(&output_file_path)?;
                output_file.write_all(&bytes)?;

                Ok(output_file_path)
            }));
        }

        let mut downloaded_files = Vec::new();
        for handle in tasks {
            match handle.await {
                Ok(Ok(file_path)) => downloaded_files.push(file_path),
                Ok(Err(err)) => eprintln!("Error downloading file: {}", err),
                Err(err) => eprintln!("Error executing task: {}", err),
            }
        }

        downloaded_files
    }
}

pub fn download_files_from_urls_sync(job_id: &str, urls: &[&str]) -> Vec<String> {
    let rt = Runtime::new().unwrap();
    let downloader = RpDownloader::new();
    rt.block_on(downloader.download_files_from_urls(job_id, urls))
}

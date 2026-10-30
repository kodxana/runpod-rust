use runpod::{
    Error, Value,
    utils::{
        download::{self, FileDownloadOptions, FileDownloader},
        ssrf::{AddressPolicy, DownloadOptions, SsrfError},
    },
};
use std::{
    collections::{BTreeMap, HashSet},
    num::NonZeroUsize,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path},
};

fn options() -> FileDownloadOptions {
    FileDownloadOptions {
        download: DownloadOptions {
            address_policy: AddressPolicy::AllowPrivate,
            ..DownloadOptions::from_values(&BTreeMap::new())
        },
        retry_base_delay: Duration::ZERO,
        ..FileDownloadOptions::default()
    }
}
fn client(root: &Path) -> FileDownloader {
    FileDownloader::new(root).unwrap().with_options(options())
}
fn fixtures() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/download.json")).unwrap()
}
fn platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "posix" }
}

#[test]
fn python_download_chunk_length_and_disposition_fixtures() {
    assert_eq!(fixtures().len(), 66);
    for case in fixtures() {
        let actual = match case["operation"].as_str().unwrap() {
            "chunk_size" => Value::from(download::calculate_chunk_size(i128::from(
                case["input"].as_i64().unwrap(),
            ))),
            "content_length" => {
                let mut headers = reqwest::header::HeaderMap::new();
                if let Some(length) = case["input"]["Content-Length"].as_str() {
                    headers.insert("Content-Length", length.parse().unwrap());
                }
                Value::String(download::parse_content_length(&headers).to_string())
            }
            "disposition" => serde_json::to_value(download::extract_disposition_params(
                case["input"].as_str().unwrap(),
            ))
            .unwrap(),
            _ => continue,
        };
        assert_eq!(actual, case["output"], "{case}");
    }
}

fn assert_uuid4(filename: &str) {
    let id = &filename[..36];
    assert_eq!(id.as_bytes()[14], b'4');
    assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    for (index, byte) in id.bytes().enumerate() {
        assert!(if [8, 13, 18, 23].contains(&index) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        });
    }
}

#[tokio::test]
async fn python_download_metadata_for_signed_urls_headers_and_filename_edges() {
    let server = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let downloader = client(root.path());
    let mut names = HashSet::new();
    for case in fixtures()
        .into_iter()
        .filter(|c| c["operation"] == "metadata" && c["platform"] == platform())
    {
        server.reset().await;
        let mut template =
            ResponseTemplate::new(200).set_body_bytes(if case["output"]["type"] == "zip" {
                include_bytes!("fixtures/download.zip").as_slice()
            } else {
                b"body"
            });
        if let Some(header) = case["input"]["disposition"].as_str() {
            template = template.insert_header("Content-Disposition", header);
        }
        Mock::given(method("GET"))
            .respond_with(template)
            .expect(2)
            .mount(&server)
            .await;
        let suffix = case["input"]["url"]
            .as_str()
            .unwrap()
            .strip_prefix("https://example.com")
            .unwrap();
        let url = format!("{}{suffix}", server.uri());
        let result = downloader.file(&url).await.unwrap();
        assert_eq!(result.file_type, case["output"]["type"], "{case}");
        assert_eq!(
            result.original_name, case["output"]["original_name"],
            "{case}"
        );
        assert_eq!(
            result.extracted_path.is_some(),
            case["output"]["extracted"].as_bool().unwrap()
        );
        assert!(result.file_path.is_absolute());
        assert!(result.file_path.starts_with(root.path().join("job_files")));
        let filename = result.file_path.file_name().unwrap().to_str().unwrap();
        assert_uuid4(filename);
        assert_eq!(
            &filename[36..],
            &case["output"]["file_name"].as_str().unwrap()[36..],
            "{case}"
        );
        assert!(names.insert(filename.to_owned()));
        let batch = downloader
            .download_files_from_urls("job-id", url)
            .await
            .unwrap();
        let file = batch[0].as_ref().unwrap();
        let filename = file.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            &filename[36..],
            &case["output"]["batch_name"].as_str().unwrap()[36..]
        );
        assert_uuid4(filename);
        assert!(names.insert(filename.to_owned()));
        assert!(file.starts_with(root.path().join("jobs/job-id/downloaded_files")));
        assert_eq!(
            std::fs::read(&result.file_path).unwrap(),
            std::fs::read(file).unwrap()
        );
        for request in server.received_requests().await.unwrap() {
            assert!(
                request.headers["User-Agent"]
                    .to_str()
                    .unwrap()
                    .starts_with("runpod-rust/")
            );
            assert!(!request.headers.contains_key("Authorization"));
            assert_eq!(
                request.url.query(),
                if suffix.contains("signature=abc") {
                    Some("signature=abc")
                } else {
                    None
                }
            );
        }
    }
}

fn collect_files(root: &Path, directory: &Path, files: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            collect_files(root, &entry.path(), files);
        } else {
            assert!(!entry.file_type().unwrap().is_symlink());
            let relative = entry
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let relative = if cfg!(windows) {
                relative.replace('\\', "/")
            } else {
                relative
            };
            files.insert(relative, std::fs::read_to_string(entry.path()).unwrap());
        }
    }
}

#[tokio::test]
async fn python_zip_contents_paths_compressions_duplicates_and_link_entries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(include_bytes!("fixtures/download.zip").as_slice()),
        )
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let result = client(root.path())
        .file(&format!("{}/archive.zip", server.uri()))
        .await
        .unwrap();
    let directory = result.extracted_path.unwrap();
    let mut files = BTreeMap::new();
    collect_files(&directory, &directory, &mut files);
    let expected = fixtures()
        .into_iter()
        .find(|case| case["operation"] == "zip" && case["platform"] == platform())
        .unwrap();
    assert_eq!(serde_json::to_value(files).unwrap(), expected["output"]);
    assert!(directory.join("empty-dir").is_dir());
    assert!(!root.path().join("outside").exists());
    assert_eq!(
        std::fs::read(&result.file_path).unwrap(),
        include_bytes!("fixtures/download.zip")
    );
}

#[tokio::test]
async fn batch_none_and_empty_inputs_create_directory_without_requests() {
    let root = tempfile::tempdir().unwrap();
    let downloader = client(root.path());
    assert_eq!(
        downloader
            .download_files_from_urls("nulls", vec![None::<String>, None])
            .await
            .unwrap(),
        [None, None]
    );
    assert!(
        downloader
            .download_files_from_urls("empty", Vec::<String>::new())
            .await
            .unwrap()
            .is_empty()
    );
    for job in ["nulls", "empty"] {
        assert!(
            root.path()
                .join(format!("jobs/{job}/downloaded_files"))
                .is_dir()
        );
    }
}

#[tokio::test]
async fn batch_retries_exactly_three_times_and_preserves_successes_and_failures() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    Mock::given(path("/eventual.txt"))
        .respond_with(move |_: &Request| {
            let attempt = observed.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(if attempt < 2 { 503 } else { 200 }).set_body_string("success")
        })
        .expect(3)
        .mount(&server)
        .await;
    Mock::given(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .expect(3)
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let results = client(root.path())
        .download_files_from_urls(
            "retry",
            vec![
                Some(format!("{}/eventual.txt", server.uri())),
                None,
                Some(format!("{}/missing", server.uri())),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(results[0].as_ref().unwrap()).unwrap(),
        "success"
    );
    assert_eq!(&results[1..], &[None, None]);
    assert_eq!(
        std::fs::read_dir(root.path().join("jobs/retry/downloaded_files"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn batch_policy_failure_is_not_retried_and_other_items_finish() {
    let server = MockServer::start().await;
    Mock::given(path("/unsafe"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "file:///blocked"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/good.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("kept")
                .set_delay(Duration::from_millis(30)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let result = client(root.path())
        .download_files_from_urls(
            "policy",
            vec![
                format!("{}/unsafe", server.uri()),
                format!("{}/good.txt", server.uri()),
            ],
        )
        .await;
    assert!(matches!(
        result,
        Err(Error::Ssrf(SsrfError::BlockedScheme(_)))
    ));
    let files: Vec<_> = std::fs::read_dir(root.path().join("jobs/policy/downloaded_files"))
        .unwrap()
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(
        std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap(),
        "kept"
    );
}

#[tokio::test]
async fn single_http_error_is_not_saved_extracted_or_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string("not a zip")
                .insert_header("Content-Disposition", "filename=error.zip"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    let error = client(root.path()).file(&server.uri()).await.unwrap_err();
    assert_eq!(error.status_code(), Some(404));
    assert_eq!(
        std::fs::read_dir(root.path().join("job_files"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn corrupt_zip_error_keeps_downloaded_file_and_partial_directory() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("corrupt"))
        .expect(1)
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(
        client(root.path())
            .file(&format!("{}/bad.zip", server.uri()))
            .await,
        Err(Error::Zip(_))
    ));
    let entries: Vec<_> = std::fs::read_dir(root.path().join("job_files"))
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries
            .iter()
            .filter(|e| e.file_type().unwrap().is_dir())
            .count(),
        1
    );
    let saved = entries
        .iter()
        .find(|e| e.file_type().unwrap().is_file())
        .unwrap();
    assert_eq!(std::fs::read(saved.path()).unwrap(), b"corrupt");
}

#[tokio::test]
async fn default_address_policy_blocks_private_urls_before_network() {
    let server = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut config = options();
    config.download.address_policy = AddressPolicy::PublicOnly;
    let result = FileDownloader::new(root.path())
        .unwrap()
        .with_options(config)
        .file(&server.uri())
        .await;
    assert!(matches!(
        result,
        Err(Error::Ssrf(SsrfError::NonPublic { .. }))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn directory_errors_propagate_before_http_and_batch_errors_follow_input_order() {
    let server = MockServer::start().await;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("jobs"), b"file in the way").unwrap();
    assert!(matches!(
        client(root.path())
            .download_files_from_urls("job", server.uri())
            .await,
        Err(Error::Io(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
    // Distinct policy failures keep input ordering, regardless of completion order.
    let other = tempfile::tempdir().unwrap();
    let result = client(other.path())
        .download_files_from_urls(
            "errors",
            vec!["ftp://first/file".to_owned(), "file:///second".to_owned()],
        )
        .await;
    assert!(matches!(result, Err(Error::Ssrf(SsrfError::BlockedScheme(s))) if s == "ftp"));
}

async fn read_headers(socket: &mut tokio::net::TcpStream) {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 16 * 1024);
        header.push(socket.read_u8().await.unwrap());
    }
}

#[tokio::test]
async fn partial_network_failure_retries_same_uuid_and_retains_partial_extension() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/input", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for response in [b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nContent-Disposition: filename=partial.txt\r\nConnection: close\r\n\r\nabc".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nContent-Disposition: filename=final.bin\r\nConnection: close\r\n\r\ndone".as_slice()] {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_headers(&mut socket).await;
            socket.write_all(response).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    let root = tempfile::tempdir().unwrap();
    let result = client(root.path())
        .download_files_from_urls("partial", url)
        .await
        .unwrap();
    let output = result[0].as_ref().unwrap();
    assert_eq!(std::fs::read(output).unwrap(), b"done");
    assert_eq!(std::fs::read(output.with_extension("txt")).unwrap(), b"abc");
    server.await.unwrap();
}

#[tokio::test]
async fn batch_starts_next_item_when_a_slot_frees_and_returns_original_order() {
    let release_first = Arc::new(Notify::new());
    let third_seen = Arc::new(Notify::new());
    let mut urls = Vec::new();
    let mut servers = Vec::new();
    for index in 0..3 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        urls.push(format!(
            "http://{}/item{index}",
            listener.local_addr().unwrap()
        ));
        let release = release_first.clone();
        let seen = third_seen.clone();
        servers.push(tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_headers(&mut socket).await;
            if index == 0 {
                release.notified().await;
            }
            if index == 2 {
                seen.notify_one();
            }
            let response =
                format!("HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{index}");
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }));
    }
    let root = tempfile::tempdir().unwrap();
    let downloader = FileDownloader::new(root.path())
        .unwrap()
        .with_options(FileDownloadOptions {
            concurrency: NonZeroUsize::new(2).unwrap(),
            ..options()
        });
    let task = tokio::spawn(async move {
        downloader
            .download_files_from_urls("concurrent", urls)
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), third_seen.notified())
        .await
        .expect("completed slot did not admit next item");
    release_first.notify_one();
    for (index, file) in task.await.unwrap().unwrap().iter().enumerate() {
        assert_eq!(
            std::fs::read_to_string(file.as_ref().unwrap()).unwrap(),
            index.to_string()
        );
    }
    for server in servers {
        server.await.unwrap();
    }
}

#[tokio::test]
async fn batch_size_rejection_flushes_partial_file_and_is_not_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/input.txt", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_headers(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n").await.unwrap();
        let mut buffer = [0; 1];
        let read = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut buffer))
            .await
            .expect("capped body was not closed");
        assert!(matches!(read, Ok(0) | Err(_)));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "policy rejection retried"
        );
    });
    let root = tempfile::tempdir().unwrap();
    let mut config = options();
    config.download.max_bytes = 5;
    let result = FileDownloader::new(root.path())
        .unwrap()
        .with_options(config)
        .download_files_from_urls("capped", url)
        .await;
    assert!(matches!(
        result,
        Err(Error::Ssrf(SsrfError::SizeLimit { .. }))
    ));
    let files: Vec<_> = std::fs::read_dir(root.path().join("jobs/capped/downloaded_files"))
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    assert_eq!(files.len(), 1);
    let partial = std::fs::read(files[0].path()).unwrap();
    assert!(partial.len() <= 5);
    assert!(b"abcdef".starts_with(&partial));
    server.await.unwrap();
}

#[tokio::test]
async fn canceling_batch_releases_active_requests_and_does_not_start_queued_urls() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first = format!("http://{}/first", listener.local_addr().unwrap());
    let ready = Arc::new(Notify::new());
    let arrived = ready.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_headers(&mut socket).await;
        arrived.notify_one();
        let mut buffer = [0; 1];
        let read = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut buffer))
            .await
            .expect("canceled request leaked socket");
        assert!(matches!(read, Ok(0) | Err(_)));
    });
    let queued = MockServer::start().await;
    let second = queued.uri();
    let root = tempfile::tempdir().unwrap();
    let downloader = FileDownloader::new(root.path())
        .unwrap()
        .with_options(FileDownloadOptions {
            concurrency: NonZeroUsize::MIN,
            ..options()
        });
    let task = tokio::spawn(async move {
        downloader
            .download_files_from_urls("cancel", vec![first, second])
            .await
    });
    ready.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    server.await.unwrap();
    assert!(queued.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn corrupt_zip_crc_is_detected_after_download_without_retry() {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(include_bytes!(
        "fixtures/download.zip"
    )))
    .unwrap();
    let offset = archive.by_name("stored.txt").unwrap().data_start().unwrap();
    let mut bytes = include_bytes!("fixtures/download.zip").to_vec();
    bytes[usize::try_from(offset).unwrap()] ^= 1;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .expect(1)
        .mount(&server)
        .await;
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(
        client(root.path())
            .file(&format!("{}/crc.zip", server.uri()))
            .await,
        Err(Error::Io(_)) | Err(Error::Zip(_))
    ));
}

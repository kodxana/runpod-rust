use runpod::{
    Error, Value,
    utils::upload::{
        BucketCredentials, UploadOptions, UploadRequest, Uploader, extract_region_from_url,
        multipart_chunk_size,
    },
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path, query_param},
};

fn options() -> UploadOptions {
    UploadOptions {
        environment: BTreeMap::new(),
        now: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1705320000)),
        ..UploadOptions::default()
    }
}
fn creds(endpoint: &str) -> BucketCredentials {
    BucketCredentials {
        endpoint_url: endpoint.into(),
        access_id: "TEST_ACCESS".into(),
        access_secret: "TEST_SECRET".into(),
        bucket_name: Some("custom".into()),
    }
}
fn request() -> UploadRequest {
    UploadRequest {
        bucket_name: Some("bucket".into()),
        ..UploadRequest::default()
    }
}
fn uploader(root: &std::path::Path, server: &MockServer) -> Uploader {
    Uploader::with_options(
        root,
        UploadOptions {
            credentials: Some(creds(&server.uri())),
            ..options()
        },
    )
    .unwrap()
}
fn header<'a>(request: &'a Request, key: &str) -> &'a str {
    request.headers.get(key).unwrap().to_str().unwrap()
}
fn fixtures() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/upload.json")).unwrap()
}
fn decoded_body(request: &Request) -> Vec<u8> {
    if request
        .headers
        .get("content-encoding")
        .is_some_and(|v| v.as_bytes() == b"aws-chunked")
    {
        let mut body = request.body.as_slice();
        let mut output = Vec::new();
        loop {
            let end = body.windows(2).position(|b| b == b"\r\n").unwrap();
            let line = std::str::from_utf8(&body[..end])
                .unwrap()
                .split(';')
                .next()
                .unwrap();
            let size = usize::from_str_radix(line, 16).unwrap();
            if size == 0 {
                break;
            }
            body = &body[end + 2..];
            output.extend_from_slice(&body[..size]);
            body = &body[size + 2..];
        }
        output
    } else {
        request.body.clone()
    }
}

#[test]
fn python_region_and_chunk_fixtures() {
    for case in fixtures() {
        match case["operation"].as_str().unwrap() {
            "region" => assert_eq!(
                json!(extract_region_from_url(case["input"].as_str().unwrap())),
                case["output"],
                "{case}"
            ),
            "chunk" => assert_eq!(
                multipart_chunk_size(case["input"].as_u64().unwrap(), 25600).unwrap(),
                case["output"].as_u64().unwrap()
            ),
            _ => {}
        }
    }
    assert!(multipart_chunk_size(1, 0).is_err());
}

#[tokio::test]
async fn fallback_copy_bytes_empty_precedence_and_original_paths() {
    let temp = tempfile::tempdir().unwrap();
    let u = Uploader::with_options(temp.path(), options()).unwrap();
    tokio::fs::write(temp.path().join("source"), b"source bytes")
        .await
        .unwrap();
    let mut r = request();
    r.prefix = Some("ignored".into());
    r.extra_args
        .insert("invalid ignored locally".into(), json!(7));
    assert_eq!(
        u.upload_file_to_bucket("copy", "source", &r).await.unwrap(),
        "local_upload/copy"
    );
    assert_eq!(
        tokio::fs::read(temp.path().join("local_upload/copy"))
            .await
            .unwrap(),
        b"source bytes"
    );
    assert_eq!(
        u.upload_in_memory_object("empty", b"", &r).await.unwrap(),
        "local_upload/empty"
    );
    assert_eq!(
        u.save_to_local_fallback(
            "both",
            Some(std::path::Path::new("source")),
            Some(b"ignored"),
            "local_upload"
        )
        .await
        .unwrap(),
        "local_upload/both"
    );
    assert_eq!(
        tokio::fs::read(temp.path().join("local_upload/both"))
            .await
            .unwrap(),
        b"source bytes"
    );
    let location = u
        .save_to_local_fallback("/leading", None, Some(b"inside"), "local_upload")
        .await
        .unwrap();
    assert_eq!(location, "local_upload//leading");
    assert!(temp.path().join("local_upload/leading").is_file());
    assert!(
        u.save_to_local_fallback("nested/a", None, Some(b"x"), "local_upload")
            .await
            .is_err()
    );
    let error = u
        .save_to_local_fallback("a", None, None, "created_first")
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Either source_path or file_data must be provided"
    );
    assert!(temp.path().join("created_first").is_dir());
    // Upstream paths are trusted and ../ is retained, rather than silently sanitized.
    assert_eq!(
        u.save_to_local_fallback("../parent", None, Some(b"parent"), "local_upload")
            .await
            .unwrap(),
        "local_upload/../parent"
    );
    assert!(temp.path().join("parent").is_file());
}

#[tokio::test]
async fn fallback_refuses_same_file_and_hardlink_without_truncating() {
    let temp = tempfile::tempdir().unwrap();
    let u = Uploader::with_options(temp.path(), options()).unwrap();
    u.upload_in_memory_object("a", b"keep", &request())
        .await
        .unwrap();
    for target in ["a", "link"] {
        if target == "link" {
            std::fs::hard_link(
                temp.path().join("local_upload/a"),
                temp.path().join("local_upload/link"),
            )
            .unwrap();
        }
        assert!(
            u.save_to_local_fallback(
                target,
                Some(std::path::Path::new("local_upload/a")),
                None,
                "local_upload"
            )
            .await
            .is_err()
        );
        assert_eq!(
            std::fs::read(temp.path().join("local_upload/a")).unwrap(),
            b"keep"
        );
    }
}

#[tokio::test]
async fn image_fallback_result_slot_and_batch_failures() {
    let temp = tempfile::tempdir().unwrap();
    let u = Uploader::with_options(temp.path(), options()).unwrap();
    std::fs::write(temp.path().join("x.JPG"), b"image").unwrap();
    let mut slot = None;
    let result = u
        .upload_image("ignored", "x.JPG", Some(&mut slot), None)
        .await
        .unwrap();
    assert_eq!(slot.as_ref(), Some(&result));
    assert!(result.ends_with(".JPG"));
    assert_eq!(std::fs::read(temp.path().join(&result)).unwrap(), b"image");
    assert_eq!(result.trim_start_matches("simulated_uploaded/").len(), 12);
    let batch = u.files("job", &["x.JPG", "missing", "x.JPG"]).await;
    assert!(batch[0].is_some());
    assert!(batch[1].is_none());
    assert!(batch[2].is_some());
    assert_ne!(batch[0], batch[2]);
    assert!(u.files::<&str>("job", &[]).await.is_empty());
    let previous = slot.clone();
    assert!(
        u.upload_image("job", "missing", Some(&mut slot), None)
            .await
            .is_err()
    );
    assert_eq!(slot, previous);
}

#[tokio::test]
async fn incomplete_credentials_fallback_and_debug_redaction() {
    let temp = tempfile::tempdir().unwrap();
    let mut c = creds("invalid URL");
    c.access_id.clear();
    let u = Uploader::with_options(
        temp.path(),
        UploadOptions {
            credentials: Some(c),
            ..options()
        },
    )
    .unwrap();
    assert!(u.s3_client().is_none());
    let mut opts = options();
    opts.environment
        .insert("BUCKET_SECRET_ACCESS_KEY".into(), "super-secret".into());
    assert!(!format!("{opts:?}").contains("super-secret"));
    let mut r = request();
    r.extra_args
        .insert("SSECustomerKey".into(), json!("secret"));
    assert!(!format!("{r:?}").contains("\"secret\""));
    assert!(
        Uploader::with_options(
            temp.path(),
            UploadOptions {
                credentials: Some(creds("invalid URL")),
                ..options()
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn python_memory_requests_presigning_and_reusable_client() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).insert_header("etag", "\"done\""))
        .mount(&server)
        .await;
    let cases: Vec<_> = fixtures()
        .into_iter()
        .filter(|v| v["operation"] == "memory")
        .collect();
    for case in &cases {
        let r = UploadRequest {
            bucket_name: case["input"]["bucket"].as_str().map(str::to_owned),
            prefix: case["input"]["prefix"].as_str().map(str::to_owned),
            ..UploadRequest::default()
        };
        let result = u
            .upload_in_memory_object("a b.txt", b"hello", &r)
            .await
            .unwrap();
        let url = reqwest::Url::parse(&result).unwrap();
        let query: BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["X-Amz-Expires"], "604800");
        assert_eq!(query["X-Amz-Date"], "20240115T120000Z");
        assert_eq!(query["X-Amz-Algorithm"], "AWS4-HMAC-SHA256");
        assert_eq!(
            query["X-Amz-Credential"],
            "TEST_ACCESS/20240115/us-east-1/s3/aws4_request"
        );
        assert_eq!(query["X-Amz-Signature"].len(), 64);
        assert!(!result.contains("TEST_SECRET"));
    }
    for (sent, case) in server.received_requests().await.unwrap().iter().zip(cases) {
        let expected = format!(
            "/{}/{}",
            case["output"]["bucket"].as_str().unwrap(),
            case["output"]["key"].as_str().unwrap().replace(' ', "%20")
        );
        assert_eq!(sent.url.path(), expected);
        assert_eq!(decoded_body(sent), b"hello");
        assert!(
            header(sent, "authorization").starts_with("AWS4-HMAC-SHA256 Credential=TEST_ACCESS/")
        );
        assert_eq!(header(sent, "x-amz-sdk-checksum-algorithm"), "CRC32");
    }
}

#[tokio::test]
async fn python_image_content_type_and_key_fixtures() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let cases: Vec<_> = fixtures()
        .into_iter()
        .filter(|v| v["operation"] == "image")
        .collect();
    for case in &cases {
        let file = case["input"].as_str().unwrap();
        std::fs::write(temp.path().join(file), b"image bytes").unwrap();
        u.upload_image("job", file, None, Some("images"))
            .await
            .unwrap();
    }
    for (sent, case) in server.received_requests().await.unwrap().iter().zip(cases) {
        assert_eq!(
            header(sent, "content-type"),
            case["output"]["content_type"].as_str().unwrap()
        );
        let suffix = case["output"]["key"]
            .as_str()
            .unwrap()
            .trim_start_matches("job/11111111");
        assert!(sent.url.path().starts_with("/images/job/"));
        assert!(sent.url.path().ends_with(suffix));
        assert_eq!(decoded_body(sent), b"image bytes");
    }
}

#[tokio::test]
async fn single_file_extra_args_and_sse_customer_key_encoding() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    std::fs::write(temp.path().join("input"), b"file body").unwrap();
    Mock::given(method("PUT"))
        .and(path("/bucket/name"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut r = request();
    r.extra_args = serde_json::from_value(json!({"ACL":"private", "ContentType":"text/plain", "Metadata":{"test":"yes"},
        "CacheControl":"max-age=60", "SSECustomerAlgorithm":"AES256", "SSECustomerKey":"my secret key", "Tagging":"a=b", "StorageClass":"STANDARD_IA",
        "ChecksumType":"ignored for single put", "MpuObjectSize":999 })).unwrap();
    u.upload_file_to_bucket("name", "input", &r).await.unwrap();
    let sent = server.received_requests().await.unwrap().pop().unwrap();
    assert_eq!(decoded_body(&sent), b"file body");
    assert_eq!(header(&sent, "content-type"), "text/plain");
    assert_eq!(header(&sent, "x-amz-meta-test"), "yes");
    assert_eq!(header(&sent, "x-amz-acl"), "private");
    assert_eq!(
        header(&sent, "x-amz-server-side-encryption-customer-key"),
        "bXkgc2VjcmV0IGtleQ=="
    );
    assert!(!header(&sent, "x-amz-server-side-encryption-customer-key-md5").is_empty());
    assert_eq!(header(&sent, "x-amz-storage-class"), "STANDARD_IA");
    assert!(!sent.headers.contains_key("x-amz-checksum-type"));
}

async fn multipart_server(server: &MockServer) {
    Mock::given(method("POST")).and(query_param("uploads", "")).respond_with(ResponseTemplate::new(200).set_body_string(
        "<InitiateMultipartUploadResult><UploadId>upload-test</UploadId></InitiateMultipartUploadResult>")).mount(server).await;
    Mock::given(method("POST"))
        .and(query_param("uploadId", "upload-test"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<CompleteMultipartUploadResult><ETag>done</ETag></CompleteMultipartUploadResult>",
        ))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(query_param("uploadId", "upload-test"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
}

#[tokio::test]
async fn exact_multipart_threshold_and_ordered_completion() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    multipart_server(&server).await;
    Mock::given(method("PUT"))
        .respond_with(|r: &Request| {
            let part = r
                .url
                .query_pairs()
                .find(|(k, _)| k == "partNumber")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            ResponseTemplate::new(200)
                .insert_header("etag", format!("\"part-{part}\""))
                .set_delay(if part == "1" {
                    Duration::from_millis(100)
                } else {
                    Duration::ZERO
                })
        })
        .mount(&server)
        .await;
    u.upload_in_memory_object("small", &vec![1; 25600 - 1], &request())
        .await
        .unwrap();
    u.upload_in_memory_object("boundary", &vec![2; 25600], &request())
        .await
        .unwrap();
    let mut opts = options();
    opts.credentials = Some(creds(&server.uri()));
    opts.transfer.max_concurrency = NonZeroUsize::new(2).unwrap();
    let u = Uploader::with_options(temp.path(), opts).unwrap();
    u.upload_in_memory_object("large", &vec![3; 10 * 1024 * 1024 + 1], &request())
        .await
        .unwrap();
    let sent = server.received_requests().await.unwrap();
    assert_eq!(
        sent.iter()
            .filter(|r| r.url.path() == "/bucket/small")
            .count(),
        1
    );
    let parts: Vec<_> = sent
        .iter()
        .filter(|r| r.method == "PUT" && r.url.path() == "/bucket/large")
        .collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(decoded_body(parts[0]).len(), 5 * 1024 * 1024);
    assert_eq!(decoded_body(parts[1]).len(), 5 * 1024 * 1024);
    assert_eq!(decoded_body(parts[2]), vec![3]);
    let completed = sent
        .iter()
        .find(|r| {
            r.method == "POST"
                && r.url.path() == "/bucket/large"
                && r.url.query().unwrap().contains("uploadId")
        })
        .unwrap();
    let xml = std::str::from_utf8(&completed.body).unwrap();
    assert!(xml.find("part-1").unwrap() < xml.find("part-2").unwrap());
    assert!(xml.find("part-2").unwrap() < xml.find("part-3").unwrap());
    assert!(!sent.iter().any(|r| r.method == "DELETE"));
}

#[tokio::test]
async fn multipart_file_metadata_full_checksum_and_encryption_routing() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    let data = vec![7; 25600];
    std::fs::write(temp.path().join("input"), &data).unwrap();
    multipart_server(&server).await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "part")
                .insert_header("x-amz-checksum-crc32", "returned-checksum"),
        )
        .mount(&server)
        .await;
    let mut r = request();
    r.extra_args = serde_json::from_value(json!({"ContentType":"application/custom", "Metadata":{"hello":"world"},
        "ChecksumCRC32":"whole-checksum", "MpuObjectSize":25600, "ExpectedBucketOwner":"123", "RequestPayer":"requester",
        "SSECustomerAlgorithm":"AES256", "SSECustomerKey":"key" })).unwrap();
    u.upload_file_to_bucket("name", "input", &r).await.unwrap();
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(header(&sent[0], "content-type"), "application/custom");
    assert_eq!(header(&sent[0], "x-amz-meta-hello"), "world");
    assert!(!sent[0].headers.contains_key("x-amz-checksum-crc32"));
    assert_eq!(header(&sent[0], "x-amz-checksum-type"), "FULL_OBJECT");
    assert_eq!(decoded_body(&sent[1]), data);
    assert!(!sent[1].headers.contains_key("x-amz-meta-hello"));
    assert_eq!(header(&sent[2], "x-amz-checksum-crc32"), "whole-checksum");
    assert_eq!(header(&sent[2], "x-amz-mp-object-size"), "25600");
    assert!(String::from_utf8_lossy(&sent[2].body).contains("returned-checksum"));
    for sent in &sent {
        assert_eq!(header(sent, "x-amz-expected-bucket-owner"), "123");
        assert_eq!(
            header(sent, "x-amz-server-side-encryption-customer-key"),
            "a2V5"
        );
    }
}

#[tokio::test]
async fn retry_count_and_nonretryable_failures_do_not_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    Mock::given(method("PUT"))
        .and(path("/bucket/retry"))
        .respond_with(
            ResponseTemplate::new(503).set_body_string("<Error><Code>SlowDown</Code></Error>"),
        )
        .expect(4)
        .mount(&server)
        .await;
    let err = u
        .upload_in_memory_object("retry", b"x", &request())
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), Some(503));
    Mock::given(method("PUT"))
        .and(path("/bucket/denied"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string("<Error><Code>AccessDenied</Code></Error>"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let err = u
        .upload_in_memory_object("denied", b"x", &request())
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), Some(403));
    assert!(!temp.path().join("local_upload").exists());
}

#[tokio::test]
async fn failed_part_awaits_abort_and_preserves_original_error() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    multipart_server(&server).await;
    Mock::given(method("PUT"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string("<Error><Code>AccessDenied</Code></Error>"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let err = u
        .upload_in_memory_object("name", &vec![1; 25600], &request())
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), Some(403));
    assert!(matches!(err, Error::Upload(_)));
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent.last().unwrap().method, "DELETE");
    assert_eq!(sent.len(), 3);
}

#[tokio::test]
async fn canceled_multipart_aborts_without_completion() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    multipart_server(&server).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let aborted = Arc::new(tokio::sync::Notify::new());
    let seen = started.clone();
    Mock::given(method("PUT"))
        .respond_with(move |_: &Request| {
            seen.notify_one();
            ResponseTemplate::new(200)
                .insert_header("etag", "part")
                .set_delay(Duration::from_secs(30))
        })
        .mount(&server)
        .await;
    let seen = aborted.clone();
    Mock::given(method("DELETE"))
        .respond_with(move |_: &Request| {
            seen.notify_one();
            ResponseTemplate::new(204)
        })
        .with_priority(1)
        .mount(&server)
        .await;
    let task = tokio::spawn(async move {
        u.upload_in_memory_object("name", &vec![1; 25600], &request())
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(5), aborted.notified())
        .await
        .unwrap();
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        1
    );
}

#[tokio::test]
async fn invalid_extra_args_rejected_before_network_and_missing_etag_aborts() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    let mut r = request();
    r.extra_args.insert("unknown".into(), json!("x"));
    assert!(matches!(
        u.upload_in_memory_object("name", b"x", &r).await,
        Err(Error::InvalidInput(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
    multipart_server(&server).await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    assert!(matches!(
        u.upload_in_memory_object("name", &vec![0; 25600], &request())
            .await,
        Err(Error::InvalidResponse(_))
    ));
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .last()
            .unwrap()
            .method,
        "DELETE"
    );
}

#[tokio::test]
async fn bucket_upload_retains_paths_raw_urls_and_sequential_failure() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    std::fs::create_dir(temp.path().join("nested")).unwrap();
    std::fs::write(temp.path().join("nested/a b"), b"body").unwrap();
    let c = creds(&format!("{}/prefix/", server.uri()));
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let result = u
        .bucket_upload("job", &["nested/a b".into()], &c)
        .await
        .unwrap();
    assert_eq!(
        result,
        vec![format!("{}/prefix//custom/job/nested/a b", server.uri())]
    );
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent[0].url.path(), "/prefix/custom/job/nested/a%20b");
    assert_eq!(decoded_body(&sent[0]), b"body");
    assert!(
        u.bucket_upload("job", &["missing".into(), "nested/a b".into()], &c)
            .await
            .is_err()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn all_python_extra_args_wire_headers() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    std::fs::write(temp.path().join("source"), b"body").unwrap();
    let case = fixtures()
        .into_iter()
        .find(|v| v["operation"] == "extra_args")
        .unwrap();
    let r = UploadRequest {
        extra_args: serde_json::from_value(case["input"].clone()).unwrap(),
        ..request()
    };
    u.upload_file_to_bucket("name", "source", &r).await.unwrap();
    let sent = server.received_requests().await.unwrap().pop().unwrap();
    for (name, value) in case["output"].as_object().unwrap() {
        assert_eq!(header(&sent, name), value.as_str().unwrap(), "{name}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn local_copy_preserves_existing_destination_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let u = Uploader::with_options(temp.path(), options()).unwrap();
    std::fs::write(temp.path().join("source"), b"new").unwrap();
    std::fs::set_permissions(
        temp.path().join("source"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    u.upload_in_memory_object("dest", b"old", &request())
        .await
        .unwrap();
    let dest = temp.path().join("local_upload/dest");
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o640)).unwrap();
    u.upload_file_to_bucket("dest", "source", &request())
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(dest).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[tokio::test]
async fn captured_environment_and_explicit_credentials_region_selection() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let mut opts = options();
    opts.environment = BTreeMap::from([
        ("BUCKET_ENDPOINT_URL".into(), server.uri()),
        ("BUCKET_ACCESS_KEY_ID".into(), "ENV_ACCESS".into()),
        ("BUCKET_SECRET_ACCESS_KEY".into(), "ENV_SECRET".into()),
        ("AWS_DEFAULT_REGION".into(), "eu-west-1".into()),
    ]);
    let u = Uploader::with_options(temp.path(), opts.clone()).unwrap();
    assert_eq!(
        u.s3_client().unwrap().config().region().unwrap().as_ref(),
        "eu-west-1"
    );
    opts.credentials = Some(creds("https://bucket.s3.eu-central-003.backblazeb2.com"));
    let u = Uploader::with_options(temp.path(), opts).unwrap();
    assert_eq!(
        u.s3_client().unwrap().config().region().unwrap().as_ref(),
        "eu-central-003"
    );
    assert_eq!(
        u.s3_client()
            .unwrap()
            .config()
            .retry_config()
            .unwrap()
            .max_attempts(),
        4
    );
    let defaults = runpod::utils::upload::TransferConfig::default();
    assert_eq!(defaults.multipart_threshold, 25600);
    assert_eq!(defaults.multipart_chunksize, 25600);
}

#[tokio::test]
async fn successful_retry_replays_file_body() {
    let temp = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let u = uploader(temp.path(), &server);
    std::fs::write(temp.path().join("source"), b"retry this body").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("PUT"))
        .respond_with(move |_: &Request| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503).set_body_string("<Error><Code>SlowDown</Code></Error>")
            } else {
                ResponseTemplate::new(200)
            }
        })
        .expect(2)
        .mount(&server)
        .await;
    u.upload_file_to_bucket("name", "source", &request())
        .await
        .unwrap();
    for sent in server.received_requests().await.unwrap() {
        assert_eq!(decoded_body(&sent), b"retry this body");
    }
}

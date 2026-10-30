use futures_util::{StreamExt, future::BoxFuture};
use runpod::{
    Error, Value,
    utils::ssrf::{
        self, AddressPolicy, DownloadOptions, ProxySettings, Resolver, SafeDownloader, SsrfError,
    },
};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{method, path},
};

fn options() -> DownloadOptions {
    DownloadOptions::from_values(&BTreeMap::new())
}
fn local_options() -> DownloadOptions {
    DownloadOptions {
        address_policy: AddressPolicy::AllowPrivate,
        timeout: Duration::from_secs(2),
        ..options()
    }
}
#[derive(Default)]
struct Dns {
    answers: Mutex<VecDeque<io::Result<Vec<IpAddr>>>>,
    calls: Mutex<Vec<(String, u16)>>,
}
impl Dns {
    fn new(answers: &[&[&str]]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(
                answers
                    .iter()
                    .map(|list| Ok(list.iter().map(|s| s.parse().unwrap()).collect()))
                    .collect(),
            ),
            ..Self::default()
        })
    }
}
impl Resolver for Dns {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((host.to_owned(), port));
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra DNS lookup")
        })
    }
}
fn hostname_url(server: &MockServer, resource: &str) -> String {
    format!("http://download.test:{}{resource}", server.address().port())
}

#[test]
fn python_ssrf_classification_configuration_and_host_fixtures() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/ssrf.json")).unwrap();
    assert_eq!(cases.len(), 498);
    for case in cases {
        let actual = match case["operation"].as_str().unwrap() {
            "address" => Value::Bool(ssrf::is_safe_address(case["input"].as_str().unwrap())),
            "host" => {
                Value::String(ssrf::host_header_for(case["input"].as_str().unwrap()).unwrap())
            }
            "max_bytes" => Value::from(
                DownloadOptions::from_values(
                    &serde_json::from_value(case["input"].clone()).unwrap(),
                )
                .max_bytes,
            ),
            "protection" => Value::Bool(
                DownloadOptions::from_values(&BTreeMap::from([(
                    "RUNPOD_ALLOW_PRIVATE_DOWNLOAD_URLS".into(),
                    case["input"].as_str().unwrap().into(),
                )]))
                .address_policy
                    == AddressPolicy::PublicOnly,
            ),
            operation => panic!("unexpected operation {operation}"),
        };
        assert_eq!(actual, case["output"], "{case}");
    }
}

#[tokio::test]
async fn validates_every_dns_answer_deduplicates_and_preserves_order() {
    let dns = Dns::new(&[
        &["8.8.8.8", "1.1.1.1", "8.8.8.8"],
        &["8.8.8.8", "10.0.0.5"],
        &[],
    ]);
    let client = SafeDownloader::with_resolver(options(), dns.clone());
    let addresses = client
        .resolve_and_validate("download.test", 443)
        .await
        .unwrap();
    assert_eq!(
        addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["8.8.8.8", "1.1.1.1"]
    );
    assert!(matches!(
        client.safe_get("https://download.test/x").await,
        Err(Error::Ssrf(SsrfError::NonPublic { .. }))
    ));
    assert!(matches!(
        client.safe_get("https://download.test/x").await,
        Err(Error::Ssrf(SsrfError::NoAddresses(_)))
    ));
    dns.answers.lock().unwrap().push_back(Err(io::Error::new(
        io::ErrorKind::NotFound,
        "fixture DNS failure",
    )));
    let error = client
        .safe_get("https://download.test/x")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Ssrf(SsrfError::Resolve { .. })));
    assert!(error.to_string().contains("fixture DNS failure"));
}

#[tokio::test]
async fn blocked_schemes_ambiguous_hosts_and_literal_forms_never_reach_dns() {
    let dns = Dns::new(&[]);
    let client = SafeDownloader::with_resolver(options(), dns.clone());
    for url in [
        "file:///etc/passwd",
        "gopher://h/x",
        "ftp://h/f",
        "data:text/plain,x",
        "http:///x",
        "http://",
        "https://x:99999",
        "http://x\\y/a",
        "http://bad\nhost/",
    ] {
        assert!(
            matches!(client.safe_get(url).await, Err(Error::Ssrf(_))),
            "{url}"
        );
    }
    for url in [
        "http://127.0.0.1/",
        "http://169.254.169.254/latest/meta-data/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://127.1/",
        "http://0177.0.0.1/",
        "http://[::ffff:127.0.0.1]/",
        "http://[::1]/",
    ] {
        assert!(
            matches!(
                client.safe_get(url).await,
                Err(Error::Ssrf(SsrfError::NonPublic { .. }))
            ),
            "{url}"
        );
    }
    assert!(dns.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dial_is_pinned_preserves_authority_and_returns_http_statuses() {
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(method("GET"))
        .and(path("/file"))
        .respond_with(move |request: &Request| {
            assert_eq!(request.headers["Host"], format!("download.test:{port}"));
            assert_eq!(request.headers.get_all("Host").iter().count(), 1);
            assert_eq!(request.headers["X-Download"], "value");
            assert!(!request.headers.contains_key("Authorization"));
            ResponseTemplate::new(503).set_body_string("body")
        })
        .expect(1)
        .mount(&server)
        .await;
    let mut options = local_options();
    options
        .headers
        .insert("Host", "attacker.test".parse().unwrap());
    options
        .headers
        .insert("X-Download", "value".parse().unwrap());
    let dns = Dns::new(&[&["127.0.0.1", "127.0.0.2"]]);
    let response = SafeDownloader::with_resolver(options, dns.clone())
        .safe_get(&hostname_url(&server, "/file"))
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.remote_addr().unwrap(), *server.address());
    assert_eq!(response.bytes().await.unwrap(), b"body");
    assert_eq!(dns.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn connection_failure_falls_back_but_http_error_does_not() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let dns = Dns::new(&[&["127.0.0.2", "127.0.0.1", "127.0.0.3"]]);
    let response = SafeDownloader::with_resolver(local_options(), dns)
        .safe_get(&hostname_url(&server, "/missing"))
        .await
        .unwrap();
    let error = response.error_for_status().unwrap_err();
    assert_eq!(error.status_code(), Some(404));
    assert!(matches!(error, Error::Http(_)));
}

#[tokio::test]
async fn all_connection_failures_return_transport_error() {
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let client =
        SafeDownloader::with_resolver(local_options(), Dns::new(&[&["127.0.0.2", "127.0.0.1"]]));
    let result = client
        .safe_get(&format!("http://download.test:{port}/"))
        .await;
    assert!(matches!(result, Err(Error::Http(error)) if error.is_connect() || error.is_timeout()));
}

#[tokio::test]
async fn relative_redirects_resolve_again_and_cookies_are_not_carried() {
    let server = MockServer::start().await;
    Mock::given(path("/start"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "next")
                .insert_header("Set-Cookie", "secret=value"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/next"))
        .respond_with(|request: &Request| {
            assert!(!request.headers.contains_key("Cookie"));
            assert!(!request.headers.contains_key("Referer"));
            ResponseTemplate::new(200).set_body_string("ok")
        })
        .expect(1)
        .mount(&server)
        .await;
    let dns = Dns::new(&[&["127.0.0.1"], &["127.0.0.1"]]);
    let response = SafeDownloader::with_resolver(local_options(), dns.clone())
        .safe_get(&hostname_url(&server, "/start"))
        .await
        .unwrap();
    assert_eq!(response.url().path(), "/next");
    assert_eq!(response.bytes().await.unwrap(), b"ok");
    assert_eq!(dns.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn redirect_scheme_revalidation_missing_location_and_limit() {
    let server = MockServer::start().await;
    Mock::given(path("/bad"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "file:///etc/passwd"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/missing"))
        .respond_with(ResponseTemplate::new(307))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/loop"))
        .respond_with(ResponseTemplate::new(308).insert_header("Location", "/loop"))
        .expect(3)
        .mount(&server)
        .await;
    let client = SafeDownloader::new(DownloadOptions {
        max_redirects: 2,
        ..local_options()
    });
    assert!(matches!(
        client.safe_get(&format!("{}/bad", server.uri())).await,
        Err(Error::Ssrf(SsrfError::BlockedScheme(_)))
    ));
    assert!(matches!(
        client.safe_get(&format!("{}/missing", server.uri())).await,
        Err(Error::Ssrf(SsrfError::MissingLocation))
    ));
    assert!(matches!(
        client.safe_get(&format!("{}/loop", server.uri())).await,
        Err(Error::Ssrf(SsrfError::TooManyRedirects(2)))
    ));
}

#[tokio::test]
async fn redirect_uses_new_dns_result_instead_of_reusing_first_connection() {
    let server = MockServer::start().await;
    Mock::given(path("/first"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/second"))
        .expect(1)
        .mount(&server)
        .await;
    let client = SafeDownloader::with_resolver(local_options(), Dns::new(&[&["127.0.0.1"], &[]]));
    assert!(matches!(
        client.safe_get(&hostname_url(&server, "/first")).await,
        Err(Error::Ssrf(SsrfError::NoAddresses(_)))
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn size_limit_rejects_declared_and_decompressed_bodies() {
    let server = MockServer::start().await;
    Mock::given(path("/plain"))
        .respond_with(ResponseTemplate::new(200).set_body_string("123456"))
        .mount(&server)
        .await;
    let client = SafeDownloader::new(DownloadOptions {
        max_bytes: 5,
        ..local_options()
    });
    assert!(matches!(
        client.safe_get(&format!("{}/plain", server.uri())).await,
        Err(Error::Ssrf(SsrfError::SizeLimit {
            declared: Some(6),
            ..
        }))
    ));
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&[b'x'; 4096]).unwrap();
    let compressed = encoder.finish().unwrap();
    assert!(compressed.len() < 100);
    Mock::given(path("/gzip"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Encoding", "gzip")
                .set_body_bytes(compressed),
        )
        .mount(&server)
        .await;
    let client = SafeDownloader::new(DownloadOptions {
        max_bytes: 100,
        ..local_options()
    });
    let response = client
        .safe_get(&format!("{}/gzip", server.uri()))
        .await
        .unwrap();
    assert!(matches!(
        response.bytes().await,
        Err(Error::Ssrf(SsrfError::SizeLimit { declared: None, .. }))
    ));
}

async fn read_request_headers<S: tokio::io::AsyncRead + Unpin>(socket: &mut S) -> String {
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        assert!(headers.len() < 16 * 1024, "oversized test request headers");
        headers.push(socket.read_u8().await.unwrap());
    }
    String::from_utf8(headers).unwrap()
}

async fn raw_response(
    head: &'static str,
    body: &'static [u8],
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request_headers(&mut socket).await;
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(body).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    (url, handle)
}

#[tokio::test]
async fn chunked_and_eof_bodies_enforce_cap_without_content_length() {
    for (head, body) in [
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            b"3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n".as_slice(),
        ),
        (
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n",
            b"abcdef".as_slice(),
        ),
    ] {
        let (url, server) = raw_response(head, body).await;
        let client = SafeDownloader::new(DownloadOptions {
            max_bytes: 5,
            ..local_options()
        });
        let response = client.safe_get(&url).await.unwrap();
        let mut stream = ssrf::iter_content_capped(response, 2, None).unwrap();
        let mut received = Vec::new();
        let mut errors = 0;
        while let Some(item) = stream.next().await {
            match item {
                Ok(bytes) => {
                    assert!(!bytes.is_empty());
                    assert!(bytes.len() <= 2);
                    received.extend(bytes);
                }
                Err(Error::Ssrf(SsrfError::SizeLimit { .. })) => errors += 1,
                Err(error) => panic!("unexpected error: {error}"),
            }
        }
        assert_eq!(errors, 1);
        assert!(received.len() <= 5);
        assert!(b"abcdef".starts_with(&received));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn exact_limit_empty_body_and_tighter_stream_limit() {
    let server = MockServer::start().await;
    Mock::given(path("/exact"))
        .respond_with(ResponseTemplate::new(200).set_body_string("12345"))
        .mount(&server)
        .await;
    Mock::given(path("/empty"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = SafeDownloader::new(DownloadOptions {
        max_bytes: 5,
        ..local_options()
    });
    assert_eq!(
        client
            .safe_get(&format!("{}/exact", server.uri()))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
        b"12345"
    );
    let response = client
        .safe_get(&format!("{}/exact", server.uri()))
        .await
        .unwrap();
    let mut stream = ssrf::iter_content_capped(response, 1, Some(3)).unwrap();
    for byte in b"123" {
        assert_eq!(stream.next().await.unwrap().unwrap(), [*byte]);
    }
    assert!(matches!(
        stream.next().await,
        Some(Err(Error::Ssrf(SsrfError::SizeLimit { limit: 3, .. })))
    ));
    assert!(stream.next().await.is_none());
    let response = client
        .safe_get(&format!("{}/empty", server.uri()))
        .await
        .unwrap();
    assert!(response.bytes().await.unwrap().is_empty());
    let response = client
        .safe_get(&format!("{}/empty", server.uri()))
        .await
        .unwrap();
    assert!(matches!(
        ssrf::iter_content_capped(response, 0, None),
        Err(Error::InvalidInput(_))
    ));
}

#[tokio::test]
async fn proxy_refusal_precedes_dns_and_never_discloses_proxy_credentials() {
    let mut options = options();
    options.proxies = ProxySettings::from_values(&BTreeMap::from([(
        "HTTPS_PROXY".into(),
        "http://name:secret@proxy.test:3128".into(),
    )]));
    let dns = Dns::new(&[]);
    let client = SafeDownloader::with_resolver(options, dns.clone());
    let error = client.safe_get("https://download.test/").await.unwrap_err();
    assert!(matches!(error, Error::Ssrf(SsrfError::Proxy)));
    assert!(!format!("{error:?} {client:?}").contains("secret"));
    assert!(dns.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn no_proxy_honors_domains_ports_ipv4_cidr_and_cgi_precedence() {
    let cases = [
        ("https://download.test/", "download.test", true),
        ("https://download.test/", ".download.test", true),
        ("https://sub.download.test/", "download.test", true),
        ("https://evildownload.test/", "download.test", false),
        ("https://download.test:443/", "download.test:443", true),
        ("https://download.test:444/", "download.test:443", false),
        ("https://8.8.8.8/", "8.8.8.0/24", true),
        ("https://8.8.8.8/", "8.8.9.0/24", false),
        ("https://download.test/", "*", true),
        ("https://download.test/", "DOWNLOAD.TEST", true),
    ];
    for (url, no_proxy, bypass) in cases {
        let mut options = options();
        // A private literal would skip the resolver; all named hosts return a
        // private address so successful bypass is still rejected before dialing.
        let url = url.replace("8.8.8.8", "127.0.0.1");
        options.proxies = ProxySettings::from_values(&BTreeMap::from([
            ("HTTPS_PROXY".into(), "http://proxy.test".into()),
            ("NO_PROXY".into(), no_proxy.replace("8.8.8.0", "127.0.0.0")),
        ]));
        let client = SafeDownloader::with_resolver(options, Dns::new(&[&["10.0.0.1"]]));
        let error = client.safe_get(&url).await.unwrap_err();
        assert_eq!(
            matches!(error, Error::Ssrf(SsrfError::NonPublic { .. })),
            bypass,
            "{url}, {no_proxy}, {error}"
        );
    }
    for (env, bypass) in [
        (
            BTreeMap::from([
                ("HTTP_PROXY".into(), "http://proxy.test".into()),
                ("REQUEST_METHOD".into(), "GET".into()),
            ]),
            true,
        ),
        (
            BTreeMap::from([
                ("HTTP_PROXY".into(), "http://proxy.test".into()),
                ("http_proxy".into(), "".into()),
            ]),
            true,
        ),
        (
            BTreeMap::from([
                ("http_proxy".into(), "http://proxy.test".into()),
                ("REQUEST_METHOD".into(), "GET".into()),
            ]),
            false,
        ),
        (
            BTreeMap::from([("ALL_PROXY".into(), "http://proxy.test".into())]),
            false,
        ),
    ] {
        let client = SafeDownloader::with_resolver(
            DownloadOptions::from_values(&env),
            Dns::new(&[&["10.0.0.1"]]),
        );
        let error = client.safe_get("http://download.test/").await.unwrap_err();
        assert_eq!(
            matches!(error, Error::Ssrf(SsrfError::NonPublic { .. })),
            bypass
        );
    }
}

#[tokio::test]
async fn private_opt_in_permits_explicit_proxy_with_scheme_and_size_guards() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(|request: &Request| {
            assert_eq!(request.headers["Host"], "download.test");
            ResponseTemplate::new(200).set_body_string("proxy response")
        })
        .expect(1)
        .mount(&proxy)
        .await;
    let mut options = local_options();
    options.proxies =
        ProxySettings::from_values(&BTreeMap::from([("HTTP_PROXY".into(), proxy.uri())]));
    options.max_bytes = 2;
    let client = SafeDownloader::with_resolver(options, Dns::new(&[&["10.0.0.1"]]));
    assert!(matches!(
        client.safe_get("http://download.test/").await,
        Err(Error::Ssrf(SsrfError::SizeLimit { .. }))
    ));
    assert!(matches!(
        client.safe_get("file:///etc/passwd").await,
        Err(Error::Ssrf(SsrfError::BlockedScheme(_)))
    ));
}

#[tokio::test]
async fn read_timeout_does_not_advance_to_another_address() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let client = SafeDownloader::with_resolver(
        DownloadOptions {
            timeout: Duration::from_millis(100),
            ..local_options()
        },
        Dns::new(&[&["127.0.0.1", "127.0.0.2"]]),
    );
    let request = client
        .safe_get(&format!("http://download.test:{port}/"))
        .await;
    assert!(matches!(request, Err(Error::Http(e)) if e.is_timeout() && !e.is_connect()));
}

#[tokio::test]
async fn dropping_unread_response_or_partial_stream_closes_the_socket() {
    for partial in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            read_request_headers(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nx")
                .await
                .unwrap();
            // EOF or a connection-reset both demonstrate cancellation.
            let read = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut request))
                .await
                .expect("download socket was leaked");
            assert!(matches!(read, Ok(0) | Err(_)));
        });
        let response = SafeDownloader::new(local_options())
            .safe_get(&url)
            .await
            .unwrap();
        if partial {
            let mut stream = ssrf::iter_content_capped(response, 1, None).unwrap();
            assert_eq!(stream.next().await.unwrap().unwrap(), b"x");
            drop(stream);
        } else {
            drop(response);
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn cap_failure_closes_body_before_delivering_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        read_request_headers(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n123456\r\n")
            .await
            .unwrap();
        let read = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut request))
            .await
            .expect("cap rejection leaked socket");
        assert!(matches!(read, Ok(0) | Err(_)));
    });
    let response = SafeDownloader::new(DownloadOptions {
        max_bytes: 5,
        ..local_options()
    })
    .safe_get(&url)
    .await
    .unwrap();
    let mut stream = ssrf::iter_content_capped(response, 10, None).unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Err(Error::Ssrf(SsrfError::SizeLimit { .. })))
    ));
    // Retain the unpolled error-yielding stream while verifying socket closure.
    server.await.unwrap();
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn canceling_header_wait_releases_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let ready = Arc::new(tokio::sync::Notify::new());
    let server_ready = ready.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        read_request_headers(&mut socket).await;
        server_ready.notify_one();
        let read = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut request))
            .await
            .expect("canceled header wait leaked socket");
        assert!(matches!(read, Ok(0) | Err(_)));
    });
    let request =
        tokio::spawn(async move { SafeDownloader::new(local_options()).safe_get(&url).await });
    ready.notified().await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    server.await.unwrap();
}

#[tokio::test]
async fn body_read_timeout_is_a_transport_error_and_releases_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        read_request_headers(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nx")
            .await
            .unwrap();
        let read = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut request))
            .await
            .expect("read timeout leaked socket");
        assert!(matches!(read, Ok(0) | Err(_)));
    });
    let response = SafeDownloader::new(DownloadOptions {
        timeout: Duration::from_millis(100),
        ..local_options()
    })
    .safe_get(&url)
    .await
    .unwrap();
    assert!(matches!(response.bytes().await, Err(Error::Http(e)) if e.is_timeout()));
    server.await.unwrap();
}

#[tokio::test]
async fn pinned_tls_preserves_sni_trust_chain_and_certificate_hostname() {
    use tokio_rustls::{
        TlsAcceptor,
        rustls::{
            self,
            pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
        },
    };
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(
            include_bytes!("fixtures/tls/server.der").to_vec(),
        )],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            include_bytes!("fixtures/tls/PUBLIC_TEST_ONLY_key.der").to_vec(),
        )),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    for (hostname, trust, expected_success) in [
        ("download.test", true, true),
        ("wrong.test", true, false),
        ("download.test", false, false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = acceptor.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let accepted = acceptor.accept(socket).await;
            if expected_success {
                let mut socket = accepted.unwrap();
                assert_eq!(socket.get_ref().1.server_name(), Some("download.test"));
                let request = read_request_headers(&mut socket).await.to_ascii_lowercase();
                assert!(request.contains(&format!("host: download.test:{port}\r\n")));
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            } else {
                assert!(accepted.is_err());
            }
        });
        let mut options = local_options();
        if trust {
            options.root_certificates.push(
                reqwest::Certificate::from_der(include_bytes!("fixtures/tls/root.der")).unwrap(),
            );
        }
        let client = SafeDownloader::with_resolver(options, Dns::new(&[&["127.0.0.1"]]));
        let result = client
            .safe_get(&format!("https://{hostname}:{port}/"))
            .await;
        if expected_success {
            assert_eq!(result.unwrap().bytes().await.unwrap(), b"ok");
        } else {
            let error = result.unwrap_err();
            assert!(matches!(error, Error::Http(ref e) if e.is_connect()));
            let mut source: &dyn std::error::Error = &error;
            let mut diagnostic = String::new();
            loop {
                diagnostic.push_str(&source.to_string());
                if let Some(next) = source.source() {
                    source = next;
                } else {
                    break;
                }
            }
            assert!(
                diagnostic.to_ascii_lowercase().contains(if trust {
                    "not valid for name"
                } else {
                    "issuer"
                }),
                "{diagnostic}"
            );
        }
        server.await.unwrap();
    }
}

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use runpod::{
    Client, ClientConfig, Value,
    apps::{
        AppsApiClient,
        auth::{self, BrowserLoginOptions, EmailLoginClient},
    },
    json,
};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

fn response(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_json(body)
        .insert_header("Set-Cookie", "browser_session=unused")
        .insert_header("Location", "/redirect-target")
}
async fn email_server(responses: Vec<ResponseTemplate>) -> MockServer {
    let server = MockServer::start().await;
    let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| response(418, json!({"unexpected":true})))
        })
        .mount(&server)
        .await;
    server
}
fn email_client(server: &MockServer) -> EmailLoginClient {
    EmailLoginClient::new(format!("{}/prefix/", server.uri())).unwrap()
}
fn initialized() -> ResponseTemplate {
    response(201, json!({"attemptId":"att_test"}))
}
fn granted() -> ResponseTemplate {
    response(200, json!({"credential":{"apiKey":"rpa_test"}}))
}
async fn code() -> runpod::Result<String> {
    Ok("012345".into())
}
async fn browser_server(request: Value, statuses: Vec<Value>) -> (MockServer, AppsApiClient) {
    let server = MockServer::start().await;
    let statuses = Arc::new(Mutex::new(VecDeque::from(statuses)));
    Mock::given(path("/graphql")).and(method("POST")).respond_with(move |req:&Request| {
        assert!(req.headers.get("authorization").is_none());
        let body:Value=req.body_json().unwrap();
        let data = if body["query"].as_str().unwrap().contains("createFlashAuthRequest") {
            json!({"createFlashAuthRequest":request})
        } else {json!({"flashAuthRequestStatus":statuses.lock().unwrap().pop_front().unwrap_or(json!({"status":"PENDING"}))})};
        response(200,json!({"data":data}))
    }).mount(&server).await;
    let api = AppsApiClient::new(
        Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("existing-key-must-not-be-sent")
        })
        .unwrap(),
    )
    .unwrap();
    (server, api)
}
fn options() -> BrowserLoginOptions {
    BrowserLoginOptions {
        timeout: Duration::from_secs(2),
        poll_interval: Duration::from_millis(1),
        console_url: "https://console.example".into(),
    }
}

#[tokio::test]
async fn test_returns_key_on_approval() {
    let (server, api) = browser_server(
        json!({"id":"req-1"}),
        vec![
            json!({"status":"PENDING"}),
            json!({"status":"APPROVED","apiKey":"rpa_new"}),
        ],
    )
    .await;
    let mut urls = Vec::new();
    let mut observe = |url: &str| {
        urls.push(url.to_owned());
        Ok(())
    };
    assert_eq!(
        auth::browser_login(&api, &options(), Some(&mut observe))
            .await
            .unwrap(),
        "rpa_new"
    );
    assert_eq!(urls, [auth::auth_url("req-1", "https://console.example")]);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn test_denied_raises() {
    for status in ["DENIED", "EXPIRED"] {
        let (_server, api) = browser_server(
            json!({"id":"req-1"}),
            vec![json!({"status":status,"apiKey":"ignored"})],
        )
        .await;
        assert_eq!(
            auth::browser_login(&api, &options(), None)
                .await
                .unwrap_err()
                .to_string(),
            format!("login {}", status.to_lowercase())
        );
    }
}

#[tokio::test]
async fn test_timeout_raises() {
    for expiry in [Value::Null, json!("2000-01-01T00:00:00Z")] {
        let (server, api) = browser_server(json!({"id":"req-1","expiresAt":expiry}), vec![]).await;
        let options = BrowserLoginOptions {
            timeout: if expiry.is_null() {
                Duration::ZERO
            } else {
                Duration::from_secs(600)
            },
            ..options()
        };
        assert_eq!(
            auth::browser_login(&api, &options, None)
                .await
                .unwrap_err()
                .to_string(),
            "login timed out"
        );
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "expired requests still poll once"
        );
    }
}

#[tokio::test]
async fn test_missing_request_id_raises() {
    for request in [json!({}), json!({"id":""}), json!({"id":3})] {
        let (server, api) = browser_server(request, vec![]).await;
        let mut observe = |_: &str| panic!("no approval URL should be produced");
        assert_eq!(
            auth::browser_login(&api, &options(), Some(&mut observe))
                .await
                .unwrap_err()
                .to_string(),
            "auth request failed to initialize"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_consumed_without_key_raises() {
    let (_server, api) =
        browser_server(json!({"id":"req-1"}), vec![json!({"status":"CONSUMED"})]).await;
    assert_eq!(
        auth::browser_login(&api, &options(), None)
            .await
            .unwrap_err()
            .to_string(),
        "login request was already used"
    );
}

#[tokio::test]
async fn browser_approval_precedes_deadline_and_expiration_parsing_uses_offsets() {
    for (status, key, expiry) in [
        ("APPROVED", " ", "2000-01-01T00:00:00Z"),
        ("CONSUMED", "key", "2000-01-01T01:00:00+01:00"),
    ] {
        let (_server, api) = browser_server(
            json!({"id":"req","expiresAt":expiry}),
            vec![json!({"status":status,"apiKey":key})],
        )
        .await;
        assert_eq!(
            auth::browser_login(
                &api,
                &BrowserLoginOptions {
                    timeout: Duration::ZERO,
                    ..options()
                },
                None
            )
            .await
            .unwrap(),
            key
        );
    }
    let (_server, api) = browser_server(
        json!({"id":"req","expiresAt":"invalid"}),
        vec![
            json!({"status":"APPROVED"}),
            json!({"status":"APPROVED","apiKey":"key"}),
        ],
    )
    .await;
    assert_eq!(
        auth::browser_login(&api, &options(), None).await.unwrap(),
        "key"
    );
    assert_eq!(
        auth::auth_url("id&raw=1", "https://example/"),
        "https://example//flash/login?request=id&raw=1"
    );
}

#[tokio::test]
async fn browser_observer_error_stops_before_polling_and_cancel_stops_pending_poll() {
    let (server, api) = browser_server(json!({"id":"req"}), vec![]).await;
    let mut observe = |_: &str| Err(runpod::Error::InvalidInput("observer stopped".into()));
    assert_eq!(
        auth::browser_login(&api, &options(), Some(&mut observe))
            .await
            .unwrap_err()
            .to_string(),
        "observer stopped"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    let (server, api) = browser_server(json!({"id":"req"}), vec![]).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    Mock::given(path("/graphql"))
        .and(body_string_contains("flashAuthRequestStatus"))
        .respond_with(move |_: &Request| {
            signal.notify_one();
            response(
                200,
                json!({"data":{"flashAuthRequestStatus":{"status":"PENDING"}}}),
            )
            .set_delay(Duration::from_millis(200))
        })
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    let options = options();
    tokio::select! {
        result = auth::browser_login(&api,&options,None) => panic!("pending poll should be canceled: {result:?}"),
        () = started.notified() => {}
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn test_agent_login_uses_anonymous_pkce_exchange() {
    let server = email_server(vec![initialized(), granted()]).await;
    let client = email_client(&server);
    assert_eq!(
        client
            .agent_login("owner@example.com", || async {
                assert_eq!(server.received_requests().await.unwrap().len(), 1);
                code().await
            })
            .await
            .unwrap(),
        "rpa_test"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].url.path(),
        "/prefix/v2/signup/authentication-attempts"
    );
    assert_eq!(
        requests[1].url.path(),
        "/prefix/v2/signup/authentication-attempts/att_test/verify"
    );
    let start: Value = requests[0].body_json().unwrap();
    let verify: Value = requests[1].body_json().unwrap();
    assert_eq!(start.as_object().unwrap().len(), 3);
    assert_eq!(verify.as_object().unwrap().len(), 2);
    assert_eq!(start["email"], "owner@example.com");
    assert_eq!(start["keyName"], "runpod-cli-agent");
    let verifier = verify["codeVerifier"].as_str().unwrap();
    assert!(
        regex::Regex::new(r"^[A-Za-z0-9._~-]{43,128}$")
            .unwrap()
            .is_match(verifier)
    );
    assert_eq!(
        start["codeChallenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    );
    assert!(!start.to_string().contains(verifier));
    assert_eq!(verify["code"], "012345");
    let uuid = requests[0].headers["idempotency-key"].to_str().unwrap();
    assert!(
        regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
            .unwrap()
            .is_match(uuid)
    );
    assert!(requests[1].headers.get("idempotency-key").is_none());
    for request in requests {
        assert!(request.headers.get("authorization").is_none());
        assert!(request.headers.get("cookie").is_none());
        assert_eq!(
            request.headers["user-agent"].to_str().unwrap(),
            concat!("runpod-rust/", env!("CARGO_PKG_VERSION"))
        );
        assert!(!request.url.as_str().contains(verifier));
    }
    assert!(!format!("{client:?}").contains("rpa_test"));
}

#[tokio::test]
async fn test_each_login_has_its_own_verifier_and_idempotency_key() {
    let server = email_server(vec![initialized(), granted(), initialized(), granted()]).await;
    let client = email_client(&server);
    for _ in 0..2 {
        client.agent_login("owner@example.com", code).await.unwrap();
    }
    let requests = server.received_requests().await.unwrap();
    assert_ne!(
        requests[0].headers["idempotency-key"],
        requests[2].headers["idempotency-key"]
    );
    let first: Value = requests[0].body_json().unwrap();
    let second: Value = requests[2].body_json().unwrap();
    assert_ne!(first["codeChallenge"], second["codeChallenge"]);
}

#[tokio::test]
async fn test_start_errors_do_not_prompt_or_follow_redirects() {
    for (status, message) in [
        (403, "console"),
        (404, "unavailable"),
        (422, "check --email"),
        (429, "wait"),
        (500, "HTTP 500"),
        (307, "HTTP 307"),
        (200, "HTTP 200"),
    ] {
        let server = email_server(vec![response(
            status,
            json!({"detail":"sensitive-server-detail"}),
        )])
        .await;
        let error = email_client(&server)
            .agent_login("owner@example.com", || async {
                panic!("start error must not prompt")
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(message));
        assert!(!error.contains("sensitive-server-detail"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_verification_errors_are_not_retried() {
    for (status, message) in [
        (403, "console"),
        (404, "expired"),
        (429, "wait"),
        (503, "HTTP 503"),
        (307, "HTTP 307"),
        (201, "HTTP 201"),
    ] {
        let server = email_server(vec![
            initialized(),
            response(status, json!({"detail":"sensitive-server-detail"})),
        ])
        .await;
        let error = email_client(&server)
            .agent_login("owner@example.com", code)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(message));
        assert!(!error.contains("sensitive-server-detail"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn test_rejected_codes_reuse_attempt_and_verifier() {
    for rejections in [1, 4] {
        let mut responses = vec![initialized()];
        responses.extend((0..rejections).map(|_| response(422, json!({}))));
        responses.push(granted());
        let server = email_server(responses).await;
        let mut count = 0;
        assert_eq!(
            email_client(&server)
                .agent_login("owner@example.com", || {
                    let value = format!("{count:06}");
                    count += 1;
                    std::future::ready(Ok(value))
                })
                .await
                .unwrap(),
            "rpa_test"
        );
        assert_eq!(count, rejections + 1);
        let requests = server.received_requests().await.unwrap();
        let mut verifier = None;
        for (index, request) in requests.iter().skip(1).enumerate() {
            let body: Value = request.body_json().unwrap();
            assert_eq!(body["code"], format!("{index:06}"));
            if let Some(previous) = &verifier {
                assert_eq!(previous, &body["codeVerifier"]);
            } else {
                verifier = Some(body["codeVerifier"].clone());
            }
            assert!(request.headers.get("idempotency-key").is_none());
            assert_eq!(
                request.url.path(),
                "/prefix/v2/signup/authentication-attempts/att_test/verify"
            );
        }
    }
}

#[tokio::test]
async fn test_retry_stops_on_terminal_verification_error() {
    for status in [403, 404, 429, 503, 307] {
        let server = email_server(vec![
            initialized(),
            response(422, json!({})),
            response(status, json!({"detail":"sensitive-server-detail"})),
        ])
        .await;
        let error = email_client(&server)
            .agent_login("owner@example.com", code)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("sensitive-server-detail"));
        if status == 404 {
            assert!(error.contains("start login again"));
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }
}

#[tokio::test]
async fn test_aborted_retry_does_not_send_another_request() {
    let server = email_server(vec![initialized(), response(422, json!({}))]).await;
    let mut count = 0;
    let error = email_client(&server)
        .agent_login("owner@example.com", || {
            count += 1;
            std::future::ready(if count == 1 {
                Ok("123456".into())
            } else {
                Err(runpod::Error::InvalidInput("reader EOF".into()))
            })
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "reader EOF");
    assert_eq!(count, 2);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn test_invalid_attempt_does_not_prompt() {
    for value in [
        json!({}),
        json!({"attemptId":null}),
        json!({"attemptId":1}),
        json!([]),
    ] {
        let server = email_server(vec![response(201, value)]).await;
        assert!(
            email_client(&server)
                .agent_login("owner@example.com", || async {
                    panic!("invalid initialization")
                })
                .await
                .is_err()
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn test_invalid_key_response_fails_without_exposing_body() {
    let mut cases =
        vec![ResponseTemplate::new(200).set_body_raw("not json secret", "application/json")];
    cases.extend(
        [
            json!([]),
            json!({}),
            json!({"credential":"invalid"}),
            json!({"credential":{"apiKey":" "}}),
            json!({"credential":{"apiKey":123}}),
        ]
        .into_iter()
        .map(|body| response(200, body)),
    );
    for reply in cases {
        let server = email_server(vec![initialized(), reply]).await;
        let error = email_client(&server)
            .agent_login("owner@example.com", code)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn test_aborted_code_prompt_does_not_verify() {
    let server = email_server(vec![initialized()]).await;
    let error = email_client(&server)
        .agent_login("owner@example.com", || async {
            Err(runpod::Error::InvalidInput("reader EOF".into()))
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "reader EOF");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn test_transport_failures_are_sanitized() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = EmailLoginClient::new(format!("http://{address}"))
        .unwrap()
        .agent_login("secret@example.com", code)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "could not complete email sign-in; check your connection"
    );
    let server = email_server(vec![initialized().set_delay(Duration::from_millis(500))]).await;
    let error = email_client(&server)
        .with_timeout(Duration::from_millis(100))
        .agent_login("secret@example.com", code)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "could not complete email sign-in; check your connection"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn email_media_types_percent_encoded_ids_and_untrimmed_keys_match_python() {
    let server = email_server(vec![
        response(201, json!({"attemptId":"a/b ?#"})),
        ResponseTemplate::new(200).set_body_raw(
            r#"{"credential":{"apiKey":" key "}}"#,
            "application/problem+json",
        ),
    ])
    .await;
    assert_eq!(
        email_client(&server)
            .agent_login("owner@example.com", code)
            .await
            .unwrap(),
        " key "
    );
    assert_eq!(
        server.received_requests().await.unwrap()[1].url.path(),
        "/prefix/v2/signup/authentication-attempts/a%2Fb%20%3F%23/verify"
    );
    let server = email_server(vec![
        ResponseTemplate::new(201).set_body_raw(r#"{"attemptId":"att"}"#, "text/plain"),
    ])
    .await;
    assert_eq!(
        email_client(&server)
            .agent_login("owner@example.com", code)
            .await
            .unwrap_err()
            .to_string(),
        "could not complete email sign-in; check your connection"
    );
}

#[tokio::test]
async fn cancellation_during_code_read_does_not_verify() {
    let server = email_server(vec![initialized()]).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let client = email_client(&server);
    tokio::select! {
        result=client.agent_login("owner@example.com",move ||{let signal=signal.clone();async move {signal.notify_one();std::future::pending().await}})=>panic!("reader should wait: {result:?}"),
        ()=started.notified()=>{}
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

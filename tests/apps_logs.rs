use futures_util::StreamExt;
use runpod::{
    Client, ClientConfig,
    apps::logs::{LogStreamOptions, LogsClient, tail_summary},
    json,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, path, query_param},
};

fn client(server: &MockServer) -> LogsClient {
    LogsClient::with_hapi_base(
        Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap(),
        server.uri(),
    )
    .unwrap()
}

#[tokio::test]
async fn snapshot_normalizes_nulls_and_tail_preserves_order_and_zero_quirk() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/pod/pod1/logs"))
        .and(query_param("type", "all"))
        .and(header("authorization", "Bearer key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"container":["a","b"],"system":null,"extra":false})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let logs = client(&server)
        .pod_logs("pod1", "all", Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(logs, json!({"container":["a","b"],"system":[],"extra":[]}));
    assert_eq!(
        tail_summary(&logs, 0).unwrap(),
        "--- container (last 0) ---\na\nb"
    );
    assert_eq!(
        tail_summary(&json!({"system":["boot"],"container":["a","b"]}), 1).unwrap(),
        "--- system (last 1) ---\nboot\n--- container (last 1) ---\nb"
    );
    assert_eq!(tail_summary(&json!({}), 20).unwrap(), "(no logs available)");
}

#[tokio::test]
async fn stream_selects_hapi_pod_or_worker_and_preserves_cursor_delivery() {
    for (endpoint, source, route) in [
        (None, Some("all"), "/v1/pod/pod1/logs"),
        (None, Some("system"), "/v2/pods/pod1/logs"),
        (
            Some("ep1"),
            Some("container"),
            "/v2/serverless/ep1/workers/pod1/logs",
        ),
        (Some("ep1"), None, "/v2/serverless/ep1/workers/pod1/logs"),
    ] {
        let server = MockServer::start().await;
        let mut mock = Mock::given(path(route))
            .and(header("authorization", "Bearer key"))
            .and(header("Last-Event-ID", "previous"))
            .and(query_param("tail", "7"))
            .and(query_param("since", "2026-01-01T00:00:00Z"));
        if source == Some("all") {
            mock = mock
                .and(query_param("type", "all"))
                .and(query_param("stream", "true"));
        } else if let Some(source) = source {
            mock = mock.and(query_param("source", source));
        }
        let frames = ":heartbeat\r\nid: first\r\ndata: {\"line\":1}\r\n\r\ndata: {\"line\":2}\nid: second\n\nid: invalid\ndata: not json\n\nid: third\ndata: {\"line\":\ndata: 3}\n\ndata: {\"line\":\"incomplete\"}\nid: incomplete\n";
        mock.respond_with(ResponseTemplate::new(200).set_body_raw(frames, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let cursors = Arc::new(Mutex::new(Vec::new()));
        let captured = cursors.clone();
        let options = LogStreamOptions {
            endpoint_id: endpoint.map(str::to_owned),
            log_type: source.map(str::to_owned),
            tail: 7,
            since: Some("2026-01-01T00:00:00Z".into()),
            last_event_id: Some("previous".into()),
            ..Default::default()
        };
        let mut stream = client(&server).stream_pod_logs(
            "pod1",
            options,
            Some(Box::new(move |id| {
                captured.lock().unwrap().push(id.to_owned());
                Ok(())
            })),
        );
        for (value, id) in [(1, "first"), (2, "second"), (3, "third")] {
            assert_eq!(stream.next().await.unwrap().unwrap(), json!({"line":value}));
            assert_eq!(cursors.lock().unwrap().last().unwrap(), id);
        }
        assert!(stream.next().await.is_none());
        assert_eq!(cursors.lock().unwrap().len(), 3);
        if source.is_none() {
            assert!(
                !server.received_requests().await.unwrap()[0]
                    .url
                    .query_pairs()
                    .any(|(k, _)| k == "source")
            );
        }
    }
}

#[tokio::test]
async fn idle_timeout_starts_after_headers_and_http_errors_propagate() {
    for status in [200, 403] {
        let server = MockServer::start().await;
        Mock::given(path("/v1/pod/pod1/logs"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_raw("data: {\"line\":\"ready\"}\n\n", "text/event-stream")
                    .set_delay(Duration::from_millis(50)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut stream = client(&server).stream_pod_logs(
            "pod1",
            LogStreamOptions {
                idle_timeout: Some(Duration::from_millis(20)),
                ..Default::default()
            },
            None,
        );
        let result = stream.next().await.unwrap();
        if status == 200 {
            assert_eq!(result.unwrap(), json!({"line":"ready"}));
            assert!(stream.next().await.is_none());
        } else {
            assert_eq!(result.unwrap_err().status_code(), Some(403));
        }
    }
}

#[tokio::test]
async fn fragmented_utf8_and_idle_shutdown_close_the_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
        }
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        for byte in "id: cursor\ndata: {\"line\":\"crab 🦀\"}\n\n".as_bytes() {
            socket.write_all(&[*byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0, "dropping an idle stream must close the body");
    });
    let logs = LogsClient::with_hapi_base(Client::new("key").unwrap(), url).unwrap();
    let mut stream = logs.stream_pod_logs(
        "pod1",
        LogStreamOptions {
            idle_timeout: Some(Duration::from_millis(100)),
            ..Default::default()
        },
        None,
    );
    assert_eq!(
        stream.next().await.unwrap().unwrap(),
        json!({"line":"crab 🦀"})
    );
    assert!(stream.next().await.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn invalid_cursor_is_rejected_and_callback_failure_closes_stream() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/pod/pod1/logs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("id: ok\ndata: {}\n\n", "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let logs = client(&server);
    let mut invalid = logs.stream_pod_logs(
        "pod1",
        LogStreamOptions {
            last_event_id: Some("bad\r\nheader".into()),
            ..Default::default()
        },
        None,
    );
    assert!(invalid.next().await.unwrap().is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
    let mut stream = logs.stream_pod_logs(
        "pod1",
        LogStreamOptions::default(),
        Some(Box::new(|_| {
            Err(runpod::Error::InvalidInput("stop observing".into()))
        })),
    );
    assert!(
        stream
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("stop observing")
    );
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn socket_timeout_before_headers_propagates_even_with_idle_timeout() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(tokio::sync::Notify::new());
    let notify = received.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        let mut request = Vec::new();
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
        }
        notify.notify_one();
        let _ = socket.read(&mut buffer).await;
    });
    let logs = LogsClient::with_hapi_base(Client::new("key").unwrap(), url).unwrap();
    let request = tokio::spawn(async move {
        logs.stream_pod_logs(
            "pod",
            LogStreamOptions {
                idle_timeout: Some(Duration::from_millis(1)),
                ..Default::default()
            },
            None,
        )
        .next()
        .await
        .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(5), received.notified())
        .await
        .unwrap();
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    tokio::time::advance(Duration::from_secs(61)).await;
    let error = request.await.unwrap().unwrap_err();
    assert!(matches!(error,runpod::Error::Http(error) if error.is_timeout()));
    assert!(
        start.elapsed() <= Duration::from_secs(62),
        "socket timeout must apply while waiting for response headers"
    );
    tokio::time::resume();
    server.await.unwrap();
}

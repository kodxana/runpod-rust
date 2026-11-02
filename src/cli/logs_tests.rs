use super::*;
use crate::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[test]
fn test_logs_routes_options_to_shared_streamer() {
    let args = command("endpoint-id")
        .try_get_matches_from([
            "logs",
            "ep1",
            "--worker",
            "w1",
            "--worker",
            "w2",
            "--follow",
            "--source",
            "system",
            "--tail",
            "20",
            "--since",
            "2026-01-01T00:00:00Z",
            "--json",
        ])
        .unwrap();
    let options = Options::read(&args, "endpoint-id");
    assert!(
        matches!(options.target,Target::Endpoint{ref id,ref workers} if id=="ep1" && workers==&["w1","w2"])
    );
    assert!(options.follow && options.as_json);
    assert_eq!(options.source.as_deref(), Some("system"));
    assert_eq!(options.tail, 20);
    assert_eq!(options.since.as_deref(), Some("2026-01-01T00:00:00Z"));
}
#[test]
fn test_logs_forward_options() {
    for extra in [
        vec![],
        vec![
            "-f",
            "--tail",
            "5",
            "--source",
            "container",
            "--since",
            "2026-10-19T00:00:00Z",
            "--json",
        ],
    ] {
        let args = command("pod-id")
            .try_get_matches_from(["logs", "pod123"].into_iter().chain(extra.iter().copied()))
            .unwrap();
        let options = Options::read(&args, "pod-id");
        assert!(matches!(options.target,Target::Pod(ref id) if id=="pod123"));
        assert_eq!(options.follow, !extra.is_empty());
        assert_eq!(options.as_json, !extra.is_empty());
        assert_eq!(options.tail, if extra.is_empty() { 100 } else { 5 });
        assert_eq!(
            options.source.as_deref(),
            if extra.is_empty() {
                None
            } else {
                Some("container")
            }
        );
        assert_eq!(
            options.since.as_deref(),
            if extra.is_empty() {
                None
            } else {
                Some("2026-10-19T00:00:00Z")
            }
        );
    }
}

async fn request(socket: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut bytes = vec![];
    let mut buffer = [0; 1024];
    while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = socket.read(&mut buffer).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buffer[..n]);
    }
    String::from_utf8(bytes).unwrap()
}
fn http_reader(url: String) -> HttpReader {
    let client = Client::with_config(crate::ClientConfig {
        api_base_url: url.clone(),
        ..crate::ClientConfig::new("fixture-key")
    })
    .unwrap();
    HttpReader {
        logs: LogsClient::with_hapi_base(client.clone(), url).unwrap(),
        client,
    }
}
struct Observed {
    bytes: Vec<u8>,
    received: Arc<tokio::sync::Notify>,
}
impl std::io::Write for Observed {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        if String::from_utf8_lossy(&self.bytes).contains("resumed") {
            self.received.notify_one();
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn real_sse_reconnect_preserves_cursor_and_cancellation_closes_socket() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let reader = http_reader(format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        for attempt in 0..4 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = request(&mut socket).await.to_ascii_lowercase();
            assert!(request.contains("get /v2/pods/pod1/logs?tail=100"));
            assert_eq!(request.contains("last-event-id: cursor1"), attempt > 0);
            if attempt == 1 {
                // A connection can disappear before response headers arrive.
                continue;
            }
            if attempt == 2 {
                socket.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                continue;
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
            let line = if attempt == 0 { "first" } else { "resumed" };
            socket
                .write_all(format!("id: cursor1\ndata: {}\n\n", old(line)).as_bytes())
                .await
                .unwrap();
            if attempt == 3 {
                let mut buffer = [0; 1];
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap(),
                    0
                );
            }
        }
    });
    let received = Arc::new(tokio::sync::Notify::new());
    let mut out = Observed {
        bytes: vec![],
        received: received.clone(),
    };
    let mut err = vec![];
    let opts = Options {
        follow: true,
        ..options(false)
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut streams = Streams::new(&mut out, &mut err, false);
        tokio::select! {
            result=opts.show(&reader,&mut streams,timing())=>panic!("follow ended: {result:?}"),
            ()=received.notified()=>{}
        }
    })
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(
        String::from_utf8(out.bytes).unwrap(),
        "[container] first\n[container] resumed\n"
    );
}
#[tokio::test]
async fn real_snapshot_idle_and_cutoff_close_socket() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for cutoff in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let reader = http_reader(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let mut body = format!("data: {}\n\n", old("backfill"));
            if cutoff {
                body.push_str(&format!(
                    "data: {}\n\n",
                    event("live", "2999-01-01T00:00:00Z")
                ));
            }
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
            let mut buffer = [0; 1];
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
        });
        let mut out = vec![];
        let mut err = vec![];
        tokio::time::timeout(
            Duration::from_secs(5),
            options(false).show(
                &reader,
                &mut Streams::new(&mut out, &mut err, false),
                Timing {
                    idle: Duration::from_millis(30),
                    ..timing()
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "[container] backfill\n");
    }
}

#[derive(Default)]
struct Script {
    events: Vec<(Option<&'static str>, Value)>,
    error: Option<Error>,
    pending: bool,
    delay: Duration,
    wait: Option<Arc<tokio::sync::Notify>>,
    signal: Option<Arc<tokio::sync::Notify>>,
}
#[derive(Default)]
struct Fake {
    workers: Mutex<VecDeque<Vec<String>>>,
    scripts: Mutex<VecDeque<Script>>,
    calls: Mutex<Vec<(String, LogStreamOptions)>>,
    closed: Arc<Mutex<Vec<String>>>,
}
struct Closed(Arc<Mutex<Vec<String>>>, String);
impl Drop for Closed {
    fn drop(&mut self) {
        self.0.lock().unwrap().push(self.1.clone());
    }
}
impl Reader for Fake {
    async fn workers(&self, _: &str) -> crate::Result<Vec<String>> {
        let mut workers = self.workers.lock().unwrap();
        Ok(if workers.len() > 1 {
            workers.pop_front().unwrap()
        } else {
            workers.front().cloned().unwrap_or_default()
        })
    }
    fn stream(
        &self,
        worker: &str,
        options: LogStreamOptions,
        mut observer: EventIdObserver,
    ) -> BoxStream<'static, crate::Result<Value>> {
        self.calls.lock().unwrap().push((worker.into(), options));
        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected stream");
        let closed = Closed(self.closed.clone(), worker.into());
        Box::pin(async_stream::try_stream! {
            let _closed = closed;
            tokio::time::sleep(script.delay).await;
            if let Some(wait) = script.wait {wait.notified().await;}
            if let Some(signal) = script.signal {signal.notify_one();}
            for (cursor, event) in script.events {
                if let Some(cursor) = cursor { observer(cursor)?; }
                yield event;
            }
            if let Some(error) = script.error { Err(error)?; }
            if script.pending { std::future::pending::<()>().await; }
        })
    }
}
fn event(line: &str, ts: &str) -> Value {
    json!({"source":"container","line":line,"ts":ts})
}
fn old(line: &str) -> Value {
    event(line, "2026-01-01T00:00:00Z")
}
fn script(line: &str) -> Script {
    Script {
        events: vec![(None, old(line))],
        ..Script::default()
    }
}
fn fake(scripts: Vec<Script>) -> Fake {
    Fake {
        scripts: Mutex::new(scripts.into()),
        ..Fake::default()
    }
}
fn status(status: u16) -> Error {
    Error::Query {
        message: "denied".into(),
        query: "logs".into(),
        status_code: Some(status),
        errors: Box::new(Value::Null),
    }
}
fn options(endpoint: bool) -> Options {
    Options {
        target: if endpoint {
            Target::Endpoint {
                id: "ep1".into(),
                workers: vec![],
            }
        } else {
            Target::Pod("pod1".into())
        },
        follow: false,
        source: None,
        tail: 100,
        since: None,
        as_json: false,
    }
}
fn timing() -> Timing {
    Timing {
        started: DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        idle: Duration::from_millis(1),
        reconnect: Duration::from_millis(1),
        poll: Duration::from_millis(5),
    }
}
async fn run(options: &Options, reader: &Fake, limit: Duration) -> (Option<CliResult<()>>, String) {
    let mut out = vec![];
    let mut err = vec![];
    let mut streams = Streams::new(&mut out, &mut err, false);
    let result = tokio::time::timeout(limit, options.show(reader, &mut streams, timing()))
        .await
        .ok();
    (result, String::from_utf8(out).unwrap())
}
async fn snapshot(options: &Options, reader: &Fake) -> (CliResult<()>, String) {
    let (result, out) = run(options, reader, Duration::from_secs(1)).await;
    (result.expect("snapshot did not finish"), out)
}

#[tokio::test]
async fn test_pod_snapshot_uses_sdk_reader() {
    for as_json in [false, true] {
        let fake = fake(vec![script("hello")]);
        let opts = Options {
            tail: 7,
            since: Some("2025-01-01T00:00:00Z".into()),
            as_json,
            ..options(false)
        };
        let (result, text) = snapshot(&opts, &fake).await;
        result.unwrap();
        if as_json {
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), old("hello"));
        } else {
            assert_eq!(text, "[container] hello\n");
        }
        let calls = fake.calls.lock().unwrap();
        let (worker, options) = &calls[0];
        assert_eq!(worker, "pod1");
        assert_eq!(options.log_type, None);
        assert_eq!(options.tail, 7);
        assert_eq!(options.since, opts.since);
        assert_eq!(options.idle_timeout, Some(timing().idle));
    }
}
#[tokio::test]
async fn test_worker_snapshot_is_sorted_and_filterable() {
    let fake = fake(vec![
        Script {
            events: vec![(None, event("a", "2026-01-01T00:00:02Z"))],
            ..Script::default()
        },
        script("b"),
    ]);
    fake.workers
        .lock()
        .unwrap()
        .push_back(vec!["a".into(), "b".into(), "ignored".into()]);
    let opts = Options {
        target: Target::Endpoint {
            id: "ep1".into(),
            workers: vec!["a".into(), "b".into()],
        },
        source: Some("system".into()),
        as_json: true,
        ..options(true)
    };
    let (result, text) = snapshot(&opts, &fake).await;
    result.unwrap();
    let rows: Vec<Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(
        rows.iter()
            .map(|r| r["worker"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    for (_, options) in fake.calls.lock().unwrap().iter() {
        assert_eq!(options.endpoint_id.as_deref(), Some("ep1"));
        assert_eq!(options.log_type.as_deref(), Some("system"));
    }
}
#[tokio::test]
async fn test_snapshot_closes_idle_or_live_stream() {
    for empty in [false, true] {
        let script = if empty {
            Script::default()
        } else {
            Script {
                events: vec![
                    (None, old("backfill")),
                    (None, event("live", "2999-01-01T00:00:00Z")),
                    (None, old("after cutoff")),
                ],
                pending: true,
                ..Script::default()
            }
        };
        let fake = fake(vec![script]);
        let (result, text) = snapshot(&options(false), &fake).await;
        result.unwrap();
        assert_eq!(text, if empty { "" } else { "[container] backfill\n" });
        assert_eq!(*fake.closed.lock().unwrap(), ["pod1"]);
    }
}
#[tokio::test]
async fn test_empty_workers() {
    for as_json in [false, true] {
        let (result, text) = snapshot(
            &Options {
                as_json,
                ..options(true)
            },
            &Fake::default(),
        )
        .await;
        result.unwrap();
        assert_eq!(text, if as_json { "" } else { "no workers\n" });
    }
}
#[tokio::test]
async fn test_snapshot_propagates_connection_timeout() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::any())
        .respond_with(wiremock::ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
        .mount(&server)
        .await;
    let error = reqwest::Client::new()
        .get(server.uri())
        .timeout(Duration::from_millis(10))
        .send()
        .await
        .unwrap_err();
    assert!(error.is_timeout());
    let fake = fake(vec![Script {
        error: Some(error.into()),
        ..Script::default()
    }]);
    assert!(snapshot(&options(false), &fake).await.0.is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn test_snapshot_surfaces_delayed_authentication_failure() {
    let fake = fake(vec![Script {
        error: Some(status(403)),
        delay: Duration::from_millis(30),
        ..Script::default()
    }]);
    let (result, text) = snapshot(&options(false), &fake).await;
    assert_eq!(result.unwrap_err().to_string(), "denied");
    assert_eq!(text, "");
}
#[tokio::test]
async fn test_stream_errors_do_not_hide_auth_failures() {
    for (endpoint, status_code, raises) in
        [(false, 404, true), (true, 404, false), (true, 403, true)]
    {
        let fake = fake(vec![Script {
            error: Some(status(status_code)),
            ..Script::default()
        }]);
        fake.workers.lock().unwrap().push_back(vec!["pod1".into()]);
        assert_eq!(snapshot(&options(endpoint), &fake).await.0.is_err(), raises);
    }
}
#[tokio::test(start_paused = true)]
async fn test_follow_resumes_and_cancels_streams() {
    let fake = fake(vec![
        Script {
            events: vec![(Some("cursor1"), old("first"))],
            ..Script::default()
        },
        Script {
            pending: true,
            ..script("second")
        },
    ]);
    let (result, text) = run(
        &Options {
            follow: true,
            ..options(false)
        },
        &fake,
        Duration::from_millis(80),
    )
    .await;
    assert!(result.is_none());
    assert_eq!(text, "[container] first\n[container] second\n");
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1.last_event_id, None);
    assert_eq!(calls[1].1.last_event_id.as_deref(), Some("cursor1"));
    assert!(calls.iter().all(|(_, o)| o.idle_timeout.is_none()));
    assert_eq!(*fake.closed.lock().unwrap(), ["pod1", "pod1"]);
}
#[tokio::test(start_paused = true)]
async fn test_follow_attaches_new_workers() {
    let fake = fake(vec![Script {
        pending: true,
        ..script("new")
    }]);
    fake.workers
        .lock()
        .unwrap()
        .extend([vec![], vec!["new".into()]]);
    let (result, text) = run(
        &Options {
            follow: true,
            ..options(true)
        },
        &fake,
        Duration::from_millis(80),
    )
    .await;
    assert!(result.is_none());
    assert_eq!(text, "waiting for workers ...\nnew [container] new\n");
    assert_eq!(*fake.closed.lock().unwrap(), ["new"]);
}
#[tokio::test(start_paused = true)]
async fn test_follow_retries_transient_http_errors() {
    for endpoint in [false, true] {
        for code in [429, 500, 502, 503, 599] {
            let companion_started = Arc::new(tokio::sync::Notify::new());
            let mut scripts = vec![Script {
                events: vec![(Some("cursor1"), old("first"))],
                wait: endpoint.then(|| companion_started.clone()),
                ..Script::default()
            }];
            if endpoint {
                scripts.push(Script {
                    pending: true,
                    signal: Some(companion_started),
                    ..script("companion")
                });
            }
            scripts.extend([
                Script {
                    error: Some(status(code)),
                    ..Script::default()
                },
                Script {
                    pending: true,
                    ..script("resumed")
                },
            ]);
            let fake = fake(scripts);
            fake.workers
                .lock()
                .unwrap()
                .push_back(vec!["pod1".into(), "pod2".into()]);
            let (result, text) = run(
                &Options {
                    follow: true,
                    as_json: true,
                    ..options(endpoint)
                },
                &fake,
                Duration::from_millis(80),
            )
            .await;
            assert!(result.is_none());
            let calls = fake.calls.lock().unwrap();
            let cursors: Vec<_> = calls
                .iter()
                .filter(|(w, _)| w == "pod1")
                .map(|(_, o)| o.last_event_id.as_deref())
                .collect();
            assert_eq!(cursors, [None, Some("cursor1"), Some("cursor1")]);
            let rows: Vec<Value> = text
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();
            let lines: Vec<_> = rows
                .iter()
                .filter(|r| r["line"] != "companion")
                .map(|r| r["line"].as_str().unwrap())
                .collect();
            assert_eq!(lines, ["first", "resumed"]);
            assert_eq!(rows.len(), if endpoint { 3 } else { 2 });
            assert_eq!(
                rows.iter()
                    .map(|r| r["line"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                if endpoint {
                    vec!["companion", "first", "resumed"]
                } else {
                    vec!["first", "resumed"]
                }
            );
            let closed = fake.closed.lock().unwrap();
            assert_eq!(closed.iter().filter(|w| *w == "pod1").count(), 3);
            assert_eq!(
                closed.iter().filter(|w| *w == "pod2").count(),
                usize::from(endpoint)
            );
        }
    }
}
#[tokio::test(start_paused = true)]
async fn test_http_errors_that_must_surface() {
    for endpoint in [false, true] {
        for (follow, code) in [
            (true, 400),
            (true, 401),
            (true, 403),
            (false, 429),
            (false, 500),
            (false, 503),
        ] {
            let fake = fake(vec![Script {
                error: Some(status(code)),
                ..Script::default()
            }]);
            fake.workers.lock().unwrap().push_back(vec!["pod1".into()]);
            assert_eq!(
                snapshot(
                    &Options {
                        follow,
                        ..options(endpoint)
                    },
                    &fake
                )
                .await
                .0
                .unwrap_err()
                .to_string(),
                "denied"
            );
        }
    }
}

#[tokio::test]
async fn snapshot_sorts_raw_timestamps_and_preserves_worker_override() {
    let mut overridden = event("offset first", "2026-01-01T01:00:00+02:00");
    overridden["worker"] = json!("override");
    let fake = fake(vec![Script {
        events: vec![
            (None, overridden),
            (None, event("lexically first", "2026-01-01T00:00:00Z")),
        ],
        ..Script::default()
    }]);
    fake.workers
        .lock()
        .unwrap()
        .push_back(vec!["a".into(), "a".into()]);
    let (result, text) = snapshot(
        &Options {
            as_json: true,
            ..options(true)
        },
        &fake,
    )
    .await;
    result.unwrap();
    let rows: Vec<Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(rows[0]["line"], "lexically first");
    assert_eq!(rows[1]["worker"], "override");
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn snapshot_error_discards_backfill_and_drops_other_streams() {
    let fake = fake(vec![
        Script {
            pending: true,
            ..script("buffered")
        },
        Script {
            error: Some(status(401)),
            delay: Duration::from_millis(5),
            ..Script::default()
        },
    ]);
    fake.workers
        .lock()
        .unwrap()
        .push_back(vec!["a".into(), "b".into()]);
    let (result, text) = snapshot(&options(true), &fake).await;
    assert!(result.is_err());
    assert_eq!(text, "");
    assert_eq!(fake.closed.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn follow_reattaches_disappeared_workers_and_grows_prefix_width() {
    let fake = fake(vec![
        Script {
            error: Some(status(404)),
            ..Script::default()
        },
        Script {
            pending: true,
            ..script("again")
        },
        Script {
            pending: true,
            ..script("new")
        },
    ]);
    fake.workers.lock().unwrap().extend([
        vec!["a".into()],
        vec!["a".into(), "longer".into(), "a".into()],
    ]);
    let (result, text) = run(
        &Options {
            follow: true,
            ..options(true)
        },
        &fake,
        Duration::from_millis(80),
    )
    .await;
    assert!(result.is_none());
    assert!(text.contains("a      [container] again\n"));
    assert!(text.contains("longer [container] new\n"));
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
    assert_eq!(fake.closed.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn follow_retries_connection_failures() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = reqwest::Client::new()
        .get(format!("http://{address}"))
        .send()
        .await
        .unwrap_err();
    assert!(error.is_connect());
    let fake = fake(vec![
        Script {
            error: Some(error.into()),
            ..Script::default()
        },
        Script {
            pending: true,
            ..script("resumed")
        },
    ]);
    tokio::time::pause();
    let (result, text) = run(
        &Options {
            follow: true,
            ..options(false)
        },
        &fake,
        Duration::from_millis(80),
    )
    .await;
    assert!(result.is_none());
    assert_eq!(text, "[container] resumed\n");
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
}

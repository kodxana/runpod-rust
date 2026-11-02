use super::super::interaction::tests::Fake;
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[derive(Default)]
struct Clock {
    elapsed: f64,
    sleeps: Vec<f64>,
    interrupt: Option<usize>,
}
impl watch::Clock for Clock {
    fn now(&self) -> f64 {
        self.elapsed
    }
    async fn sleep(&mut self, seconds: f64) -> CliResult<bool> {
        self.sleeps.push(seconds);
        self.elapsed += seconds;
        Ok(self.interrupt == Some(self.sleeps.len()))
    }
}
async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../tests/fixtures/endpoint_cli.json")).unwrap();
    let selected: Vec<_> = cases.iter().filter(|c| c["name"] == name).collect();
    assert!(!selected.is_empty());
    for case in selected {
        let server = MockServer::start().await;
        let replies = case["requests"].as_array().unwrap().clone();
        let index = Arc::new(AtomicUsize::new(0));
        Mock::given(any())
            .respond_with(move |_: &Request| {
                replies
                    .get(index.fetch_add(1, Ordering::SeqCst))
                    .map_or_else(
                        || ResponseTemplate::new(500),
                        |reply| {
                            ResponseTemplate::new(
                                u16::try_from(reply["status"].as_u64().unwrap()).unwrap(),
                            )
                            .set_body_json(&reply["response"])
                        },
                    )
            })
            .mount(&server)
            .await;
        let client = Client::with_config(crate::ClientConfig {
            api_base_url: server.uri(),
            ..crate::ClientConfig::new("fixture-key")
        })
        .unwrap();
        let mut clock = Clock {
            interrupt: case["interrupt_at_sleep"]
                .as_u64()
                .map(|v| usize::try_from(v).unwrap()),
            ..Clock::default()
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut streams = Streams::new(&mut stdout, &mut stderr, false);
        if case["helper"] == "follow" {
            watch::follow(
                &client,
                "ep1",
                &case["previous"],
                case["timeout"].as_f64().unwrap(),
                &mut clock,
                &mut streams,
                Destination::Out,
            )
            .await
            .unwrap();
            assert_eq!(
                String::from_utf8(stdout)
                    .unwrap()
                    .split_whitespace()
                    .collect::<Vec<_>>(),
                case["stdout"]
                    .as_str()
                    .unwrap()
                    .split_whitespace()
                    .collect::<Vec<_>>(),
                "{case}"
            );
        } else {
            let directory = tempfile::tempdir().unwrap();
            let file = directory.path().join("document.json");
            if let Some(document) = case["document"].as_str() {
                std::fs::write(&file, document).unwrap();
            }
            let args: Vec<_> = std::iter::once("endpoint".into())
                .chain(case["args"].as_array().unwrap().iter().map(|v| {
                    if v == "{document}" {
                        file.to_string_lossy().into_owned()
                    } else {
                        v.as_str().unwrap().into()
                    }
                }))
                .collect();
            let parsed = command().try_get_matches_from(args).unwrap();
            let options = Options::read(&parsed).unwrap();
            let input: Vec<_> = case["input"].as_str().unwrap_or_default().lines().collect();
            let mut ui = Fake {
                interactive: case["interactive"].as_bool().unwrap_or(false)
                    || case["confirm_interactive"].as_bool().unwrap_or(false),
                answer: input.last().is_some_and(|v| *v == "y"),
                retries: input.iter().map(|v| *v == "y").collect(),
                edits: case["edits"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["result"].as_str().map(str::to_owned))
                    .collect(),
                ..Fake::default()
            };
            if let Some(pick) = case["picks"].as_array().unwrap().first() {
                ui.selected = pick["choices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .position(|choice| choice[0] == pick["selected"]);
            }
            let result = options
                .execute_with_clock(&client, &mut ui, &mut streams, &mut clock)
                .await;
            assert_eq!(
                i64::from(result.as_ref().err().map_or(0, CliError::code)),
                case["code"].as_i64().unwrap(),
                "{case}\n{result:?}"
            );
            let stdout = String::from_utf8(stdout).unwrap();
            let stderr = String::from_utf8(stderr).unwrap();
            for (actual, expected) in ui.edit_inputs.iter().zip(case["edits"].as_array().unwrap()) {
                assert_eq!(actual, expected["text"].as_str().unwrap());
            }
            assert_eq!(
                ui.edit_inputs.len(),
                case["edits"].as_array().unwrap().len()
            );
            for success in case["success"].as_array().unwrap() {
                assert!(
                    format!("{stdout}\n{stderr}").contains(success.as_str().unwrap()),
                    "{stdout}\n{stderr}"
                );
            }
            for (actual, expected) in ui.selections.iter().zip(case["picks"].as_array().unwrap()) {
                assert_eq!(actual.0, expected["message"].as_str().unwrap());
                assert_eq!(
                    actual.1,
                    expected["choices"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|c| c[1].as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                );
            }
            if name == "test_workers_watch" {
                if options.json {
                    let lines = stdout
                        .lines()
                        .map(|line| serde_json::from_str::<Value>(line).unwrap())
                        .collect::<Vec<_>>();
                    let expected = case["stdout"]
                        .as_str()
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str::<Value>(line).unwrap())
                        .collect::<Vec<_>>();
                    assert_eq!(lines, expected);
                } else {
                    assert!(stdout.contains("w1") && stdout.contains("1h02m"));
                }
            }
            if name == "test_edit_in_editor_reopens_on_error_then_follows" {
                assert!(stdout.contains("unknown field 'bogus'"));
            }
            if name == "test_edit_in_editor_gives_up_or_saves_nothing" && result.is_ok() {
                assert!(stdout.contains("no changes"));
            }
        }
        assert_eq!(json!(clock.sleeps), case["sleeps"], "{case}");
        let actual = server.received_requests().await.unwrap();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "{case}");
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual.method.as_str(), expected["method"].as_str().unwrap());
            assert_eq!(actual.url.path(), expected["path"].as_str().unwrap());
            let params: serde_json::Map<String, Value> = actual
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), json!(v)))
                .collect();
            assert_eq!(Value::Object(params), expected["params"]);
            let body = if actual.body.is_empty() {
                Value::Null
            } else {
                actual.body_json().unwrap()
            };
            assert_eq!(body, expected["body"]);
        }
    }
}
macro_rules! source_test { ($($name:ident),* $(,)?)=>{$(#[tokio::test] async fn $name() { verify(stringify!($name)).await; })*}; }
source_test!(
    test_get_without_id_picks_interactively,
    test_edit_in_editor_reopens_on_error_then_follows,
    test_edit_in_editor_gives_up_or_saves_nothing,
    test_delete_confirms,
    test_workers_watch,
    test_follow_rollout_without_new_release,
    test_follow_rollout_waits_for_delayed_release,
    test_follow_rollout_reports_unobserved_release_at_deadline,
    test_follow_rollout_times_out_and_detaches
);

#[tokio::test]
async fn baseline_transport_timeout_still_applies_update() {
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    let server = MockServer::start().await;
    for method_name in ["GET", "PATCH"] {
        Mock::given(method(method_name))
            .and(path("/v2/serverless/ep1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"name":"endpoint","image":"new"})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/v2/serverless/ep1/releases"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({}))
                .set_delay(Duration::from_secs(5)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::with_config(crate::ClientConfig {
        api_base_url: server.uri(),
        timeout: Duration::from_secs(1),
        ..crate::ClientConfig::new("fixture-key")
    })
    .unwrap();
    let parsed = command()
        .try_get_matches_from([
            "endpoint", "update", "ep1", "--image", "new", "--wait", "--json",
        ])
        .unwrap();
    let options = Options::read(&parsed).unwrap();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    options
        .execute(
            &client,
            &mut Fake::default(),
            &mut Streams::new(&mut stdout, &mut stderr, false),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&stdout).unwrap()["image"],
        "new"
    );
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("cannot follow rollout")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

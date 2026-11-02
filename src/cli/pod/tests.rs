use super::*;
use crate::Value;
use crate::cli::interaction::tests::Fake;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[derive(Default)]
struct Clock {
    sleeps: Vec<Duration>,
}
impl wait::Clock for Clock {
    fn now(&self) -> Duration {
        Duration::ZERO
    }
    async fn sleep(&mut self, duration: Duration) {
        self.sleeps.push(duration);
    }
}
async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../tests/fixtures/pod_cli.json")).unwrap();
    let selected: Vec<_> = cases.iter().filter(|case| case["name"] == name).collect();
    assert!(!selected.is_empty(), "missing source cases for {name}");
    for case in selected {
        let server = MockServer::start().await;
        let replies = case["requests"].as_array().unwrap().clone();
        let next = Arc::new(AtomicUsize::new(0));
        Mock::given(any())
            .respond_with(move |_: &Request| {
                replies
                    .get(next.fetch_add(1, Ordering::SeqCst))
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
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let matches = command()
            .try_get_matches_from(std::iter::once("pod").chain(args.iter().copied()))
            .unwrap();
        let mut ui = Fake {
            interactive: case["interactive"].as_bool().unwrap(),
            ..Fake::default()
        };
        if let Some(choices) = case["choices"].as_array() {
            ui.selected = choices.iter().position(|c| c[0] == case["picked"]);
        }
        if let Some(input) = case["input"].as_str() {
            ui.answer = input.trim() == "y";
            ui.texts.push_back(input.trim().into());
        }
        let mut waiter = wait::Waiter {
            clock: Clock::default(),
            timeout: if case["wait_timeout"].as_i64().unwrap() < 0 {
                Duration::ZERO
            } else {
                Duration::from_secs(600)
            },
        };
        let mut out = vec![];
        let mut err = vec![];
        let result = async {
            let options = Options::read(&matches)?;
            options.validate_interaction(&ui)?;
            options
                .execute(
                    &client,
                    &mut ui,
                    &mut Streams::new(&mut out, &mut err, false),
                    &mut waiter,
                )
                .await
        }
        .await;
        assert_eq!(
            result.as_ref().map_or_else(CliError::code, |()| 0),
            u8::try_from(case["code"].as_u64().unwrap()).unwrap(),
            "{name} {args:?}: {result:?}"
        );
        let text = String::from_utf8(out).unwrap();
        let stderr = String::from_utf8(err).unwrap();
        if let Err(error) = result {
            let expected = case["stderr"].as_str().unwrap();
            if expected.contains("Aborted!") {
                assert!(matches!(error, CliError::Aborted));
            } else {
                let expected = expected.rsplit_once("Error: ").unwrap().1.trim();
                let expected = expected.split(" after -1s").next().unwrap();
                assert!(
                    error.to_string().contains(expected),
                    "{name}: {error:?} should contain {expected:?}"
                );
            }
        } else if args.contains(&"--json") {
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap(),
                serde_json::from_str::<Value>(case["stdout"].as_str().unwrap()).unwrap(),
                "{name}"
            );
        } else if !case["fields"].as_array().unwrap().is_empty() {
            let fields = case["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| (v[0].as_str().unwrap().into(), v[1].as_str().unwrap().into()))
                .collect::<Vec<_>>();
            assert!(
                text.contains(&output::fields(&fields)),
                "{name} fields: {text:?}"
            );
            assert!(!text.contains("redacted-test-value"));
        } else if let Some(headers) = case["headers"].as_array() {
            let headers = headers
                .iter()
                .map(|v| v.as_str().unwrap().into())
                .collect::<Vec<_>>();
            let rows = case["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    row.as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().into())
                        .collect()
                })
                .collect::<Vec<_>>();
            assert_eq!(
                text,
                output::table(&headers, &rows, case["empty"].as_str().unwrap()),
                "{name}"
            );
        } else if case["code"] == 0 && matches!(args[0], "terminate" | "delete") {
            assert_eq!(text, "terminated pod pod123\n");
        }
        for message in case["messages"].as_array().unwrap() {
            let actual = if message["stderr"] == true {
                &stderr
            } else {
                &text
            };
            assert!(
                actual.contains(message["message"].as_str().unwrap()),
                "{name}: missing {message} in {actual:?}"
            );
        }
        if let Some(choices) = case["choices"].as_array() {
            let labels = choices
                .iter()
                .map(|v| v[1].as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(ui.selections[0].0, "pod");
            assert_eq!(ui.selections[0].1, labels);
        }
        if !ui.confirmations.is_empty() {
            assert_eq!(
                ui.confirmations,
                ["terminate pod pod123 (trainer)? its disk and persistent volume are deleted"]
            );
        }
        let requests = server.received_requests().await.unwrap();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(requests.len(), expected.len(), "{name} {args:?}");
        for (actual, expected) in requests.iter().zip(expected) {
            assert_eq!(
                actual.method.as_str(),
                expected["method"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(
                actual.url.path(),
                expected["path"].as_str().unwrap(),
                "{name}"
            );
            assert_eq!(actual.headers["authorization"], "Bearer fixture-key");
            let params: serde_json::Map<String, Value> = actual
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
                .collect();
            assert_eq!(Value::Object(params), expected["params"], "{name}");
            let body = if actual.body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&actual.body).unwrap()
            };
            assert_eq!(body, expected["body"], "{name}");
        }
        assert!(
            waiter
                .clock
                .sleeps
                .iter()
                .all(|d| *d == Duration::from_secs(5))
        );
    }
}
macro_rules! source_cases {
    ($($name:ident => $source:literal),* $(,)?) => {$ (
        #[tokio::test]
        async fn $name() {verify($source).await;}
    )*};
}

#[test]
fn native_cli_future_is_send() {
    fn assert_send<T: Send>(_: T) {}
    assert_send(crate::cli::run(
        ["rp", "pod", "list"].map(std::ffi::OsString::from),
    ));
}

#[tokio::test]
async fn malformed_update_state_fails_before_patch() {
    for value in [json!("bad"), json!([{}]), json!(1)] {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(value))
            .mount(&server)
            .await;
        let client = Client::with_config(crate::ClientConfig {
            api_base_url: server.uri(),
            ..crate::ClientConfig::new("fixture-key")
        })
        .unwrap();
        let args = command()
            .try_get_matches_from(["pod", "update", "pod123", "--env", "K=V"])
            .unwrap();
        let mut out = vec![];
        let mut err = vec![];
        let result = Options::read(&args)
            .unwrap()
            .execute(
                &client,
                &mut Fake::default(),
                &mut Streams::new(&mut out, &mut err, false),
                &mut wait::Waiter::default(),
            )
            .await;
        assert!(result.is_err());
        let calls = server.received_requests().await.unwrap();
        assert_eq!(
            calls.len(),
            1,
            "invalid current settings must not produce a PATCH"
        );
        assert_eq!(calls[0].method.as_str(), "GET");
    }
}

#[tokio::test]
async fn logs_accepts_a_picked_pod_and_termination_filters_choices() {
    use wiremock::matchers::path;
    let server = MockServer::start().await;
    let pods = json!({"pods":[{"id":"not-terminable","name":"busy","status":"RUNNING","actions":["stop"]},{"id":"chosen","name":"box","status":"RUNNING","actions":["terminate"]}],"pagination":{"hasNextPage":false}});
    Mock::given(path("/v2/pods"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pods))
        .mount(&server)
        .await;
    Mock::given(path("/v2/pods/chosen/logs")).respond_with(ResponseTemplate::new(200).set_body_raw("data: {\"source\":\"container\",\"line\":\"picked\",\"ts\":\"2020-01-01T00:00:00Z\"}\n\n","text/event-stream")).expect(1).mount(&server).await;
    let client = Client::with_config(crate::ClientConfig {
        api_base_url: server.uri(),
        ..crate::ClientConfig::new("fixture-key")
    })
    .unwrap();
    let mut ui = Fake::default();
    assert_eq!(
        resolve_id(&client, None, Some(Action::Terminate), &mut ui)
            .await
            .unwrap(),
        "chosen"
    );
    assert_eq!(ui.selections[0].1, ["box  running"]);
    ui.selected = Some(1);
    let matches = command().try_get_matches_from(["pod", "logs"]).unwrap();
    let mut out = vec![];
    let mut err = vec![];
    show_logs(
        matches.subcommand_matches("logs").unwrap(),
        client,
        &mut ui,
        &mut Streams::new(&mut out, &mut err, false),
    )
    .await
    .unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "[container] picked\n");
    assert_eq!(ui.selections[1].1, ["busy  running", "box  running"]);
}

source_cases! {
    test_actions_action => "TestActions.test_action",
    test_actions_api_error => "TestActions.test_api_error",
    test_actions_picker_offers_pods_that_allow_the_action => "TestActions.test_picker_offers_pods_that_allow_the_action",
    test_actions_restart_wait_rejects_recent_pre_action_runtime => "TestActions.test_restart_wait_rejects_recent_pre_action_runtime",
    test_actions_restart_wait_rejects_recent_runtime_with_new_timestamp => "TestActions.test_restart_wait_rejects_recent_runtime_with_new_timestamp",
    test_actions_restart_wait_rejects_stale_runtime_with_new_timestamp => "TestActions.test_restart_wait_rejects_stale_runtime_with_new_timestamp",
    test_actions_restart_wait_requires_new_start_timestamp => "TestActions.test_restart_wait_requires_new_start_timestamp",
    test_actions_start_wait => "TestActions.test_start_wait",
    test_create_container_every_container_field => "TestCreateContainer.test_every_container_field",
    test_create_container_network_volume_sets_its_data_center => "TestCreateContainer.test_network_volume_sets_its_data_center",
    test_create_container_regions_and_data_centers_merge => "TestCreateContainer.test_regions_and_data_centers_merge",
    test_create_cpu_vcpu_resolution => "TestCreateCpu.test_vcpu_resolution",
    test_create_gpu_exact_cuda => "TestCreateGpu.test_exact_cuda",
    test_create_gpu_gpu_settings => "TestCreateGpu.test_gpu_settings",
    test_create_gpu_selects_stock_then_price_within_placement => "TestCreateGpu.test_selects_stock_then_price_within_placement",
    test_create_gpu_ssh_without_keys_warns => "TestCreateGpu.test_ssh_without_keys_warns",
    test_create_gpu_ticket_example => "TestCreateGpu.test_ticket_example",
    test_create_gpu_unique_match_used_directly => "TestCreateGpu.test_unique_match_used_directly",
    test_create_validation_malformed_command_quoting => "TestCreateValidation.test_malformed_command_quoting",
    test_create_validation_prompts_for_name => "TestCreateValidation.test_prompts_for_name",
    test_create_validation_resolution_errors => "TestCreateValidation.test_resolution_errors",
    test_create_validation_usage_errors => "TestCreateValidation.test_usage_errors",
    test_get_fields_and_connection_info => "TestGet.test_fields_and_connection_info",
    test_get_json => "TestGet.test_json",
    test_get_missing_id_without_tty => "TestGet.test_missing_id_without_tty",
    test_get_other_shapes => "TestGet.test_other_shapes",
    test_get_picker => "TestGet.test_picker",
    test_get_status_follows_container => "TestGet.test_status_follows_container",
    test_list_api_error_is_clean => "TestList.test_api_error_is_clean",
    test_list_cluster_pods_and_empty => "TestList.test_cluster_pods_and_empty",
    test_list_filters => "TestList.test_filters",
    test_list_filters_use_displayed_status => "TestList.test_filters_use_displayed_status",
    test_list_table => "TestList.test_table",
    test_terminate_confirmation_identifies_pod => "TestTerminate.test_confirmation_identifies_pod",
    test_terminate_interactive_confirm => "TestTerminate.test_interactive_confirm",
    test_terminate_lookup_failure_prevents_deletion => "TestTerminate.test_lookup_failure_prevents_deletion",
    test_terminate_needs_confirmation => "TestTerminate.test_needs_confirmation",
    test_terminate_with_yes => "TestTerminate.test_with_yes",
    test_terminate_yes_skips_name_lookup => "TestTerminate.test_yes_skips_name_lookup",
    test_update_env_edits_existing => "TestUpdate.test_env_edits_existing",
    test_update_malformed_command_quoting => "TestUpdate.test_malformed_command_quoting",
    test_update_mount_path_defaults_to_current => "TestUpdate.test_mount_path_defaults_to_current",
    test_update_nothing_to_update => "TestUpdate.test_nothing_to_update",
    test_update_null_mounts => "TestUpdate.test_null_mounts",
    test_update_simple_fields_skip_the_read => "TestUpdate.test_simple_fields_skip_the_read",
    test_wait_create_waits_until_running => "TestWait.test_create_waits_until_running",
    test_wait_error_status_fails => "TestWait.test_error_status_fails",
    test_wait_timeout => "TestWait.test_timeout",
}

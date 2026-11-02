use runpod::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[test]
fn argument_errors_precede_missing_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    for (args, code, message) in [
        (vec!["get"], 2, "pass an ENDPOINT_ID"),
        (vec!["get", ""], 2, "pass an ENDPOINT_ID"),
        (vec!["edit"], 2, "pass an ENDPOINT_ID"),
        (vec!["edit", ""], 2, "pass an ENDPOINT_ID"),
        (
            vec!["edit", "--file", missing.to_str().unwrap()],
            2,
            "Invalid value for --file",
        ),
        (vec!["edit", "ep1"], 1, "No API key provided"),
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .arg("endpoint")
            .args(&args)
            .env_remove("RUNPOD_API_KEY")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(result.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(result.stdout.is_empty());
    }
}

async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/endpoint_cli.json")).unwrap();
    let mut checked = 0;
    for case in cases.iter().filter(|case| {
        case["name"] == name
            && case["interactive"] == false
            && (case["input"].is_null() || case["name"] == "stdin_edit_document")
            && case.get("interrupt_at_sleep").is_none()
    }) {
        checked += 1;
        let server = MockServer::start().await;
        let replies = case["requests"].as_array().unwrap().clone();
        let index = Arc::new(AtomicUsize::new(0));
        let cursor = index.clone();
        Mock::given(any())
            .respond_with(move |_: &Request| {
                let index = cursor.fetch_add(1, Ordering::SeqCst);
                replies.get(index).map_or_else(
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
        let directory = tempfile::tempdir().unwrap();
        let document = directory.path().join("edit.json");
        if let Some(text) = case["document"].as_str() {
            std::fs::write(&document, text).unwrap();
        }
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| {
                if arg == "{document}" || arg == "{missing-document}" {
                    document.as_os_str().to_owned()
                } else {
                    arg.as_str().unwrap().into()
                }
            })
            .collect();
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .arg("endpoint")
            .args(args)
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = case["input"].as_str() {
            use tokio::io::AsyncWriteExt;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .await
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let result = child.wait_with_output().await.unwrap();
        let stdout = String::from_utf8(result.stdout).unwrap();
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert_eq!(
            result.status.code().map(i64::from),
            case["code"].as_i64(),
            "{case}\n{stdout}\n{stderr}"
        );
        let combined = format!("{stdout}\n{stderr}");
        if case["code"] != 0 {
            let expected = case["stderr"]
                .as_str()
                .unwrap()
                .split("Error: ")
                .last()
                .unwrap()
                .trim();
            if name == "unreadable_edit_document" {
                assert!(stderr.contains("edit.json"), "{stderr}");
            } else {
                assert!(stderr.contains(expected), "{case}\n{stderr}");
            }
        } else if let Ok(expected) = serde_json::from_str::<Value>(case["stdout"].as_str().unwrap())
        {
            assert_eq!(
                serde_json::from_str::<Value>(&stdout).unwrap(),
                expected,
                "{case}"
            );
        }
        for success in case["success"].as_array().unwrap() {
            assert!(
                combined.contains(success.as_str().unwrap()),
                "{case}\n{combined}"
            );
        }
        for fields in case["fields"].as_array().unwrap() {
            for field in fields.as_array().unwrap() {
                assert!(
                    stdout
                        .lines()
                        .any(|line| line.contains(field[0].as_str().unwrap())
                            && line.contains(field[1].as_str().unwrap())),
                    "{case}\n{stdout}"
                );
            }
        }
        for table in case["tables"].as_array().unwrap() {
            let rows = table["cells"].as_array().unwrap();
            if rows.is_empty() {
                assert!(
                    stdout.contains(table["empty"].as_str().unwrap()),
                    "{stdout}"
                );
            } else {
                for row in rows {
                    assert!(
                        stdout.lines().any(|line| row
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|cell| line.contains(cell.as_str().unwrap()))),
                        "{case}\n{stdout}"
                    );
                }
            }
        }
        if let Some(summary) = case["worker_summary"].as_str() {
            assert!(stdout.contains(summary), "{case}\n{stdout}");
        }
        if name == "test_wait_json_keeps_rollout_messages_on_stderr" {
            assert!(
                stderr.contains("rolling out") && stderr.contains("release v4 is live"),
                "{stderr}"
            );
        }
        if name == "test_release_baseline_failure_does_not_block_update" {
            assert!(stderr.contains("cannot follow rollout"), "{stderr}");
        }
        if name == "test_edit_requires_yes_without_a_terminal" {
            assert!(
                stdout.contains("patch:") && stdout.contains("renamed"),
                "{stdout}"
            );
        }
        if name == "partial_output_survives_rollout_failure" {
            assert_eq!(
                serde_json::from_str::<Value>(&stdout).unwrap()["image"],
                "new"
            );
            assert!(stderr.contains("rollout read failed"));
        }
        if name == "rollout_timeout_is_successful_detachment" {
            assert!(stderr.contains("no new release observed after 0s"));
        }
        let actual = server.received_requests().await.unwrap();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "{case}");
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual.method.as_str(), expected["method"].as_str().unwrap());
            assert_eq!(actual.url.path(), expected["path"].as_str().unwrap());
            assert_eq!(actual.headers["authorization"], "Bearer fixture-key");
            let query: serde_json::Map<String, Value> = actual
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), json!(v)))
                .collect();
            assert_eq!(Value::Object(query), expected["params"]);
            let body = if actual.body.is_empty() {
                Value::Null
            } else {
                actual.body_json().unwrap()
            };
            assert_eq!(body, expected["body"], "{case}");
        }
    }
    assert!(checked > 0, "no process cases for {name}");
}
macro_rules! source_test { ($($name:ident),* $(,)?) => {$( #[tokio::test] async fn $name() { verify(stringify!($name)).await; } )*}; }
source_test!(
    test_list_shows_ids_and_summaries,
    test_list_filters_by_name_glob_and_type,
    test_get_shows_fields,
    test_get_without_id_needs_a_terminal,
    test_api_errors_are_clean,
    test_create_queue_endpoint_from_ticket,
    test_create_load_balancer_endpoint_from_ticket,
    test_create_cpu_endpoint_from_ticket,
    test_create_maps_container_and_placement_flags,
    test_create_exact_cuda_and_cpu_vcpu_suffix,
    test_create_rejects_bad_flags,
    test_update_merges_env_and_workers,
    test_update_gpu_replaces_pools_and_exclusions_together,
    test_update_cpu_vcpus_and_mounts,
    test_update_rejects_bad_flags,
    test_update_json_and_wait_follows_rollout,
    test_wait_json_keeps_rollout_messages_on_stderr,
    test_release_baseline_failure_does_not_block_update,
    test_registry_selection_and_clearing,
    test_unset_missing_environment_variable_is_noop,
    test_invalid_container_quoting_is_usage_error,
    test_nullable_configuration_maps,
    test_edit_file_shows_patch_and_applies,
    test_runtime_only_changes_skip_rollout,
    test_mixed_edit_waits_for_delayed_release,
    test_edit_without_changes,
    test_edit_requires_yes_without_a_terminal,
    test_edit_needs_terminal_or_file,
    test_scale_sends_workers_and_scaler,
    test_scale_rejects_nothing_and_lb_queue_delay,
    test_delete_confirms,
    test_workers_table_and_json,
    test_releases,
    additional_glob_edges,
    partial_output_survives_rollout_failure,
    rollout_timeout_is_successful_detachment,
    stdin_edit_document,
    encoded_endpoint_id,
    unreadable_edit_document
);

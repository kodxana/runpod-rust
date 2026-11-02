use runpod::Value;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[tokio::test]
async fn noninteractive_source_cases_reach_native_cli() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/pod_cli.json")).unwrap();
    let mut checked = 0;
    for case in cases {
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        if case["interactive"] == true || args.contains(&"--wait") {
            continue;
        }
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
        let result = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .arg("pod")
            .args(&args)
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .unwrap();
        assert_eq!(
            result.status.code().map(i64::from),
            case["code"].as_i64(),
            "{} {args:?}: {}",
            case["name"],
            String::from_utf8_lossy(&result.stderr)
        );
        let out = String::from_utf8(result.stdout)
            .unwrap()
            .replace("\r\n", "\n");
        let err = String::from_utf8(result.stderr).unwrap();
        if result.status.success() {
            if args.contains(&"--json") {
                assert_eq!(
                    serde_json::from_str::<Value>(&out).unwrap(),
                    serde_json::from_str::<Value>(case["stdout"].as_str().unwrap()).unwrap(),
                    "{}",
                    case["name"]
                );
            } else {
                for field in case["fields"].as_array().unwrap() {
                    assert!(
                        out.lines()
                            .any(|line| line.starts_with(field[0].as_str().unwrap())
                                && line.ends_with(field[1].as_str().unwrap())),
                        "{} missing {field} in {out:?}",
                        case["name"]
                    );
                }
                for row in case["rows"].as_array().unwrap() {
                    for value in row.as_array().unwrap() {
                        assert!(
                            out.contains(value.as_str().unwrap()),
                            "{} missing {value}",
                            case["name"]
                        );
                    }
                }
                if case["headers"].is_array() && case["rows"].as_array().unwrap().is_empty() {
                    assert!(out.contains(case["empty"].as_str().unwrap()));
                }
            }
        } else {
            let expected = case["stderr"]
                .as_str()
                .unwrap()
                .rsplit_once("Error: ")
                .unwrap()
                .1
                .trim();
            assert!(
                err.contains(expected),
                "{}: {err:?} should contain {expected:?}",
                case["name"]
            );
        }
        for message in case["messages"].as_array().unwrap() {
            assert!(
                (if message["stderr"] == true {
                    &err
                } else {
                    &out
                })
                .contains(message["message"].as_str().unwrap())
            );
        }
        let actual = server.received_requests().await.unwrap();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "{}", case["name"]);
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual.method.as_str(), expected["method"].as_str().unwrap());
            assert_eq!(actual.url.path(), expected["path"].as_str().unwrap());
            assert_eq!(actual.headers["authorization"], "Bearer fixture-key");
            let params: serde_json::Map<String, Value> = actual
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
                .collect();
            assert_eq!(Value::Object(params), expected["params"]);
            let body = if actual.body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&actual.body).unwrap()
            };
            assert_eq!(body, expected["body"]);
        }
        checked += 1;
    }
    assert_eq!(checked, 71);
}

#[test]
fn missing_ids_and_invalid_options_precede_credentials() {
    for (args, message) in [
        (vec!["get"], "pass a POD_ID"),
        (vec!["logs"], "pass a POD_ID"),
        (vec!["start"], "pass a POD_ID"),
        (vec!["stop"], "pass a POD_ID"),
        (vec!["restart"], "pass a POD_ID"),
        (vec!["terminate"], "pass a POD_ID"),
        (vec!["update"], "nothing to update"),
        (vec!["update", "--name", "new"], "pass a POD_ID"),
        (
            vec!["create", "--gpu", "4090", "--image", "x"],
            "pass a NAME",
        ),
        (
            vec![
                "create", "a", "--gpu", "4090", "--cpu", "cpu3c", "--image", "x",
            ],
            "use either --gpu or --cpu",
        ),
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .arg("pod")
            .args(args)
            .env_remove("RUNPOD_API_KEY")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains(message));
    }
}

#[test]
fn numeric_bounds_and_both_binary_aliases() {
    for executable in [env!("CARGO_BIN_EXE_rp"), env!("CARGO_BIN_EXE_runpod")] {
        for (flag, value) in [
            ("--disk", "0"),
            ("--gpu-count", "0"),
            ("--vcpu", "1"),
            ("--min-ram-per-gpu", "0"),
            ("--min-vcpu-per-gpu", "0"),
        ] {
            let result = std::process::Command::new(executable)
                .args([
                    "pod", "create", "a", "--gpu", "4090", "--image", "x", flag, value,
                ])
                .env_remove("RUNPOD_API_KEY")
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(2));
        }
        let result = std::process::Command::new(executable)
            .args(["pod", "--help"])
            .output()
            .unwrap();
        assert!(result.status.success());
        let output = String::from_utf8_lossy(&result.stdout);
        for command in [
            "list",
            "get",
            "create",
            "update",
            "start",
            "stop",
            "restart",
            "terminate",
            "delete",
            "logs",
        ] {
            assert!(output.contains(command));
        }
    }
}

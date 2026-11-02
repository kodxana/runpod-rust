use runpod::Value;
use std::fmt::Write;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[tokio::test]
async fn source_management_requests_reach_both_cli_aliases() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/app_cli.json")).unwrap();
    let mut checked = 0;
    for case in cases.iter().filter(|case| case["requests"].is_array()) {
        let server = MockServer::start().await;
        let replies = case["requests"].as_array().unwrap().clone();
        let next = Arc::new(AtomicUsize::new(0));
        Mock::given(any())
            .respond_with(move |_: &Request| {
                replies
                    .get(next.fetch_add(1, Ordering::SeqCst))
                    .map_or_else(
                        || ResponseTemplate::new(500),
                        |r| {
                            ResponseTemplate::new(
                                u16::try_from(r["status"].as_u64().unwrap()).unwrap(),
                            )
                            .set_body_json(&r["response"])
                        },
                    )
            })
            .mount(&server)
            .await;
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let binary = if checked % 2 == 0 {
            env!("CARGO_BIN_EXE_rp")
        } else {
            env!("CARGO_BIN_EXE_runpod")
        };
        let result = tokio::process::Command::new(binary)
            .args(args)
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .unwrap();
        let out = String::from_utf8(result.stdout)
            .unwrap()
            .replace("\r\n", "\n");
        let err = String::from_utf8(result.stderr)
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(
            result.status.code().map(i64::from),
            case["code"].as_i64(),
            "{}: {err}",
            case["name"]
        );
        let mut expected = String::new();
        for event in case["events"].as_array().unwrap() {
            let name = event["args"][0].as_str().unwrap_or_default();
            match event["event"].as_str().unwrap() {
                "deleting" => writeln!(expected, "  deleting {name}").unwrap(),
                "delete_failed" => writeln!(expected, " ! could not delete {name}").unwrap(),
                _ => (),
            }
        }
        for message in case["messages"].as_array().unwrap() {
            expected.push_str(message.as_str().unwrap());
            expected.push('\n');
        }
        assert_eq!(out, expected, "{}", case["name"]);
        assert_eq!(err, case["stderr"].as_str().unwrap(), "{}", case["name"]);
        let actual = server.received_requests().await.unwrap();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "{}", case["name"]);
        for (a, e) in actual.iter().zip(expected) {
            assert_eq!(a.method.as_str(), e["method"].as_str().unwrap());
            assert_eq!(a.url.path(), e["path"].as_str().unwrap());
            assert_eq!(a.headers["authorization"], "Bearer fixture-key");
            assert!(a.url.query().is_none());
            let body: Value = if a.body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&a.body).unwrap()
            };
            assert_eq!(body, e["body"], "{}", case["name"]);
        }
        checked += 1;
    }
    assert_eq!(checked, 15);
}

#[test]
fn argument_and_confirmation_errors_precede_credentials() {
    for args in [
        vec!["flash", "app", "delete", "demo"],
        vec!["flash", "app", "get"],
        vec!["app", "list"],
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(&args)
            .env_remove("RUNPOD_API_KEY")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2), "{args:?}");
        if args.last() == Some(&"demo") {
            assert!(String::from_utf8_lossy(&result.stderr).contains("--yes"));
        }
    }
}

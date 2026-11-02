use runpod::Value;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[tokio::test]
async fn source_cases_reach_both_cli_aliases() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/credential_cli.json")).unwrap();
    let mut checked = 0;
    for case in cases {
        if !case["prompts"].as_array().unwrap().is_empty()
            || !case["confirmations"].as_array().unwrap().is_empty()
        {
            continue;
        }
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
            .args(&args)
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
        if result.status.success() {
            let expected = case["messages"].as_array().unwrap().iter().fold(
                String::new(),
                |mut text, message| {
                    text.push_str(message.as_str().unwrap());
                    text.push('\n');
                    text
                },
            );
            assert_eq!(out, expected, "{}", case["name"]);
            assert_eq!(err, "");
        } else {
            assert!(
                err.contains(
                    case["stderr"]
                        .as_str()
                        .unwrap()
                        .rsplit_once("Error: ")
                        .unwrap()
                        .1
                        .trim()
                ),
                "{}: {err}",
                case["name"]
            );
        }
        assert!(!out.contains("never-print-me"));
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
            assert_eq!(body, e["body"]);
        }
        checked += 1;
    }
    assert_eq!(checked, 31);
}

#[test]
fn missing_input_and_confirmation_fail_before_credentials() {
    for (args, message) in [
        (vec!["registry", "add"], "pass a NAME"),
        (vec!["registry", "add", "r"], "--username"),
        (
            vec!["registry", "add", "r", "--username", "u"],
            "--password",
        ),
        (vec!["secret", "add"], "pass a NAME"),
        (vec!["secret", "add", "s"], "--value"),
        (vec!["secret", "delete", "s"], "--yes"),
        (vec!["registry", "delete", "r"], "--yes"),
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
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(message),
            "{args:?}"
        );
    }
}

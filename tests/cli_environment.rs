use runpod::Value;
use std::fmt::Write;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

#[tokio::test]
async fn source_management_requests_reach_both_cli_aliases() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/env_cli.json")).unwrap();
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
    assert_eq!(checked, 16);
}

#[tokio::test]
async fn current_project_resolves_unique_app_and_explicit_names_bypass_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"cli-discovery-fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[profile.dev]\ndebug=0\nincremental=false\n").unwrap();
    std::fs::write(root.join("src/main.rs"), r#"
fn main() {
    if std::env::var_os("RUNPOD_DISCOVERY_SCAN").is_some() {
        std::fs::copy("declarations.json", std::env::var_os("RUNPOD_DISCOVERY_OUTPUT").unwrap()).unwrap();
        return;
    }
    std::fs::write("local-entrypoint-ran", "unexpected").unwrap();
}
"#).unwrap();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(
            runpod::json!({"data":{"flashAppByName":{"id":"app1","flashEnvironments":[]}}}),
        ))
        .mount(&server)
        .await;
    let invocation = |args: &[&str]| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rp"));
        command
            .args(args)
            .current_dir(root)
            .env("RUNPOD_API_KEY", "fixture-key")
            .env("RUNPOD_API_BASE_URL", server.uri())
            .stdin(std::process::Stdio::null());
        command
    };
    for names in [vec!["native"], vec!["first", "second"], vec![]] {
        let apps: Vec<_> = names
            .iter()
            .map(|name| runpod::json!({"name":name,"environment":"default","resources":[]}))
            .collect();
        std::fs::write(
            root.join("declarations.json"),
            runpod::json!({"version":1,"apps":apps}).to_string(),
        )
        .unwrap();
        let result = invocation(&["flash", "env", "list"])
            .output()
            .await
            .unwrap();
        if names.len() == 1 {
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                String::from_utf8(result.stdout)
                    .unwrap()
                    .replace("\r\n", "\n"),
                "\n  no environments in native. run rp flash deploy\n\n"
            );
        } else {
            assert_eq!(result.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&result.stderr).contains("pass --app <name>"));
        }
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["variables"]["flashAppName"], "native");
    assert!(!root.join("local-entrypoint-ran").exists());
    std::fs::write(root.join("Cargo.toml"), "invalid TOML }").unwrap();
    let result = invocation(&["flash", "env", "list", "-a", "explicit"])
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("in explicit"));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[test]
fn destructive_confirmation_precedes_credentials() {
    for args in [
        vec!["flash", "env", "delete", "prod", "-a", "demo"],
        vec!["flash", "undeploy", "--app", "demo"],
    ] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_rp"))
            .args(args)
            .env_remove("RUNPOD_API_KEY")
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains("--yes"));
    }
}

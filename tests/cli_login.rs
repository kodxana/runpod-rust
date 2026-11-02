use runpod::{Value, credentials::CredentialStore, json};
use std::process::Stdio;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

fn command(binary: &str, home: &std::path::Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("login")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("RUNPOD_API_KEY", "existing-key-must-not-be-sent")
        .stdin(Stdio::null());
    command
}

#[tokio::test]
async fn direct_keys_preserve_profiles_without_authentication() {
    let home = tempfile::tempdir().unwrap();
    let store = CredentialStore::new(home.path().join(".runpod/config.toml"));
    store
        .set_credentials("keep", "other", runpod::credentials::Overwrite::Allow)
        .unwrap();
    for (binary, profile) in [
        (env!("CARGO_BIN_EXE_rp"), "default"),
        (env!("CARGO_BIN_EXE_runpod"), "staging"),
    ] {
        let output = command(binary, home.path())
            .args(["--api-key", "rpa_direct", "--profile", profile])
            .env(
                "RUNPOD_API_BASE_URL",
                "invalid URL: must not be parsed for direct keys",
            )
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("rpa_direct"));
        assert_eq!(
            store.get_credentials(profile).unwrap().unwrap().api_key,
            "rpa_direct"
        );
        assert_eq!(
            store.get_credentials("other").unwrap().unwrap().api_key,
            "keep"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn browser_login_uses_anonymous_api_and_saves_only_after_approval() {
    for grant in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(home.path().join(".runpod/config.toml"));
        store
            .set_credentials("keep", "default", runpod::credentials::Overwrite::Allow)
            .unwrap();
        let before = std::fs::read(store.path()).unwrap();
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                let data = if body["query"]
                    .as_str()
                    .unwrap()
                    .contains("createFlashAuthRequest")
                {
                    json!({"createFlashAuthRequest":{"id":"req-1"}})
                } else {
                    json!({"flashAuthRequestStatus":if grant {
                json!({"status":"APPROVED","apiKey":"rpa_granted"})
            } else {json!({"status":"DENIED"})}})
                };
                ResponseTemplate::new(200).set_body_json(json!({"data":data}))
            })
            .mount(&server)
            .await;
        let result = command(env!("CARGO_BIN_EXE_rp"), home.path())
            .args(["--no-open", "--profile", "agent"])
            .env("RUNPOD_API_BASE_URL", server.uri())
            .env("RUNPOD_CONSOLE_URL", "https://console.example")
            .output()
            .await
            .unwrap();
        let out = String::from_utf8(result.stdout).unwrap();
        let err = String::from_utf8(result.stderr).unwrap();
        assert_eq!(result.status.code(), Some(i32::from(!grant)), "{err}");
        assert!(out.contains("https://console.example/flash/login?request=req-1"));
        assert!(!out.contains("rpa_granted") && !err.contains("rpa_granted"));
        assert_eq!(
            store.get_credentials("default").unwrap().unwrap().api_key,
            "keep"
        );
        if grant {
            assert_eq!(
                store.get_credentials("agent").unwrap().unwrap().api_key,
                "rpa_granted"
            );
        } else {
            assert_eq!(std::fs::read(store.path()).unwrap(), before);
            assert!(err.contains("login denied"));
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert_eq!(request.url.path(), "/graphql");
            assert!(!request.headers.contains_key("authorization"));
            assert!(!request.headers.contains_key("cookie"));
        }
        assert_eq!(
            requests[1].body_json::<Value>().unwrap()["variables"],
            json!({"flashAuthRequestId":"req-1"})
        );
    }
}

#[tokio::test]
async fn invalid_options_and_noninteractive_agent_login_do_not_touch_storage() {
    let home = tempfile::tempdir().unwrap();
    for (args, message) in [
        (vec!["--agent"], "--agent requires --email"),
        (vec!["--agent", "--email", " "], "--agent requires --email"),
        (vec!["--email", "x@example.com"], "--email requires --agent"),
        (
            vec!["--agent", "--api-key", "rpa_input"],
            "--agent cannot be combined with --api-key",
        ),
        (
            vec!["--agent", "--email", "x@example.com"],
            "interactive terminal",
        ),
    ] {
        let result = command(env!("CARGO_BIN_EXE_rp"), home.path())
            .args(&args)
            .env("RUNPOD_API_BASE_URL", "invalid URL")
            .output()
            .await
            .unwrap();
        let err = String::from_utf8(result.stderr).unwrap();
        assert_eq!(result.status.code(), Some(2), "{args:?}: {err}");
        assert!(err.contains(message), "{args:?}: {err}");
        assert!(!err.contains("rpa_input"));
        assert!(!home.path().join(".runpod").exists());
    }
}

use super::*;
use crate::{Value, apps::AppError, cli::interaction::tests::Fake};

struct FakeAuth {
    call: Value,
    urls: Vec<String>,
    calls: usize,
}
impl Authentication for FakeAuth {
    async fn browser(&mut self, show: &mut auth::LoginUrlObserver<'_>) -> crate::Result<String> {
        self.calls += 1;
        assert_eq!(self.call["mode"], "browser");
        for url in &self.urls {
            show(url)?;
        }
        self.result()
    }
    async fn agent(
        &mut self,
        email: &str,
        read: &mut (dyn FnMut() -> crate::Result<String> + Send),
    ) -> crate::Result<String> {
        self.calls += 1;
        assert_eq!(self.call["mode"], "agent");
        assert_eq!(email, self.call["email"].as_str().unwrap());
        for index in 0..self.call["reads"].as_u64().unwrap() {
            let code = read()?;
            assert_eq!(
                code,
                self.call["codes"][usize::try_from(index).unwrap()]
                    .as_str()
                    .unwrap()
            );
        }
        self.result()
    }
}
impl FakeAuth {
    fn result(&self) -> crate::Result<String> {
        if let Some(error) = self.call["error"].as_str() {
            Err(AppError::Login(error.into()).into())
        } else {
            Ok(self.call["key"].as_str().expect("configured result").into())
        }
    }
}

fn profiles(text: &str) -> std::collections::BTreeMap<String, String> {
    text.parse::<toml_edit::DocumentMut>()
        .unwrap()
        .iter()
        .map(|(profile, value)| (profile.into(), value["api_key"].as_str().unwrap().into()))
        .collect()
}

async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../tests/fixtures/login_cli.json")).unwrap();
    let cases: Vec<_> = cases
        .iter()
        .filter(|case| {
            case["name"] == name
                || (name == "characterization" && case["name"].as_str().unwrap().starts_with(name))
        })
        .collect();
    assert!(!cases.is_empty(), "missing source cases for {name}");
    for case in cases {
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let matches = command().try_get_matches_from(&args);
        if args.contains(&"--help") {
            let help = matches.unwrap_err();
            assert!(!help.use_stderr());
            let text = help
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            for message in [
                "requires an interactive terminal",
                "Keep this process running until sign-in completes",
                "same process that requested it",
            ] {
                assert!(text.contains(message), "{text}");
            }
            continue;
        }
        let matches = matches.unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(home.path().join("config.toml"));
        if let Some(before) = case["before"].as_str() {
            std::fs::write(store.path(), before).unwrap();
        }
        let mut io = Fake::default();
        for prompt in case["prompts"].as_array().unwrap() {
            if let Some(answer) = prompt["answer"].as_str() {
                io.texts.push_back(answer.into());
            }
        }
        let mut auth = FakeAuth {
            call: case["auth"].get(0).cloned().unwrap_or(Value::Null),
            urls: case["urls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().into())
                .collect(),
            calls: 0,
        };
        let mut written = vec![];
        let mut save = |key: &str, profile: &str| {
            written.push(crate::json!({"key":key,"profile":profile,"overwrite":true}));
            if let Some(error) = case["writes"][0]["error"]
                .as_str()
                .filter(|_| name == "test_credential_write_failure_is_reported")
            {
                return Err(crate::Error::Io(std::io::Error::other(error.to_owned())));
            }
            store.set_credentials(key, profile, Overwrite::Allow)
        };
        let mut opened = vec![];
        let mut launch = |url: &str| {
            opened.push(url.to_owned());
        };
        let mut out = vec![];
        let mut err = vec![];
        let result = async {
            Options::read(&matches)?
                .execute(
                    &mut auth,
                    &mut save,
                    &mut io,
                    &mut Streams::new(&mut out, &mut err, false),
                    &mut launch,
                )
                .await
        }
        .await;
        let code = result.as_ref().map_or_else(CliError::code, |()| 0);
        assert_eq!(
            u64::from(code),
            case["code"].as_u64().unwrap(),
            "{}: {result:?}",
            case["name"]
        );
        if let Err(error) = result {
            let expected = case["stderr"].as_str().unwrap();
            if expected.contains("Aborted!") {
                assert!(matches!(error, CliError::Aborted));
            } else {
                assert!(
                    error
                        .to_string()
                        .contains(expected.rsplit_once("Error: ").unwrap().1.trim()),
                    "{error}: {expected}"
                );
            }
        } else {
            assert_eq!(
                String::from_utf8(err).unwrap(),
                case["stderr"].as_str().unwrap()
            );
        }
        // Click adds a second line break when a hidden prompt reaches EOF. Prompt
        // rendering belongs to Interaction; the fake records prompts without printing.
        let source_out = case["stdout"]
            .as_str()
            .unwrap()
            .replace("Verification code: \n\n", "Verification code: \n");
        let expected = source_out
            .lines()
            .filter(|line| !line.starts_with("Verification code:"))
            .fold(String::new(), |mut out, line| {
                out.push_str(line);
                out.push('\n');
                out
            });
        assert_eq!(
            String::from_utf8(out).unwrap(),
            expected,
            "{}",
            case["name"]
        );
        assert_eq!(auth.calls, case["auth"].as_array().unwrap().len());
        assert_eq!(
            opened,
            case["opened"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            io.prompts,
            case["prompts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (
                    p["message"].as_str().unwrap().into(),
                    p["hidden"].as_bool().unwrap()
                ))
                .collect::<Vec<_>>()
        );
        let expected_writes:Vec<_>=case["writes"].as_array().unwrap().iter().map(|v|crate::json!({"key":v["key"],"profile":v["profile"],"overwrite":v["overwrite"]})).collect();
        assert_eq!(written, expected_writes);
        let after = std::fs::read_to_string(store.path()).ok();
        if code == 0 && case["after"].is_string() {
            assert_eq!(
                profiles(after.as_deref().unwrap()),
                profiles(case["after"].as_str().unwrap())
            );
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
        } else {
            assert_eq!(after.as_deref(), case["after"].as_str());
        }
    }
}

#[tokio::test]
async fn test_agent_saves_named_profile_and_hides_code_and_key() {
    verify("test_agent_saves_named_profile_and_hides_code_and_key").await;
}
#[tokio::test]
async fn test_invalid_options_have_no_auth_or_storage_side_effects() {
    verify("test_invalid_options_have_no_auth_or_storage_side_effects").await;
}
#[tokio::test]
async fn test_code_format_is_validated_without_echoing_input() {
    verify("test_code_format_is_validated_without_echoing_input").await;
}
#[tokio::test]
async fn test_eof_during_code_prompt_preserves_credentials() {
    verify("test_eof_during_code_prompt_preserves_credentials").await;
}
#[tokio::test]
async fn test_auth_error_preserves_credentials() {
    verify("test_auth_error_preserves_credentials").await;
}
#[tokio::test]
async fn test_credential_write_failure_is_reported() {
    verify("test_credential_write_failure_is_reported").await;
}
#[tokio::test]
async fn test_direct_key_login_does_not_start_email_flow() {
    verify("test_direct_key_login_does_not_start_email_flow").await;
}
#[tokio::test]
async fn test_agent_retry_explains_rejection_without_resending_notice() {
    verify("test_agent_retry_explains_rejection_without_resending_notice").await;
}
#[tokio::test]
async fn test_agent_help_explains_interactive_process_requirement() {
    verify("test_agent_help_explains_interactive_process_requirement").await;
}
#[tokio::test]
async fn test_login_updates_only_selected_profile() {
    verify("TestLoginCommand.test_login_updates_only_selected_profile").await;
}
#[tokio::test]
async fn test_browser_flow_error() {
    verify("TestLoginCommand.test_browser_flow_error").await;
}
#[tokio::test]
async fn characterized_login_edges() {
    verify("characterization").await;
}

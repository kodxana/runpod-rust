use super::credentials::{Kind, Operation};
use super::{CliError, interaction::tests::Fake};
use crate::{Client, ClientConfig, Value, apps::AppsApiClient};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::any};

async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../tests/fixtures/credential_cli.json")).unwrap();
    let selected: Vec<_> = cases
        .iter()
        .filter(|case| {
            case["name"] == name
                || (name == "characterization" && case["name"].as_str().unwrap().starts_with(name))
        })
        .collect();
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
        let api = AppsApiClient::new(
            Client::with_config(ClientConfig {
                api_base_url: server.uri(),
                ..ClientConfig::new("fixture-key")
            })
            .unwrap(),
        )
        .unwrap();
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let kind = if args[0] == "secret" {
            Kind::Secret
        } else {
            Kind::Registry
        };
        let matches = kind.command().try_get_matches_from(&args).unwrap();
        let mut ui = Fake::default();
        for prompt in case["prompts"].as_array().unwrap() {
            if let Some(answer) = prompt["answer"].as_str() {
                ui.texts.push_back(answer.into());
            }
        }
        if let Some(confirm) = case["confirmations"].as_array().unwrap().first() {
            ui.answer = confirm["answer"].as_bool().unwrap();
        }
        let result = async {
            Operation::read(kind, &matches, &mut ui)?
                .execute(&api)
                .await
        }
        .await;
        assert_eq!(
            result.as_ref().map_or_else(CliError::code, |_| 0),
            u8::try_from(case["code"].as_u64().unwrap()).unwrap(),
            "{}: {result:?}",
            case["name"]
        );
        match result {
            Ok(out) => {
                let expected = case["messages"].as_array().unwrap().iter().fold(
                    String::new(),
                    |mut text, message| {
                        text.push_str(message.as_str().unwrap());
                        text.push('\n');
                        text
                    },
                );
                assert_eq!(out, expected, "{}", case["name"]);
                assert!(!out.contains("never-print-me"));
            }
            Err(error) => {
                let expected = case["stderr"].as_str().unwrap();
                if expected.contains("Aborted!") {
                    assert!(matches!(error, CliError::Aborted));
                } else {
                    assert!(
                        error
                            .to_string()
                            .contains(expected.rsplit_once("Error: ").unwrap().1.trim()),
                        "{}: {error}",
                        case["name"]
                    );
                }
            }
        }
        assert_eq!(
            ui.prompts,
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
        assert_eq!(
            ui.confirmations,
            case["confirmations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["message"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        );
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
    }
}

#[tokio::test]
async fn test_secret_add() {
    verify("TestSecretCommands.test_add").await;
}
#[tokio::test]
async fn test_secret_add_prompts() {
    verify("TestSecretCommands.test_add_prompts").await;
}
#[tokio::test]
async fn test_secret_list() {
    verify("TestSecretCommands.test_list").await;
}
#[tokio::test]
async fn test_secret_list_empty() {
    verify("TestSecretCommands.test_list_empty").await;
}
#[tokio::test]
async fn test_secret_delete() {
    verify("TestSecretCommands.test_delete").await;
}
#[tokio::test]
async fn test_secret_delete_unknown() {
    verify("TestSecretCommands.test_delete_unknown").await;
}
#[tokio::test]
async fn test_registry_add() {
    verify("TestRegistryCommands.test_add").await;
}
#[tokio::test]
async fn test_registry_list() {
    verify("TestRegistryCommands.test_list").await;
}
#[tokio::test]
async fn test_registry_delete() {
    verify("TestRegistryCommands.test_delete").await;
}
#[tokio::test]
async fn test_registry_delete_unknown() {
    verify("TestRegistryCommands.test_delete_unknown").await;
}
#[tokio::test]
async fn characterized_edges() {
    verify("characterization").await;
}

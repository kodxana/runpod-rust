use super::*;
use crate::{ClientConfig, cli::interaction::tests::Fake};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../tests/fixtures/volume_cli.json")).unwrap();
    assert!(cases.iter().any(|case| case["name"] == name));
    for case in cases.iter().filter(|case| case["name"] == name) {
        let server = MockServer::start().await;
        for request in case["requests"].as_array().unwrap() {
            Mock::given(method(request["method"].as_str().unwrap()))
                .and(path(request["path"].as_str().unwrap()))
                .and(header("authorization", "Bearer fixture-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&request["response"]))
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("fixture-key")
        })
        .unwrap();
        let args = std::iter::once("volume").chain(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap()),
        );
        let matches = command().try_get_matches_from(args).unwrap();
        let mut ui = Fake {
            columns: 1000,
            answer: case["answer"] == "y",
            ..Fake::default()
        };
        if let Some(selected) = case.get("selected") {
            ui.selected = case["picks"][0]["choices"]
                .as_array()
                .unwrap()
                .iter()
                .position(|choice| choice[0] == *selected);
        }
        let result = Options::read(&matches)
            .unwrap()
            .execute(&client, &mut ui)
            .await;
        let code = result.as_ref().err().map_or(0, CliError::code);
        assert_eq!(u64::from(code), case["code"].as_u64().unwrap(), "{case}");
        if let Some(error) = case.get("error") {
            assert_eq!(result.unwrap_err().to_string(), error.as_str().unwrap());
        } else {
            let output = result.unwrap();
            if let Some(expected) = case.get("json") {
                assert_eq!(serde_json::from_str::<Value>(&output).unwrap(), *expected);
            } else {
                assert!(output.contains("vol1"));
            }
        }
        assert_eq!(json!(ui.confirmations), case["confirms"]);
        assert_eq!(ui.selections.len(), case["picks"].as_array().unwrap().len());
        for ((message, labels), expected) in
            ui.selections.iter().zip(case["picks"].as_array().unwrap())
        {
            assert_eq!(message, expected["message"].as_str().unwrap());
            let expected: Vec<_> = expected["choices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|choice| choice[1].as_str().unwrap())
                .collect();
            assert_eq!(*labels, expected);
        }
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), case["requests"].as_array().unwrap().len());
        for (actual, expected) in received.iter().zip(case["requests"].as_array().unwrap()) {
            assert_eq!(actual.method.as_str(), expected["method"].as_str().unwrap());
            assert_eq!(actual.url.path(), expected["path"].as_str().unwrap());
            assert_eq!(actual.url.query(), None);
            let body = if actual.body.is_empty() {
                Value::Null
            } else {
                actual.body_json().unwrap()
            };
            assert_eq!(body, expected["body"]);
        }
    }
}
#[tokio::test]
async fn test_get_picker() {
    verify("test_get_picker").await;
}
#[tokio::test]
async fn test_create_filters_picker_candidates() {
    verify("test_create_filters_picker_candidates").await;
}
#[tokio::test]
async fn test_delete_confirmation() {
    verify("test_delete_confirmation").await;
}

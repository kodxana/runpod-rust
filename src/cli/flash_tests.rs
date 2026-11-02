use super::*;
use crate::cli::interaction::tests::Fake;
use crate::json;

struct Recorded {
    expected: Vec<Value>,
    actual: Vec<Value>,
}
impl Recorded {
    fn call(&mut self, operation: &str, args: &Value) -> crate::Result<Value> {
        let expected = &self.expected[self.actual.len()];
        self.actual
            .push(json!({"operation": operation, "args": args}));
        assert_eq!(expected["operation"], operation);
        assert_eq!(&expected["args"], args);
        if let Some(error) = expected["error"].as_str() {
            return Err(crate::Error::Authentication(error.into()));
        }
        Ok(expected["result"].clone())
    }
}
impl Backend for Recorded {
    async fn list(&mut self) -> crate::Result<Vec<Value>> {
        Ok(serde_json::from_value(self.call("list_apps", &json!([]))?).unwrap())
    }
    async fn get(&mut self, name: &str) -> crate::Result<Value> {
        self.call("get_app", &json!([name]))
    }
    async fn delete(&mut self, name: &str, _: &EventSink) -> crate::Result<UndeployResult> {
        Ok(serde_json::from_value(self.call("delete_app", &json!([name]))?).unwrap())
    }
}

async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../tests/fixtures/app_cli.json")).unwrap();
    let case = cases
        .iter()
        .find(|case| case["name"] == format!("TestAppCommands.{name}"))
        .unwrap();
    let args: Vec<_> = case["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let matches = command().try_get_matches_from(args).unwrap();
    let mut ui = Fake::default();
    if let Some(confirm) = case["confirmations"].as_array().unwrap().first() {
        ui.answer = confirm["answer"].as_bool().unwrap();
    }
    let mut backend = Recorded {
        expected: case["calls"].as_array().unwrap().clone(),
        actual: Vec::new(),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = async {
        Operation::read(&matches, &mut ui)?
            .execute(&mut backend, &mut Streams::new(&mut out, &mut err, false))
            .await
    }
    .await;
    assert_eq!(
        result.as_ref().map_or_else(CliError::code, |()| 0),
        u8::try_from(case["code"].as_u64().unwrap()).unwrap(),
        "{name}: {result:?}"
    );
    let mut expected = String::new();
    for message in case["messages"].as_array().unwrap() {
        writeln!(expected, "{}", message.as_str().unwrap()).unwrap();
    }
    assert_eq!(String::from_utf8(out).unwrap(), expected);
    if let Err(error) = result {
        if case["stderr"].as_str().unwrap().contains("Aborted!") {
            assert!(matches!(error, CliError::Aborted));
        } else {
            assert!(
                error.to_string().contains(
                    case["stderr"]
                        .as_str()
                        .unwrap()
                        .rsplit_once("Error: ")
                        .unwrap()
                        .1
                        .trim()
                ),
                "{error}"
            );
        }
    }
    assert_eq!(backend.actual.len(), backend.expected.len());
    assert_eq!(
        ui.confirmations,
        case["confirmations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["message"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(err, [] as [u8; 0]);
}

#[tokio::test]
async fn test_list_empty() {
    verify("test_list_empty").await;
}
#[tokio::test]
async fn test_list() {
    verify("test_list").await;
}
#[tokio::test]
async fn test_get() {
    verify("test_get").await;
}
#[tokio::test]
async fn test_get_error() {
    verify("test_get_error").await;
}
#[tokio::test]
async fn test_delete_confirmed() {
    verify("test_delete_confirmed").await;
}
#[tokio::test]
async fn test_delete_aborts_without_confirmation() {
    verify("test_delete_aborts_without_confirmation").await;
}
#[tokio::test]
async fn test_delete_failures_keep_app() {
    verify("test_delete_failures_keep_app").await;
}

#[test]
fn malformed_metadata_returns_errors() {
    for entry in [
        json!({"name": 1}),
        json!({"name": "a", "flashEnvironments": "wrong"}),
        json!({"name":"a", "flashEnvironments":[{"name":"e", "activeBuildId":1}]}),
    ] {
        assert!(show_app(&entry).is_err(), "{entry}");
    }
}

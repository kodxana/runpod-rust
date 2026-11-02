use super::*;
use crate::{cli::interaction::tests::Fake, json};

struct Recorded {
    expected: Vec<Value>,
    calls: usize,
}
impl Recorded {
    fn call(&mut self, operation: &str, args: &Value) -> crate::Result<Value> {
        let expected = &self.expected[self.calls];
        self.calls += 1;
        assert_eq!(expected["operation"], operation);
        assert_eq!(&expected["args"], args);
        if let Some(error) = expected["error"].as_str() {
            return Err(crate::Error::Authentication(error.into()));
        }
        Ok(expected["result"].clone())
    }
}
impl Backend for Recorded {
    async fn app(&mut self, name: &str) -> crate::Result<Value> {
        self.call("get_app", &json!([name]))
    }
    async fn environment(&mut self, app: &str, env: &str) -> crate::Result<Value> {
        self.call("get_environment", &json!([app, env]))
    }
    async fn create(&mut self, id: &str, env: &str) -> crate::Result<Value> {
        self.call("create_environment", &json!([id, env]))
    }
    async fn undeploy(
        &mut self,
        app: &str,
        env: &str,
        _: &EventSink,
    ) -> crate::Result<UndeployResult> {
        Ok(serde_json::from_value(self.call("undeploy_environment", &json!([app, env]))?).unwrap())
    }
}
async fn verify(name: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../tests/fixtures/env_cli.json")).unwrap();
    let case = cases.iter().find(|case| case["name"] == name).unwrap();
    let args: Vec<_> = case["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let matches = crate::cli::flash::command()
        .try_get_matches_from(args)
        .unwrap();
    let (command, args) = arguments(&matches).unwrap();
    let app = args.get_one::<String>("app").unwrap();
    let mut ui = Fake::default();
    let mut backend = Recorded {
        expected: case["calls"].as_array().unwrap().clone(),
        calls: 0,
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let result = async {
        Operation::read(command, args, app, &mut ui)?
            .execute(
                app,
                &mut backend,
                &mut Streams::new(&mut out, &mut err, false),
            )
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
    assert_eq!(backend.calls, backend.expected.len());
    assert_eq!(err, [] as [u8; 0]);
    assert_eq!(ui.confirmations, [] as [String; 0]);
}

#[tokio::test]
async fn test_list() {
    verify("TestEnvCommands.test_list").await;
}
#[tokio::test]
async fn test_list_empty() {
    verify("TestEnvCommands.test_list_empty").await;
}
#[tokio::test]
async fn test_get() {
    verify("TestEnvCommands.test_get").await;
}
#[tokio::test]
async fn test_add() {
    verify("TestEnvCommands.test_add").await;
}
#[tokio::test]
async fn test_delete() {
    verify("TestEnvCommands.test_delete").await;
}
#[tokio::test]
async fn test_undeploy() {
    verify("TestUndeploy.test_undeploy").await;
}
#[tokio::test]
async fn test_undeploy_failures() {
    verify("TestUndeploy.test_undeploy_failures").await;
}
#[tokio::test]
async fn test_resolve_app_name_fails_without_unique_app() {
    let dir = tempfile::tempdir().unwrap();
    let error = resolve_app_name(None, || Ok(dir.path().into()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), 1);
    assert_eq!(
        error.to_string(),
        "pass --app <name> (no unique app found in the current directory)"
    );
    assert_eq!(
        resolve_app_name(Some("explicit"), || panic!(
            "explicit names bypass discovery"
        ))
        .await
        .unwrap(),
        "explicit"
    );
}

#[test]
fn confirmation_and_timestamp_edges() {
    for (args, question) in [
        (
            vec!["flash", "env", "delete", "prod", "-a", "demo"],
            "undeploy and delete 'demo/prod'?",
        ),
        (
            vec!["flash", "undeploy", "--app", "demo"],
            "undeploy 'demo/default'?",
        ),
    ] {
        let matches = crate::cli::flash::command()
            .try_get_matches_from(args)
            .unwrap();
        let (command, args) = arguments(&matches).unwrap();
        let mut ui = Fake {
            answer: false,
            ..Fake::default()
        };
        assert!(matches!(
            Operation::read(command, args, "demo", &mut ui),
            Err(CliError::Aborted)
        ));
        assert_eq!(ui.confirmations, [question]);
    }
    assert_eq!(timestamp(&Value::Null), "-");
    assert_eq!(timestamp(&json!(false)), "-");
    assert_eq!(timestamp(&json!(17)), "17");
    assert_eq!(timestamp(&json!("é中😀abcdefghij")), "é中😀abcdefg");
}

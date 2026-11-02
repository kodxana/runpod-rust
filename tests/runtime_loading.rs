use runpod::{
    Error, Result, Value,
    apps::{
        App, AppRegistry, FunctionArgs, NativeApi, NativeFunction, ResourceSpec, RouteSpec,
        deploy::SymbolReference, runtime::DeployedContext, targets::FunctionDefinition,
    },
    json,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/runtime_loading.json")).unwrap()
}

fn context(values: &Value, manifest: Option<&Value>) -> (tempfile::TempDir, DeployedContext) {
    let dir = tempfile::tempdir().unwrap();
    if let Some(manifest) = manifest {
        std::fs::write(
            dir.path().join("runpod_manifest.json"),
            manifest.to_string(),
        )
        .unwrap();
    }
    let mut values: BTreeMap<String, String> = serde_json::from_value(values.clone()).unwrap();
    values.insert("RUNPOD_APP_DIR".into(), dir.path().to_str().unwrap().into());
    let context = DeployedContext::from_values(&values).unwrap();
    (dir, context)
}

fn definition(name: &str) -> FunctionDefinition {
    FunctionDefinition {
        name: name.into(),
        parameters: vec![],
        source: None,
    }
}

fn registry(declarations: &Value, events: &Arc<Mutex<Vec<String>>>) -> Result<AppRegistry> {
    let mut registry = AppRegistry::new();
    for item in declarations.as_array().unwrap() {
        if let Some(alias) = item["alias"].as_u64() {
            registry.register(registry.apps()[usize::try_from(alias).unwrap()].clone());
            continue;
        }
        let app = App::new(item["app"].as_str().unwrap())?;
        let name = item["name"].as_str().unwrap();
        let tag = item["tag"].as_str().unwrap().to_owned();
        let body_tag = tag.clone();
        let body = NativeFunction::sync(move |_| Ok(json!(body_tag)));
        let symbol = SymbolReference {
            module: "user".into(),
            qualname: item["qualname"].as_str().unwrap().into(),
        };
        if item["kind"] == "api" {
            let mut api = NativeApi::new(name);
            api.route(
                RouteSpec {
                    method: "GET".into(),
                    path: "/tag".into(),
                    handler_name: "tag".into(),
                },
                body,
            )?;
            api.set_symbol(symbol);
            app.api(ResourceSpec::default(), api)?;
        } else {
            let handle = if item["kind"] == "task" {
                app.task(ResourceSpec::default(), definition(name), body)?
            } else {
                app.queue(ResourceSpec::default(), definition(name), body)?
            };
            handle.set_symbol(symbol)?;
            if item["init"] == true {
                let events = events.clone();
                handle.init(NativeFunction::sync(move |_| {
                    events.lock().unwrap().push(tag.clone());
                    Ok(Value::Null)
                }))?;
            }
        }
        registry.register(app);
    }
    Ok(registry)
}

async fn selection(case: &Value) {
    let (_dir, context) = context(&case["values"], Some(&case["manifest"]));
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut imports = Vec::new();
    let loader = |module: &str, _: &_| {
        imports.push(module.to_owned());
        if module != "user" {
            return Err(Error::InvalidInput(format!("unknown module: {module}")));
        }
        registry(&case["declarations"], &events)
    };
    let result = match case["kind"].as_str().unwrap() {
        "queue" => match context.load_queue(loader) {
            Ok(handle) => handle.local(vec![], serde_json::Map::new()).await,
            Err(error) => Err(error),
        },
        "api" => match context.load_api(loader) {
            Ok(handle) => handle.call_handler("tag", FunctionArgs::default()).await,
            Err(error) => Err(error),
        },
        "task" => match context.load_task(loader).await {
            Ok(handle) => handle.local(vec![], serde_json::Map::new()).await,
            Err(error) => Err(error),
        },
        _ => unreachable!(),
    };
    let (selected, error, phase) = match result {
        Ok(value) => (value, Value::Null, Value::Null),
        Err(error) => {
            let phase = match &error {
                Error::Bootstrap(e) => json!(e.phase),
                _ => Value::Null,
            };
            (
                Value::Null,
                json!(error.to_string().split(" (has:").next().unwrap()),
                phase,
            )
        }
    };
    assert_eq!(
        json!({"selected":selected,"error":error,"phase":phase,"imports":imports,"init":*events.lock().unwrap()}),
        json!({"selected":case["selected"],"error":case["error"],"phase":case["phase"],"imports":case["imports"],"init":case["init"]}),
        "{}",
        case["name"]
    );
}

async fn named(names: &[&str]) {
    for name in names {
        let data = fixtures();
        selection(
            data["selection"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == *name)
                .unwrap(),
        )
        .await;
    }
}

#[tokio::test]
async fn source_runtime_loading_captures() {
    let data = fixtures();
    assert_eq!(data["selection"].as_array().unwrap().len(), 34);
    for case in data["selection"].as_array().unwrap() {
        selection(case).await;
    }
}

#[tokio::test]
async fn test_load_deployed_handle() {
    named(&["queue"]).await;
}
#[tokio::test]
async fn test_load_deployed_handle_missing_resource() {
    named(&["queue missing manifest resource"]).await;
}
#[tokio::test]
async fn test_load_deployed_handle_missing_handle() {
    named(&["queue missing handle"]).await;
}
#[tokio::test]
async fn test_missing_resource_errors() {
    named(&["api missing manifest resource"]).await;
}
#[tokio::test]
async fn test_invalid_production_identity_never_falls_back() {
    named(&[
        "task runtime kind",
        "task identity FLASH_APP_NAME",
        "task identity FLASH_ENVIRONMENT",
        "task identity FLASH_RESOURCE_NAME",
        "task identity RUNPOD_BUILD_ID",
    ])
    .await;
}
#[tokio::test]
async fn test_manifest_must_identify_registered_task() {
    named(&[
        "task wrong manifest app",
        "task wrong manifest kind",
        "task missing manifest resource",
        "task manifest qualified name mismatch",
        "task unknown module",
    ])
    .await;
}
#[tokio::test]
async fn test_duplicate_task_manifest_rejected() {
    named(&["task duplicate manifest"]).await;
}
#[tokio::test]
async fn test_manifest_qualname_resolves_class_attribute() {
    named(&["task nested qualified name"]).await;
}
#[tokio::test]
async fn test_task_identity_accepts_host_artifact_path() {
    let mut case = fixtures()["selection"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "task initializes")
        .unwrap()
        .clone();
    case["values"]["FLASH_BUILD_ARTIFACT_PATH"] = json!("/host/artifact.tar.gz");
    selection(&case).await;
}

#[test]
fn test_resource_name_flash_env() {
    assert_eq!(
        context(
            &json!({"FLASH_RESOURCE_NAME":"flash","RUNPOD_RESOURCE_NAME":"fallback"}),
            None
        )
        .1
        .resource_name(),
        "flash"
    );
}
#[test]
fn test_resource_name_runpod_env() {
    assert_eq!(
        context(&json!({"RUNPOD_RESOURCE_NAME":"fallback"}), None)
            .1
            .resource_name(),
        "fallback"
    );
}
#[test]
fn test_resource_name_empty() {
    assert_eq!(context(&json!({}), None).1.resource_name(), "");
}
#[test]
fn test_is_deployed_requires_name_and_manifest() {
    for case in fixtures()["detection"].as_array().unwrap() {
        let manifest = json!({});
        let (_dir, context) = context(
            &case["values"],
            case["manifest"].as_bool().unwrap().then_some(&manifest),
        );
        assert_eq!(json!(context.resource_name()), case["name"]);
        assert_eq!(json!(context.is_deployed()), case["deployed"]);
    }
}

#[test]
fn source_mount_configuration_captures() {
    for case in fixtures()["mounts"].as_array().unwrap() {
        let mut values = BTreeMap::new();
        if let Some(raw) = case["raw"].as_str() {
            values.insert("RUNPOD_MOUNTS".into(), raw.into());
        }
        let result = DeployedContext::from_values(&values).and_then(|context| {
            if let Some(lookup) = case["lookup"].as_array() {
                Ok(json!(context.mounts().path(
                    lookup[0].as_str().unwrap(),
                    lookup[1].as_str().unwrap()
                )?))
            } else {
                Ok(Value::Null)
            }
        });
        match result {
            Ok(value) => {
                assert_eq!(value, case["result"], "{case}");
                assert!(case["error"].is_null(), "{case}");
            }
            Err(error) => {
                let message = match &error {
                    Error::Context { message, source } => {
                        assert!(matches!(source.as_ref(), Error::Json(_)));
                        message.clone()
                    }
                    _ => error.to_string(),
                };
                assert_eq!(json!(message), case["error"], "{case}");
            }
        }
    }
}

#[tokio::test]
async fn loader_failure_retains_native_cause() {
    let manifest = json!({"resources":[{"name":"work","module":"user"}]});
    let (_dir, context) = context(&json!({"FLASH_RESOURCE_NAME":"work"}), Some(&manifest));
    let error = context
        .load_queue(|_, _| {
            Err(
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "registration denied")
                    .into(),
            )
        })
        .unwrap_err();
    assert!(matches!(error, Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied));
}

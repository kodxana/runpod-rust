//! Hosts a compiled task selected from an already-unpacked worker manifest.
use runpod::{
    Error, Result,
    apps::{
        App, AppRegistry, NativeFunction, ResourceSpec, deploy::SymbolReference, discovery,
        runtime::DeployedContext, targets::FunctionDefinition, task_runner,
    },
    json,
};

#[tokio::main]
async fn main() -> Result<()> {
    let app = App::new("example")?;
    let task = app.task(
        ResourceSpec::default(),
        FunctionDefinition {
            name: "work".into(),
            parameters: vec![],
            source: None,
        },
        NativeFunction::sync(|call| {
            runpod::serverless::capture::stdout().write_text("task output\n")?;
            Ok(json!({"args":call.args,"kwargs":call.kwargs}))
        }),
    )?;
    task.set_symbol(SymbolReference {
        module: "deployed_task".into(),
        qualname: "work".into(),
    })?;
    let mut apps = AppRegistry::new();
    apps.register(app);
    if discovery::export_if_requested(&apps)? {
        return Ok(());
    }
    task_runner::start(async {
        DeployedContext::from_env()?
            .load_task(|module, _| match module {
                "deployed_task" => Ok(apps),
                _ => Err(Error::InvalidInput(format!(
                    "unknown compiled module: {module}"
                ))),
            })
            .await
    })
    .await
}

//! Loads a compiled queue from an already-unpacked worker manifest.
use runpod::{
    Error, Result,
    apps::{
        App, AppRegistry, NativeFunction, ResourceSpec, deploy::SymbolReference, discovery,
        runtime::DeployedContext, targets::FunctionDefinition, volume::MountBindings,
    },
    json,
};

fn declarations(mounts: &MountBindings) -> Result<AppRegistry> {
    let app = App::new("greetings")?;
    let mounts = mounts.clone();
    let hello = app.queue(
        ResourceSpec::default(),
        FunctionDefinition {
            name: "hello".into(),
            parameters: vec!["name".into()],
            source: None,
        },
        NativeFunction::sync(move |call| {
            let name = call
                .kwargs
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("world");
            Ok(json!({"greeting":format!("hello {name}"),"data":mounts.path("network", "data")?}))
        }),
    )?;
    hello.set_symbol(SymbolReference {
        module: "deployed_queue".into(),
        qualname: "hello".into(),
    })?;
    let mut apps = AppRegistry::new();
    apps.register(app);
    Ok(apps)
}

#[tokio::main]
async fn main() -> Result<()> {
    let context = DeployedContext::from_env()?;
    let apps = declarations(context.mounts())?;
    if discovery::export_if_requested(&apps)? {
        return Ok(());
    }
    let handle = context.load_queue(|module, _| match module {
        "deployed_queue" => Ok(apps),
        _ => Err(Error::InvalidInput(format!(
            "unknown compiled module: {module}"
        ))),
    })?;
    handle.start_queue_worker().await
}

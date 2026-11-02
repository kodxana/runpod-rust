//! Prepares a platform artifact and serves its compiled queue declaration.
use runpod::{
    Error, Result,
    apps::{
        App, AppRegistry, NativeFunction, ResourceSpec, bootstrap, deploy::SymbolReference,
        discovery, in_discovery, targets::FunctionDefinition, volume::MountBindings,
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
                .and_then(|value| value.as_str())
                .unwrap_or("world");
            Ok(json!({"greeting":format!("hello {name}"),"data":mounts.path("network","data")?}))
        }),
    )?;
    hello.set_symbol(SymbolReference {
        module: "bootstrap_queue".into(),
        qualname: "hello".into(),
    })?;
    let mut apps = AppRegistry::new();
    apps.register(app);
    Ok(apps)
}

#[tokio::main]
async fn main() -> Result<()> {
    if in_discovery() && discovery::export_if_requested(&declarations(&MountBindings::default())?)?
    {
        return Ok(());
    }
    bootstrap::start_deployed(|module, context| match module {
        "bootstrap_queue" => declarations(context.mounts()),
        _ => Err(Error::InvalidInput(format!(
            "unknown compiled module: {module}"
        ))),
    })
    .await
}

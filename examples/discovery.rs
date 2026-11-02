//! Declare native apps before dispatching local entrypoints.
use runpod::{
    Result,
    apps::{
        App, AppRegistry, NativeFunction, ResourceSpec, discovery, targets::FunctionDefinition,
    },
    json,
};

#[tokio::main]
async fn main() -> Result<()> {
    let app = App::new("greetings")?;
    let hello = app.queue(
        ResourceSpec::default(),
        FunctionDefinition {
            name: "hello".into(),
            parameters: vec!["name".into()],
            source: None,
        },
        NativeFunction::new(|call| async move {
            Ok(json!(format!(
                "hello {}",
                call.args
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("world")
            )))
        }),
    )?;
    let mut apps = AppRegistry::new();
    apps.register(app);
    if discovery::export_if_requested(&apps)? {
        return Ok(());
    }
    println!(
        "{}",
        hello
            .local(vec![json!("world")], serde_json::Map::new())
            .await?
    );
    Ok(())
}

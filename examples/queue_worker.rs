//! Compiled Apps queue worker: accepts plain keyword input and streams results.
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
        NativeFunction::streaming(|call| {
            async_stream::try_stream! {
                let name = call.kwargs.get("name").and_then(|v| v.as_str()).unwrap_or("world");
                yield json!(format!("hello {name}"));
                yield json!("done");
            }
        }),
    )?;
    let mut apps = AppRegistry::new();
    apps.register(app);
    if discovery::export_if_requested(&apps)? {
        return Ok(());
    }
    hello.start_queue_worker().await
}

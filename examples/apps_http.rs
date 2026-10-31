//! Start with `cargo run --example apps_http`, then POST JSON to localhost:8080/echo.
use runpod::{
    Result, Value,
    apps::{App, NativeApi, NativeFunction, ResourceSpec, RouteSpec},
};

#[tokio::main]
async fn main() -> Result<()> {
    let app = App::new("native-api")?;
    let mut service = NativeApi::new("web");
    service.route(
        RouteSpec {
            method: "POST".into(),
            path: "/echo".into(),
            handler_name: "echo".into(),
        },
        NativeFunction::new(|call| async move {
            Ok(call.args.into_iter().next().unwrap_or(Value::Null))
        }),
    )?;
    let api = app.api(ResourceSpec::default(), service)?;
    api.start("127.0.0.1", 8080).await
}

//! Run with `cargo run --example apps_http_factory`, then POST bytes to /echo/42.
use axum::{Router, body::Bytes, extract::Path, http::StatusCode, routing::post};
use runpod::apps::{App, FactoryHealth, NativeApi, NativeHttpApp, ResourceSpec};

#[tokio::main]
async fn main() -> runpod::Result<()> {
    let app = App::new("native-web")?;
    let factory = NativeApi::http_factory("web", || async {
        // Load application state here before health checks or requests can succeed.
        let router = Router::new().route(
            "/echo/{id}",
            post(|Path(id): Path<u32>, body: Bytes| async move {
                (StatusCode::CREATED, [("x-id", id.to_string())], body)
            }),
        );
        Ok(
            NativeHttpApp::new(router, FactoryHealth::Add).on_shutdown(async {
                // Flush state or close external resources after active requests drain.
                Ok(())
            }),
        )
    });
    let api = app.api(ResourceSpec::default(), factory)?;
    api.start("127.0.0.1", 8080).await
}

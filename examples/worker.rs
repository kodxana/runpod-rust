use runpod::{
    json,
    serverless::{self, Handler},
};

#[tokio::main]
async fn main() -> runpod::Result<()> {
    serverless::start(Handler::new(
        |job| async move { Ok(json!({"echo":job.input})) },
    ))
    .await
}

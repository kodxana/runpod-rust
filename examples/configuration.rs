//! Build and inspect an endpoint payload; this example does not create a resource.
use runpod::{
    Client,
    configuration::{self, ContainerOptions, EndpointOptions, EndpointType, ScaleOptions},
    json,
};

#[tokio::main]
async fn main() -> runpod::Result<()> {
    let client = Client::from_env()?;
    let options = EndpointOptions {
        name: Some("inference".into()),
        container: ContainerOptions {
            image: Some("my-worker:latest".into()),
            env: vec!["MODEL_ID=example/model".into()],
            ..ContainerOptions::default()
        },
        gpu: vec!["h100:1".into()],
        scale: ScaleOptions {
            min_workers: Some(0),
            max_workers: Some(2),
            ..ScaleOptions::default()
        },
        ..EndpointOptions::default()
    };
    // GPU selectors perform a read-only catalog lookup using the client's credentials.
    let payload = client
        .create_endpoint_config(&options, EndpointType::Queue)
        .await?;
    println!("{}", serde_json::to_string_pretty(&payload)?);

    let before =
        configuration::mutable_endpoint(&json!({"name":"inference", "workers":{"min":0,"max":2}}))?;
    let edited = configuration::validate_endpoint_document(r#"{"name":"inference-v2"}"#, &before)?;
    let patch = configuration::endpoint_patch(&before, &edited)?;
    println!("{}", serde_json::to_string_pretty(&patch)?);
    Ok(())
}

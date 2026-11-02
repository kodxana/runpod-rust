//! Resolve a GPU query against the live catalog: cargo run --example compute -- h100
use runpod::{
    Client, Result,
    compute::{GpuCatalogOptions, endpoint_gpu_selection, resolve_gpus},
};

#[tokio::main]
async fn main() -> Result<()> {
    let query = std::env::args().nth(1).unwrap_or_else(|| "h100".into());
    let client = Client::from_env()?;
    let catalog = client
        .list_catalog_gpus(&GpuCatalogOptions {
            product: Some("SERVERLESS".into()),
            count: Some(1),
            ..Default::default()
        })
        .await?;
    let matches = resolve_gpus(&query, &catalog)?;
    let (pools, excluded) = endpoint_gpu_selection(matches, &catalog)?;
    println!(
        "{}",
        runpod::json!({"pools":pools,"excludedTypes":excluded})
    );
    Ok(())
}

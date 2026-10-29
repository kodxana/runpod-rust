# Runpod Rust SDK

A native Rust port of [runpod-python](https://github.com/runpod/runpod-python), pinned to
`ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5`. **The complete port is in progress.**
The checked-in API and test inventories track the remaining work explicitly.

Implemented so far: all `api.ctl_commands` operations, REST and GraphQL transports,
cursor pagination, serverless endpoint clients, job polling, cancellation, streaming,
and a synchronous endpoint facade. The native worker now supports single/streaming handlers,
concurrent intake, startup hooks, heartbeats, stop signals, and progress updates. Credential
profiles, validation, cleanup, CUDA probing, and model-cache helpers are also available.
Apps resource specifications, GPU/CPU/datacenter selection, model/secret references, and
volume mount definitions are available. `apps::AppsApiClient` implements endpoint saves,
task pod provisioning, volumes, secrets, registries, application/environment lifecycle,
browser-auth requests, artifact uploads, and build deployment. GPU catalog caching is
scoped to client clones. Higher-level Apps execution/build/deployment workflows,
advanced worker utilities, and CLI remain in progress.
See [PORTING.md](PORTING.md) for behavior details and verification.

```rust,no_run
use runpod::{Client, json};
use std::time::Duration;

#[tokio::main]
async fn main() -> runpod::Result<()> {
    let client = Client::from_env()?;
    let endpoint = client.endpoint("YOUR_ENDPOINT_ID");
    let mut job = endpoint.run(json!({"prompt": "Hello"})).await?;
    let output = job.output(Duration::from_secs(60)).await?;
    println!("{output}");
    Ok(())
}
```

Use `runpod::blocking::Client` outside an async runtime for synchronous code.
Use `runpod::AsyncioEndpoint` when migrating code that depends on the Python async
client's unconditional input wrapping and zero-timeout behavior.

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo doc --locked --no-deps
```

Tests use local mock HTTP services; they do not create billable cloud resources.
Python is used only to regenerate differential fixtures, never at Rust runtime.

The destination project's Apache-2.0 license is retained. Upstream-derived code and
fixtures retain the original MIT notice in [LICENSE-UPSTREAM](LICENSE-UPSTREAM).

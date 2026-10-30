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
scoped to client clones. Shared-volume placement, storage attachment, and secret/registry
reference validation are implemented. Apps Queue/Api clients support async and blocking
calls, per-job options, reconnectable jobs, and streaming. Live targets support request
envelopes, result decoding, coordinated source synchronization, and worker metrics/logs.
Pod logs support snapshots, SSE cursors, and reconnect deduplication. Ephemeral tasks
support runtime image selection, pod creation, runner requests, result polling, and
cleanup after failure or cancellation. Advanced worker utilities and the CLI remain in progress.
`apps::deploy` provides explicit application definitions, manifest generation, deployed
endpoint payloads, endpoint reconciliation, and project archives with nested ignore rules,
protected credential exclusions, and separately vendored dependency files. `apps::build`
collects requirements, computes installed dependency closures, and prunes image-provided
packages while retaining shared files. `BuildTools` and `build_environment` install Linux
wheels through an explicitly configurable Python/pip command, report progress, and build
the artifact environment. `build_artifact` and async `deploy_app` connect preflight
validation, building, packaging, upload, activation, and endpoint reconciliation.
`apps::App` registers native synchronous, asynchronous, and streaming queue/task functions.
Handles support local calls, worker-local dispatch, remote calls, submission, reconnection,
streaming, startup hooks, and per-job options. `AppRegistry` owns declaration membership;
development target bindings and worker sibling lookup share the application's state.
API route registration, native binary deployment, and development sessions remain in progress.
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

To call an application deployed from another codebase:

```rust,no_run
use runpod::{Client, apps::{Queue, ResourceRef, StubConfig}};
use std::time::Duration;

#[tokio::main]
async fn main() -> runpod::Result<()> {
let config = StubConfig::new("my-app", ResourceRef::Name("transcribe".into()));
let queue = Queue::new(Client::from_env()?, config)?;
let input = serde_json::from_value(runpod::json!({"audio_url":"https://example.com/audio.wav"}))?;
let mut job = queue.spawn(&input).await?;
let result = job.result(Duration::from_secs(300)).await?;
println!("{result}");
Ok(())
}
```

`StubConfig::environment` overrides `FLASH_ENVIRONMENT` (otherwise `default`).
For synchronous applications, `blocking::Client::queue` and `::api` return clients
with the same operations. Their job handles and iterators retain the owned runtime.

For an ephemeral task, `apps::targets::PodTarget` accepts a task `ResourceSpec` and
explicit `FunctionDefinition` source metadata. `build_payload` encodes arguments;
`invoke` runs and cleans up the pod, while `submit` returns a `TaskJob` whose `wait`
and `cancel` clean up. An execution timeout of `None` is unlimited. `TaskConfig::images`
selects the remote interpreter (default Python 3.12); no local Python is started.
Keep the Tokio runtime alive until cancellation cleanup finishes. Dropping a submitted
job without calling `wait` or `cancel` leaves it running, matching the Python lifecycle.

For application deployment, create an `AppDefinition` with resource/symbol metadata.
Use `ResourceDefinition::set_environment` with typed `EnvValue::Secret` values to retain
secret references for preflight validation. `deploy_app` takes the core `Client` plus
`DeployOptions`; its artifact interpreter version also selects the worker image version.
Build work runs on a blocking executor. Canceling its await prevents later cloud calls,
while an already running local build finishes and releases its owned temporary files.

Native function registration uses `App::queue` or `App::task` with a `ResourceSpec`, explicit
`FunctionDefinition` parameter names, and a `NativeFunction`. Use `NativeFunction::new` for
async bodies, `::sync` for synchronous bodies, and `::streaming` for stream factories.
`FunctionHandle::local` collects generator output; `::local_stream` retains incremental output.
These methods are async, and synchronous bodies execute on Tokio's blocking pool. Local
calls never run startup hooks automatically; worker startup calls `::initialize` explicitly.
Native function bodies are compiled Rust code. Source-based live/task calls still require
explicit Python source understood by the existing remote worker; shipping native Rust binaries
is a remaining part of the port. `App::definition` returns deployment metadata without serializing closures.

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo doc --locked --no-deps
```

Tests use local mock HTTP services; they do not create billable cloud resources.
Core API clients, workers, serialization, and dependency analysis execute natively in Rust.
Building Python-compatible deployment artifacts invokes Python/pip as external build tools;
it does not invoke the Python SDK to perform SDK operations. Build tests use local stand-in
executables without installing packages. Python also regenerates differential fixtures,
and CI verifies that it can decode pickle values produced by Rust.

The destination project's Apache-2.0 license is retained. Upstream-derived code and
fixtures retain the original MIT notice in [LICENSE-UPSTREAM](LICENSE-UPSTREAM).

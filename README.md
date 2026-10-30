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
`apps::App` registers native synchronous, asynchronous, and streaming queue/task functions,
plus named HTTP routes through `NativeApi` and `ApiHandle`.
Handles support local calls, worker-local dispatch, remote calls, submission, reconnection,
streaming, startup hooks, and per-job options. `AppRegistry` owns declaration membership;
development target bindings and worker sibling lookup share the application's state.
`blocking::apps::App` supplies synchronous registered handles, jobs, and stream iterators.
`apps::manage` lists and inspects applications and tears down endpoints before their
environment or application, collecting failures and retaining parents when cleanup fails.
`apps::dev::DevSession` adopts/provisions live endpoints, refreshes configuration, rebinds
application handles, and tracks failures for cleanup retries. `run_until` awaits teardown
after a shutdown signal, including endpoint creation that finishes during cancellation.
`apps::auth` implements browser approval polling and anonymous email-code login with a
fresh verifier/challenge per attempt. Callbacks handle UI and code input; keys are returned
without opening a browser or saving credentials automatically.
`apps::entrypoint` provides an owned local-entrypoint registry and async execution, with
`blocking::apps::run_entrypoint` for synchronous callers.
Native binary deployment and HTTP server integration remain in progress.
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

For synchronous registered functions/APIs, construct `blocking::apps::App` outside Tokio,
or wrap existing declarations with `::from_app`. Register resources with the same inputs.
Returned handles, jobs, and iterators retain the runtime after the app is dropped. Use
`as_async` for native async calls or metadata configuration. Blocking methods must run in
synchronous code, outside an actively polled async task; the SDK does not create a hidden
global event-loop thread. Keep explicit task jobs until `wait` or `cancel` completes.

For HTTP APIs, add `RouteSpec`/`NativeFunction` pairs to a `NativeApi`, then call `App::api`.
Routes are emitted in handler-name order, duplicates and runtime control paths are rejected,
and one named startup hook is supported. `ApiHandle` sends GET/POST/PUT/DELETE/PATCH requests;
`initialize` and `call_handler` expose native execution to a worker/router. HTTP hosting remains
pending. `NativeApi::asgi_factory` describes a Python factory for interoperability; it does not
run Python locally. `App::resources` returns `ResourceHandle` values for mixed function/API apps.

For untrusted download URLs, use `utils::ssrf::SafeDownloader` with `DownloadOptions`.
It validates DNS results, pins connections, checks each redirect, and bounds consumed bytes.
`SafeResponse::error_for_status` rejects HTTP failure statuses; `::bytes` reads a small body,
while `iter_content_capped` yields a stream suitable for writing larger files. Responses and
streams own their transports and release them when dropped. Defaults capture the environment;
use `DownloadOptions::from_values` for an explicit configuration. The default limit is 5 GiB.
See [PORTING.md](PORTING.md#download-security-transport) for inherited IP-classification
behavior, proxy opt-in, and native transport differences.

`utils::download::FileDownloader::new(project_root)` adds file download helpers. Use
`download_files_from_urls(job_id, urls)` for ordered results with three-attempt HTTP retries,
or `file(url)` for metadata and ZIP extraction. Paths and partial files belong to the caller;
policy/filesystem failures propagate. The default single-file operation does not retry.
`job_id` is a trusted path and ZIP expansion is unbounded, matching Python; see the
[porting notes](PORTING.md#file-downloads-and-zip-extraction). Upload helpers remain in progress.

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

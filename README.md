# Runpod Rust SDK

A native Rust port of [runpod-python](https://github.com/runpod/runpod-python), pinned to
`ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5`. **The complete port is in progress.**
The checked-in API and test inventories track the remaining work explicitly.
The current minimum Rust version is **1.94.1**, required by the native AWS S3 SDK.

Implemented so far: all `api.ctl_commands` operations, REST and GraphQL transports,
cursor pagination, serverless endpoint clients, job polling, cancellation, streaming,
and a synchronous endpoint facade. The native worker now supports single/streaming handlers,
concurrent intake, startup hooks, heartbeats, stop signals, and progress updates. Credential
profiles, validation, cleanup, CUDA probing, and model-cache helpers are also available.
`serverless::PingJobMirror` provides a bounded job-ID snapshot shared by explicit clones.
Attach one with `JobsProgress::set_mirror(Some(mirror.clone()))` to publish current jobs
and subsequent add/remove/clear updates. Independent trackers never share state implicitly.
The mirror preserves Python's byte truncation, embedded-NUL and invalid-UTF-8 behavior.
It shares memory across native threads; independent heartbeat execution/retry wiring is
still pending, and the current worker heartbeat uses the caller's async executor.
`serverless::WorkerApi` hosts the development `/run`, `/runsync`, `/status`, and `/stream`
routes, optional endpoint-specific realtime calls, and interactive API documentation.
The development `/run` route stores a job; polling `/status` or `/stream` executes it.
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
The native `rp` and `runpod` commands support catalog search and network/global volume
listing, inspection, creation, renaming, expansion, and deletion. Endpoint resource
commands support configuration, editing, scaling, deletion, workers, releases and rollout
tracking, with JSON and terminal interaction. Job commands inspect, wait, stream, cancel and retry jobs;
queue purge confirms before removing queued jobs. `endpoint run` supports queue submission
and load-balancer requests. `pod logs` and `endpoint logs` show snapshots or follow live SSE
output, with cursor resumption, worker selection, source filters and JSON lines.
Pod resource commands list, inspect, create, update, start, stop, restart and terminate
GPU/CPU pods, with hardware selection, mount/environment edits and readiness observation.
Omitted pod IDs open a picker in a terminal, including for logs. SSH connect/sync,
shell completion and the remaining CLI groups remain pending.
`configuration` provides typed container, scaling, and endpoint options; async and
blocking clients resolve selections into creation/update payloads. Editable JSON
documents support validation and minimal patches. These helpers return payloads
without modifying resources; the endpoint resource commands use these helpers.
See [examples/configuration.rs](examples/configuration.rs).
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
`ApiHandle::serve` and `start` host native JSON or raw HTTP API routes, run initialization
before accepting requests, expose `/ping` and API documentation, and drain requests
on shutdown. `NativeApi::http_factory` hosts an owned Axum router with startup and cleanup.
Live code loading remains in progress.
See [examples/apps_http.rs](examples/apps_http.rs) for a runnable HTTP service.
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
Native task deployment, the remaining CLI commands and live HTTP runtime integration remain in progress.
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
explicit Python source understood by the existing remote worker. Native queue/API deployment
accepts a compiled Linux worker; native task deployment remains in progress. `App::definition` returns deployment metadata without serializing closures.

Registered functions can serve platform queue jobs with `FunctionHandle::start_queue_worker`.
It runs the init hook before core startup, uses `RUNPOD_MAX_CONCURRENCY`, and selects the
core worker's queue, realtime HTTP, or local `test_input.json` mode. Job input is keyword
arguments; `__empty` is removed. Streaming functions send partial results and a final
aggregate. For explicit configuration and shutdown, await `queue_worker(WorkerConfig)`
then call `Worker::run`. Apps initialization completes before intake, independently of
the core worker's later prestart hooks. See [examples/queue_worker.rs](examples/queue_worker.rs).
Native task deployment and live source loading remain in progress.

`apps::bootstrap::BootstrapConfig` prepares a platform-mounted artifact before worker
startup. `locate` prefers a prebuilt tree or warm unpack marker, otherwise waits for and
extracts tar/gzip/bzip2/xz content. `read_manifest` reads its JSON and `dependency_paths`
returns vendored-environment/source order. These are blocking operations; use a blocking
executor from async code. `deploy_native_app` connects this to native queue/API startup.

`apps::bootstrap::start_deployed` connects preparation to a native queue, API or task worker.
Pass a loader that accepts the manifest module and a `DeployedContext`; use `context.app_dir()`
for bundled assets and `context.mounts()` for volume bindings. It validates task identity,
extracts or reuses the artifact, installs the selected resource's system packages, and starts
the worker with its existing initialization and error behavior. It does not modify the
process environment or working directory. See [examples/bootstrap_queue.rs](examples/bootstrap_queue.rs)
for app `greetings`, resource `hello`, module `bootstrap_queue`, with a mount named `data`.
Use `RUNPOD_PREBUILT_APP_DIR` for an already-unpacked tree or `FLASH_BUILD_ARTIFACT_PATH`
and `RUNPOD_APP_DIR` for extraction. Native crate dependencies are resolved during compilation.

`apps::deploy::build_native_artifact(&app.definition()?, project_root, executable, output, events)`
packages an already-compiled worker at `runpod-native/worker`, project assets and a manifest.
It uses the existing source ignore rules, additionally excludes the root `target/` build
directory, and gives the archived worker executable permissions. Supply a binary built for
the intended container OS, architecture and libraries; packaging does not cross-compile it.
An unpacked artifact can be run through that worker's `start_deployed` entry point.

`apps::deploy::deploy_native_app(client, app, project_root, executable, environment, events)`
packages and uploads a compiled worker, activates the build and configures queue/API
endpoints to execute it. It requires a Linux x86_64 ELF header and explicit resource symbols.
Compile against the selected container's libraries; header validation does not verify those
libraries or the entire executable. Built-in images retain their existing `py3.12` image tag
and manifest image-version field, while the startup command executes the native binary.
Custom images need `sh`, `tar` (gzip/stdout extraction), `mkdir`, `chmod`, `sleep`, compatible
libraries and an entrypoint that accepts the generated command. The launcher honors prebuilt
trees and warm markers; cold starts extract a separate bootstrap copy before Rust prepares
the full archive. Prelaunch failures appear on stderr; structured error reporting starts
inside the worker. This path is covered by local HTTP and process tests; live Runpod
compatibility still needs validation. Native task resources are rejected before mutations
until coordinator task launch is wired. `deploy_app` continues to build Python-compatible artifacts.

`rp flash deploy [TARGET]` (also `runpod flash deploy`) discovers Cargo app declarations,
builds their workers, and deploys each app in order. TARGET is a Cargo directory, manifest,
or binary source; it defaults to the current directory. The binary must call
`discovery::export_if_requested` after registration and use `start_deployed` for deployed
startup. Assets are taken from each owning Cargo package directory.

```sh
# Requires an installed target standard library, linker and compatible native libraries.
rp flash deploy ./my-worker --env production
rp flash deploy ./my-worker --build-only
# Supply a worker already built for the intended Linux x86_64 container.
rp flash deploy ./my-worker --worker ./dist/worker
```

The worker build defaults to `--target-triple x86_64-unknown-linux-gnu --profile release`.
Cargo runs with `--locked` and each binary's required features. No toolchains or linkers
are installed automatically. `--worker` cannot be combined with target/profile flags and
requires declarations from a single binary. `--build-only` writes `<app>-artifact.tar.gz`
in the current directory without credentials or cloud requests. Packaging accepts a host
target for local checks; deployment separately requires Linux x86_64. Both retain Cargo's
cache, and native artifacts exclude the root `target/` directory. Library callers can use
`DiscoveredApp::build_worker(&WorkerBuildOptions)` to obtain Cargo's reported executable.

`--runtime python` selects the existing Python-compatible artifact builder for explicit
Python symbols/assets exported by Cargo metadata. It accepts `--python-version` and
`--exclude torch,numpy`; otherwise it probes the configured Python executable and falls
back to 3.12 for an unsupported version. It does not translate Rust handlers into Python.
Exclude `target/` through `.gitignore` or `.runpodignore` in these mixed-language projects.
The native default requires no Python interpreter. Deployment stops on the first failure;
apps already deployed remain active. See [reviews/deploy-cli.md](reviews/deploy-cli.md)
for validation and remaining platform limits.

`apps::runtime::DeployedContext` reads the worker identity and validates `RUNPOD_MOUNTS`
before loading application declarations. `load_queue` and `load_api` select the first
matching handle from an explicit compiled-module callback; start it with the queue or
HTTP methods above. `load_task` requires exactly one matching app/task/qualified name
and awaits its init hook. The callback receives owned-context mount bindings, which
handlers can clone for their lifetime.
See [examples/deployed_queue.rs](examples/deployed_queue.rs) for an already-unpacked
manifest selecting module `deployed_queue`, resource `hello`, with a network mount named
`data`. Set `RUNPOD_APP_DIR`, `FLASH_RESOURCE_NAME` and `RUNPOD_MOUNTS` for the worker;
the example also supports compiled metadata discovery.

When artifact preparation returns a `PhaseError`, call `apps::bootstrap::report_error(&error).await`
to keep startup failures visible through the worker's transport. Queue workers report the
error for each job; APIs return HTTP 500; tasks retain public readiness and authenticated
error details. Embedded callers can use `QueueErrorReporter::run` or `serve_api_error`
with their own shutdown future. These responders require no application registration.

`apps::task_runner::start` hosts a deployed task on `RUNPOD_TASK_PORT` (default 8080).
Pass an initializer that calls `DeployedContext::load_task`; `/ping` reports readiness
while initialization runs. `/execute`, `/submit`, `/result` and `/timeout` require
`Authorization: Bearer $RUNPOD_TASK_TOKEN`. One execution is accepted per process, and
request timeouts can only shorten its deadline. Initialization/execution caps and idle
cleanup use the runtime environment settings. Pod deletion requires acknowledgement
using the pod-scoped `RUNPOD_API_KEY` and `RUNPOD_POD_ID` before the process exits.
See [examples/deployed_task.rs](examples/deployed_task.rs).

For embedded hosting, `TaskRunner::serve` takes a listener, initializer, deletion callback
and shutdown future; it returns `TaskExit` instead of exiting the process. Response stdout
uses `serverless::capture::stdout()` or `TeeWriter`, including synchronous functions and
streams. Ordinary `println!` is not intercepted. Native task deployment and live-source
dispatch remain in progress.

For synchronous registered functions/APIs, construct `blocking::apps::App` outside Tokio,
or wrap existing declarations with `::from_app`. Register resources with the same inputs.
Returned handles, jobs, and iterators retain the runtime after the app is dropped. Use
`as_async` for native async calls or metadata configuration. Blocking methods must run in
synchronous code, outside an actively polled async task; the SDK does not create a hidden
global event-loop thread. Keep explicit task jobs until `wait` or `cancel` completes.

For HTTP APIs, add `RouteSpec`/`NativeFunction` pairs to a `NativeApi`, then call `App::api`.
Routes are emitted in handler-name order, duplicates and runtime control paths are rejected,
and one named startup hook is supported. `ApiHandle` sends GET/POST/PUT/DELETE/PATCH requests;
`initialize` and `call_handler` expose direct native execution. `serve` or `start` hosts the
registered routes. Use `http_route` with `NativeHttpFunction` for an Axum `Response`, preserving
status, repeated headers, binary data, and `Body::from_stream` output. `call_http_handler`
retains that response for direct calls. These integrations use Axum 0.8 types.
`NativeApi::http_factory` instead accepts an async factory returning `NativeHttpApp`:
the factory performs startup, and `on_shutdown` registers cleanup after requests drain.
`FactoryHealth::Add` supplies GET health when the router has no `/ping` route;
`Preserve` retains an existing route of any method, and `Wrap` intercepts every method.
See [examples/apps_http_factory.rs](examples/apps_http_factory.rs).
`NativeApi::asgi_factory` describes a Python factory for interoperability; it does not
run Python locally. `App::resources` returns `ResourceHandle` values for mixed function/API apps.

Serverless workers accept a `ConcurrencyModifier` in `WorkerConfig`. Returning zero pauses
intake; changing capacity waits for existing work to drain. `Worker::control` returns a
cloneable handle for per-job cancellation, shutdown, and concurrency/occupancy observation.
`Worker::run` accepts your shutdown future; `run_until_signal` handles Ctrl-C and Unix SIGTERM.
Startup hooks run alongside intake and gate execution. Register blocking hooks with
`PrestartHooks::register_sync_prestart_hook` so they can time out without blocking the runtime.
`Worker::start` adds the production process policy: fitness checks, panic logging, signals,
and hard exit after a reported prestart failure. `start_local` applies the prestart exit policy
to a local job. The lower-level run methods return typed `Error::Prestart` failures instead.
See the
[worker notes](PORTING.md#worker-scaling-and-startup-coordination) for failure reporting and
remaining runtime work.

Worker failures can include scoped log tails. Use `capture::stdout().write_text(...)`,
`capture::stderr()`, or `capture::TeeWriter` to forward text and make it available to capture.
The default `Auto` policy captures prestart output; `All` also attaches handler logs, and
`Off` disables attachment. Set `WorkerConfig::capture_mode` explicitly, or read
`RUNPOD_LOG_CAPTURE` through `WorkerConfig::from_env`. Ordinary `println!` and external
process output are not intercepted. `Capture` exposes async/sync scopes and explicit context
propagation for user-spawned tasks. See the [capture notes](PORTING.md#scoped-capture-and-failure-reports).
`run_job_generator` yields partial output envelopes and a terminal error envelope;
`run_job_with_capture` and `run_job_generator_with_capture` accept an owned capture policy.

`serverless::health` exposes native GPU allocation/fallback probes and memory, disk, TCP,
and CUDA-version checks. `GpuChecks::from_values` reads explicit environment values and
uses the bundled Linux x86_64 allocation binary unless an existing override is supplied.
`SystemChecks` accepts owned thresholds and optional executable overrides. Probes have
bounded process/network lifetimes and need no Python interpreter. Production `serverless::start`
runs fitness checks before intake. Use an owned `FitnessChecks` registry with
`start_with_fitness` for custom checks and retained early-check state. Call its synchronous
`run_startup_fitness_checks` before initializing the handler or Tokio runtime when early
checks are wanted. Failed worker checks report unhealthy and terminate the process;
`FitnessChecks::check` instead returns a typed failure for library callers.
GPU framework checks use explicit `GpuFramework` adapters supplied by the handler and
never load a framework implicitly. See the [lifecycle notes](PORTING.md#fitness-lifecycle-and-framework-checks).

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
[porting notes](PORTING.md#file-downloads-and-zip-extraction).

GPU/CPU/region selection is available through `runpod::compute`. Fetch live records
with `Client::list_catalog_gpus`, `list_catalog_cpus`, or `list_catalog_datacenters`,
then use `resolve_gpus`, `resolve_cpu`, or `resolve_regions`. Blocking clients expose
the same catalog methods. See [examples/compute.rs](examples/compute.rs) and the
[compute porting notes](PORTING.md#compute-selection-and-catalog-requests).

To install the native CLI from this checkout, run `cargo install --path . --locked`.
Set `RUNPOD_API_KEY` or use an existing default credential profile, then run:

```sh
rp catalog h100 --count 2 --region eu --cuda '>=12.4'
rp catalog --cpu --vcpu 8 --available --json
runpod catalog --datacenters --region us
```

Catalog commands only query existing catalog data. See `rp catalog --help` and the
[catalog CLI notes](PORTING.md#native-catalog-cli) for supported options and display adaptations.

Volume commands support network volumes by default and global volumes with `--global`:

```sh
rp volume list --json
rp volume get VOLUME_ID
rp volume create --name models --size 100 --data-center EU-RO-1
rp volume expand VOLUME_ID --size +50
rp volume create --name shared --global
```

Omitting an ID for `get` opens a fuzzy selector in an interactive terminal. Region-based
creation narrows by storage tier and selects a data center when several match. Deletion
asks for confirmation; scripts must explicitly pass `--yes`. See `rp volume --help`
and the [volume CLI notes](PORTING.md#native-volume-cli).

Endpoint commands support queued and load-balanced resources:

```sh
rp endpoint list --name 'llama*' --type queue --json
rp endpoint create --name inference --type queue --image IMAGE --gpu h100
rp endpoint update ENDPOINT_ID --image IMAGE --wait
rp endpoint edit ENDPOINT_ID
rp endpoint edit ENDPOINT_ID --file config.json --yes
rp endpoint scale ENDPOINT_ID --min-workers 0 --max-workers 4
rp endpoint workers ENDPOINT_ID --watch --json
rp endpoint releases ENDPOINT_ID
rp endpoint job get ENDPOINT_ID JOB_ID --json
rp endpoint job wait ENDPOINT_ID JOB_ID --timeout 30
rp endpoint job stream ENDPOINT_ID JOB_ID --json
rp endpoint job cancel ENDPOINT_ID JOB_ID
rp endpoint job retry ENDPOINT_ID JOB_ID
rp endpoint purge ENDPOINT_ID --yes
rp endpoint run ENDPOINT_ID --input request.json
rp endpoint run ENDPOINT_ID --input request.json --async
rp endpoint run ENDPOINT_ID --input - --stream
rp endpoint run LB_ENDPOINT_ID --path /v1/models
rp pod logs POD_ID --tail 100
rp endpoint logs ENDPOINT_ID --worker WORKER_ID --source container
rp endpoint logs ENDPOINT_ID --follow --json
rp pod list --status starting --json
rp pod get POD_ID
rp pod create trainer --gpu h100:2 --image IMAGE --region eu --ssh --wait
rp pod create box --cpu cpu3c --vcpu 4 --image ubuntu:24.04
rp pod update POD_ID --env MODEL=qwen --unset-env OLD_MODEL --network-volume VOLUME_ID
rp pod restart POD_ID --wait
rp pod stop POD_ID
rp pod terminate POD_ID --yes
```

`edit` uses `$VISUAL` or `$EDITOR`, validates JSON, shows the patch, and asks before
applying it. `--file -` reads stdin. Updates can follow the new release with `--wait`;
Ctrl-C detaches while the remote rollout continues. With `--json`, update results go
to stdout and rollout progress goes to stderr. Worker watch emits one JSON document
per refresh. See `rp endpoint --help` and the [endpoint CLI notes](PORTING.md#native-endpoint-resource-cli).
Job waiting bounds each HTTP request by the remaining deadline. Streaming drains terminal
chunks, prints incremental output and retains failed-job details. Ctrl-C leaves the job
running and prints a resume command. `endpoint run` accepts inline `--data`, a file or stdin;
queue requests submit jobs and load-balancer requests use the endpoint URL and requested
path/method. `--timeout` starts after queue submission and bounds result observation.
Log snapshots request up to 5,000 backfill lines per worker and sort the returned timestamp
strings. `--follow` reconnects with the last event cursor and discovers new workers;
Ctrl-C closes the streams and exits successfully. `--since` passes an earliest timestamp
to the API. Shell completion remains in progress.
See [job observation notes](PORTING.md#native-job-waiting-and-streaming) and
[log command notes](PORTING.md#native-pod-and-endpoint-log-commands).

Pod creation selects matching GPUs by available stock in the requested data centers,
then price and name. Updates preserve existing environment entries and omitted mount
paths; `--lock` / `--unlock` and networking flags retain explicit false values.
`start --wait`, `restart --wait` and `create --wait` check actual container readiness.
Restart observation rejects stale pre-action runtime data. Termination identifies the
pod before asking for confirmation; `--yes` skips that lookup and prompt.
See [pod command notes](PORTING.md#native-pod-resource-commands).

Authenticate with browser approval, an API key, or an emailed verification code:

```sh
rp login
rp login --no-open
rp login --api-key YOUR_API_KEY --profile staging
rp login --agent --email owner@example.com --profile agent
```

Email sign-in requires an interactive terminal. Keep the same process running until
the code is accepted. Codes stay hidden; invalid formats are rejected before submission.
Login replaces only the selected profile in `~/.runpod/config.toml`. Cancellation or
authentication failure preserves existing credentials. See the
[login notes](PORTING.md#native-login-command).

Registry credentials and platform secrets have native `add`, `list`, and `delete` commands:

```sh
rp registry add dockerhub --username my-user
rp registry list
rp secret add API_TOKEN --description "Upstream API token"
rp secret list
rp secret delete API_TOKEN --yes
```

Omitted passwords and secret values use hidden terminal prompts. Scripts can supply
`--password` or `--value`; deletion requires `--yes` outside an interactive terminal.
Lists show names and metadata, without secret values. See the
[credential command notes](PORTING.md#native-registry-and-secret-commands).

Inspect and delete deployed applications:

```sh
rp flash app list
rp flash app get demo
rp flash app delete demo --yes
```

Deletion cleans up each environment's endpoints before removing the environment and
app. Failures are reported in order; failed child cleanup retains its parent. Ctrl-C
stops further requests; completed deletions are not rolled back. Local discovery and
deployment commands remain in progress. See the
[app command notes](PORTING.md#native-app-management-commands).

Manage environments and undeploy their endpoints while retaining the app and build history:

```sh
rp flash env list --app demo
rp flash env get prod --app demo
rp flash env add staging --app demo
rp flash env delete staging --app demo --yes
rp flash undeploy --app demo --env default --yes
```

Without `--app`, these commands build and inspect the current Cargo project's binary
declarations and require one unique app name. Native projects register their apps in an
`AppRegistry`, call `apps::discovery::export_if_requested`, and return before local
entrypoints when discovery is requested; [examples/discovery.rs](examples/discovery.rs)
shows the pattern. `--app` bypasses discovery. The discovery API returns metadata and
executable paths; live-handle discovery and native CLI deployment remain in progress.
See the [environment and discovery notes](PORTING.md#native-environments-and-compiled-discovery).

`serverless::VolumeCache` (also `utils::VolumeCache`) keeps local cache directories on a
mounted volume. Construct it with `[local_directory]` and `VolumeCacheConfig`; defaults
use `/runpod-volume` and `RUNPOD_ENDPOINT_ID`. `hydrate` restores files, `sync_inline`
copies changes synchronously, and `sync` schedules background work. `enter` returns a
guard that hydrates on entry and schedules sync on drop. These filesystem operations
are blocking; retain the cache and `flush` background work before process exit.
Small files share a tar archive; large files hydrate with bounded parallel range reads.
See [examples/volume_cache.rs](examples/volume_cache.rs) and the
[cache behavior notes](PORTING.md#directory-volume-cache).

`utils::upload::Uploader::new(project_root)` provides native S3 uploads with local fallback.
Use `upload_file_to_bucket` or `upload_in_memory_object` with an `UploadRequest` for bucket,
prefix, and boto3-compatible extra arguments. Credentials come from `BUCKET_ENDPOINT_URL`,
`BUCKET_ACCESS_KEY_ID`, and `BUCKET_SECRET_ACCESS_KEY`, or an explicit `UploadOptions`.
Missing credentials save to `local_upload`; configured S3 failures propagate. Uploads use
SigV4, automatic multipart transfers, and seven-day presigned GET URLs. `upload_image`,
ordered concurrent `files`, and sequential `bucket_upload` preserve their Python contracts.
No Python process is involved. See the [upload notes](PORTING.md#file-and-s3-uploads) for
credential-provider, cancellation, and transport adaptations.

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo doc --locked --no-deps
```

Development and test builds omit debug information and incremental caches to reduce
disk use. Debug assertions and overflow checks remain enabled. Reuse the default
`target/` directory for focused checks instead of keeping separate toolchain caches;
`cargo clean` removes generated artifacts when they are no longer needed. For a debugger
session, override `CARGO_PROFILE_DEV_DEBUG=2` (or `CARGO_PROFILE_TEST_DEBUG=2` for tests).
See Cargo's [profile settings](https://doc.rust-lang.org/cargo/reference/profiles.html).

Tests use local mock HTTP services; they do not create billable cloud resources.
Core API clients, workers, serialization, and dependency analysis execute natively in Rust.
Building Python-compatible deployment artifacts invokes Python/pip as external build tools;
it does not invoke the Python SDK to perform SDK operations. Build tests use local stand-in
executables without installing packages. Python also regenerates differential fixtures,
and CI verifies that it can decode pickle values produced by Rust.

The destination project's Apache-2.0 license is retained. Upstream-derived code and
fixtures retain the original MIT notice in [LICENSE-UPSTREAM](LICENSE-UPSTREAM).

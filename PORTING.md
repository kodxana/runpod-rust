# Porting status and behavior

Source: `runpod/runpod-python` commit `ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5`.
The source inventory contains 1,135 classes/functions, of which 662 have public names,
and 1,659 test functions. These counts include internal implementation modules exposed
by Python; the complete port remains in progress.

## API mapping

| Python | Rust |
| --- | --- |
| `runpod.api_key`, base URL globals | owned `ClientConfig` |
| `api.rest.run_rest_request`, `run_rest_request_async` | `Client::run_rest_request`, `blocking::Client::run_rest_request` |
| `api.rest.run_rest_paginated_request_async` | `Client::run_rest_paginated_request` |
| `api.graphql.run_graphql_query`, `run_graphql_query_async` | `Client::run_graphql_query`, `blocking::Client::run_graphql_query` (anonymous mode pending) |
| All 17 `api.ctl_commands` functions | identically named `Client` methods; option bags replace long parameter lists |
| `endpoint.runner.Endpoint` | `Endpoint` (async Rust methods), `blocking::Endpoint` |
| `endpoint.runner.Job` | `Job`, `blocking::Job` |
| `endpoint.asyncio.Endpoint`, `Job` | `AsyncioEndpoint`, `AsyncioJob` |
| `endpoint.helpers.is_completed`, `FINAL_STATES` | `endpoint::is_completed`, `endpoint::FINAL_STATES` |
| AuthenticationError / QueryError | `Error::Authentication` / `Error::Query` |
| `cli.groups.config.functions` | `credentials::CredentialStore` methods |
| `rp_job.run_job`, `handle_job`, `get_job`, `get_stop_signals` | `serverless::run_job`, `Worker::handle_job`, `WorkerTransport` methods |
| `rp_http.send_result`, `stream_result`; `rp_progress.progress_update` | `WorkerTransport` methods |
| `rp_prestart` registration, enumeration, clearing, execution | owned `PrestartHooks` registry |
| `rp_prestart` blocking-hook invocation and raw hook sequence | `PrestartHooks::{register_sync_prestart_hook,run_prestart_hooks_async}` |
| `PrestartError`, `PrestartTimeout` | `PrestartError::{Failed,Timeout}` with hook name and typed cause, carried by `Error::Prestart` |
| `rp_capture` mode selection, text buffer, and clipping | `serverless::capture::{CaptureMode,RingBuffer,clip}` |
| `rp_capture` tee, capture scopes, and pause | explicit `TeeWriter`, `Capture::{scope,scope_sync,current,text}`, `paused`, and `paused_sync` |
| `rp_prestart` failure/cancellation payloads and paused progress | `build_prestart_failed_payload`, `build_prestart_cancelled_payload`, and scoped progress exclusion |
| `rp_job.run_job_generator` | `run_job_generator`, `run_job_generator_with_capture` using native `Handler` streams |
| `worker_state.Job`, `JobsProgress` | `WorkerJob`, `JobsProgress` |
| `rp_scale.JobScaler` concurrency, intake, dispatch, stop, shutdown, and startup coordination | `Worker`, `WorkerControl`, `ConcurrencyModifier`, and private `IntakeSchedule` |
| `JobScaler.start`, uncaught-exception logging, prestart process exit | `Worker::start`, native panic hook, and `Worker::start_local`; lower-level runs retain typed errors |
| `_health.gpu` parsing, configuration, allocation binary, and nvidia-smi fallback | `serverless::health::{GpuChecks, GpuConfig, GpuTestOutput, HealthError, parse_gpu_test_output}` |
| `_health.system` memory, disk, TCP readiness, and CUDA-version probes | `serverless::health::{SystemChecks, SystemConfig, MemoryInfo, DiskInfo, parse_version, network_probe_target}` |
| `_health.fitness` registration, early/start passes, settings refresh, reporting, and hard exit | `serverless::health::{FitnessCheck, FitnessChecks, FitnessPass, FitnessFailure, report_unhealthy, terminate_unhealthy}` |
| `_startup.run_import_checks` | explicit `FitnessChecks::run_startup_fitness_checks` before handler/runtime initialization |
| Loaded Torch/CuPy lookup, CUDA initialization, compute benchmark | explicit `GpuFrameworks` / `GpuFramework` adapters and `SystemChecks` methods |
| `_binary_helpers.get_binary_path`, serverless/early eligibility, environment flags | `serverless::health::{get_binary_path, is_serverless_environment, is_early_check_eligible}` and private flag helper |
| `rp_validator.validate` | `utils::validator::validate` with typed ordered rules |
| `rp_model_cache` path helper, `rp_cleanup.clean`, CUDA availability | `utils` functions |
| `rp_cli.compute` matching, parsing, catalog requests | `compute` helpers and `Client::list_catalog_*` / `blocking::Client::list_catalog_*` |
| `rp_cli.catalog`, JSON and table output | native `rp catalog` / `runpod catalog`, `cli::run`, and private rendering helpers |
| `rp_cli.volume`, CLI pagination, fields and terminal interaction | native `rp volume` / `runpod volume`, private owned options, pagination and prompt helpers |
| `rp_cli.options::{parse_env,container_fields}` | `configuration::parse_env`, async/blocking `Client::container_fields` |
| `rp_cli.endpoint_config::{_gpu,_cpu,config_body}` | private hardware selection and async/blocking `Client::{create_endpoint_config,update_endpoint_config}` |
| `rp_cli.endpoint_config::{scale_patch,mutable,_kind,validate,patch}` | `configuration::{scale_patch,mutable_endpoint,validate_endpoint_document,endpoint_patch}` and native type labels |
| `rp_cli.endpoint` resource commands, formatting, editor and rollout/worker observation | native `rp endpoint` / `runpod endpoint`, private options, views, clock and streaming-output helpers |
| `rp_cli.endpoint` job inspection, cancellation, retry, queue purge and data-plane errors | native `endpoint job get/cancel/retry`, `endpoint purge`, private `cli::endpoint::jobs` |
| `rp_cli.endpoint` job waiting, streaming and final output | native `endpoint job wait/stream`, private `cli::endpoint::job_follow::Follow` |
| `rp_cli.endpoint` request-body loading, submission and load-balancer calls | native `endpoint run`, private `cli::endpoint::run::RunOptions` |
| `rp_cli.logs::{_show_logs,show_logs}`, `rp_cli.endpoint.logs`, `rp_cli.pod.logs` | native `endpoint logs` / `pod logs`, private `cli::logs::Options` and SDK `LogsClient` |
| `rp_cli.pod` resource commands, selection, payloads and readiness | native `pod list/get/create/update/start/stop/restart/terminate/delete`, private `cli::pod` modules |
| `rp_cli.main` registry/secret groups and add/list/delete commands | native `registry` / `secret`, private `cli::credentials::{Kind,Operation}` |
| `rp_cli.main.login` | native `rp login` / `runpod login`, private `cli::login::Options` and SDK auth/storage helpers |
| `rp_cli.main` app group and list/get/delete commands | native `flash app`, private `cli::flash` and existing `apps::manage` helpers |
| `rp_cli.main` environment commands, undeploy, timestamp formatting and app-name resolution | native `flash env` / `flash undeploy`, private `cli::environment` and compiled metadata discovery |
| `rp_volume_cache.VolumeCache` | `utils::VolumeCache` / `serverless::VolumeCache`, `VolumeCacheConfig`, and owned scope/background guards |
| `apps.Model`, `Secret`, environment rendering | `apps::Model`, `Secret`, `render_env`, `secret_names` |
| `apps.spec.ResourceSpec`, `ResourceKind`, `RouteSpec`, workers | `apps::ResourceSpec`, `ResourceKind`, `RouteSpec`, `Workers` |
| GPU/CPU/datacenter enums and GPU selection | `apps::catalog`, `apps::gpu` |
| Volume references and mount validation | `apps::Volume`, `NetworkVolume`, `GlobalVolume`, `MountBindings`, `apps::volume` |
| `apps.targets.QueueClient`, `SentinelTarget` | `apps::targets::QueueClient`, `Target::sentinel` |
| `apps.Job`, queue/API stubs | `apps::Job`, `Queue`, `Api`, plus `blocking::apps` equivalents |
| `apps.protocol`, serialization | typed wire envelopes and native Serde/pickle functions; language adaptations below |
| Live target payloads, decoding, source synchronization, monitoring | `Target::live`, `FunctionDefinition`, `FunctionSource`, `set_events`, `set_monitor_clients` |
| `apps.logs` snapshots, SSE streams, tail summaries | `apps::logs::LogsClient`, `LogStreamOptions`, `tail_summary` |
| `apps.monitor` worker state/log observation and filtering | `apps::monitor::WorkerMonitor`, `PodLogStream`, and helper functions |
| Runtime image/version selection and custom-image shell launcher | `apps::images::RuntimeImages`, `runtime_tag`, `apps::shim::runtime_launcher` |
| Ephemeral task pod lifecycle and runner transport | `apps::tasks::TaskExecution`, `TaskConfig`, `pod_input` |
| `apps.tasks.TaskJob`, task result decoding and cleanup | `apps::tasks::TaskJob`, `unwrap_task_response`, `finish_cleanup` |
| `apps.targets.PodTarget` | `apps::targets::PodTarget`, explicit function metadata and async invoke/submit |
| `apps.deploy` manifests, deployed endpoint payloads, reconciliation, result type | `apps::deploy::{build_manifest, deployed_endpoint_input, reconcile_endpoints, DeployResult}` with `AppDefinition` metadata |
| `apps.deploy.package_project` and ignore helpers | `apps::deploy::{package_project, PackageOptions, Artifact}` and private scoped ignore matching |
| `apps.build` requirement selection, installed closures, and pruning | `apps::build::{requirement_name, collect_requirements, split_exclusions, installed_closure, prune_excluded}` |
| `apps.build` installer, SDK overlay, complete environment construction | `apps::build::{BuildTools, VendorOptions, BuildOptions, BuildResult, sync_running_package, build_environment}`, `AppError::Build` |
| `apps.deploy.build_artifact`, `deploy_app`, `_module_path_for` | `apps::deploy::{build_artifact, deploy_app, module_path_for}` with explicit metadata and owned artifacts |
| `apps.App` queue/task registration and process registry | `apps::{App, AppConfig, AppRegistry}`, explicit registration and owned state |
| Function handle local/async remote, spawn, job, stream, init, options | `apps::{FunctionHandle, NativeFunction, FunctionArgs, FunctionJob}` plus `blocking::apps` facades |
| Context detection, discovery guard, development endpoint names | `apps::{Context, ExecutionContext, current_context, is_local, in_discovery, dev_endpoint_name}` |
| `schedule` intent marker | `FunctionHandle::schedule` or an explicit `ResourceSpec::schedule` |
| `App.api`, `ApiHandle`, route/lifecycle markers | `App::api`, `NativeApi`, `ApiHandle`, and explicit `RouteSpec`/hook registration |
| Invoker/StreamInvoker sync and async access | native async handle methods and `blocking::apps::{App, FunctionHandle, ApiHandle, FunctionJob, OutputIter}` |
| `apps.manage` inspection, undeploy, and application deletion | `apps::manage` with `UndeployResult` and typed `AppError` variants |
| `apps.dev` payloads, adoption, refresh, cleanup, and context management | `apps::dev::{DevConfig, DevSession, endpoint_input, comparable_payload, changed_fields}` |
| `apps.auth` browser approval and email verification | `apps::auth::{browser_login, BrowserLoginOptions, auth_url, EmailLoginClient}`, `AppError::Login` |
| `local_entrypoint`, entrypoint lookup/execution/clearing | `apps::entrypoint::{EntrypointRegistry, run_entrypoint}`, `blocking::apps::run_entrypoint` |
| SSRF address checks, pinned requests, redirects, proxy refusal, and byte caps | `utils::ssrf::{SafeDownloader, DownloadOptions, SafeResponse, SsrfError, is_safe_address, iter_content_capped}` |
| File chunk/header helpers, batch download, single-file download and ZIP extraction | `utils::download::{calculate_chunk_size, parse_content_length, extract_disposition_params, FileDownloader, DownloadedFile, download_files_from_urls, file}` |
| Local fallback, region extraction, bucket client/configuration, image/file/memory and custom-bucket uploads | `utils::upload::{Uploader, UploadOptions, UploadRequest, BucketCredentials, TransferConfig, get_boto_client, extract_region_from_url}` and corresponding free functions |

The detailed symbol and test ledgers are [parity/api.json](parity/api.json) and
[parity/tests.json](parity/tests.json). Pending entries must not be interpreted as
implemented merely because a broad Rust module exists.
The ledger labels serialization/reflection and runtime-lifecycle adaptations explicitly.
This milestone maps 398 implemented symbols, 420 adapted symbols, and 928 upstream
test functions; other entries remain pending.
The test mapping includes 861 ported tests and 67 explicitly adapted tests.

## Test mapping and reproduction

`tests/transport.rs` ports REST authentication, custom URL, empty response, problem
JSON, fallback errors, and pagination contracts from `tests/test_api/test_rest.py`.
`tests/endpoint.rs` ports the endpoint/job contracts in `tests/test_endpoint/test_runner.py`
and async-specific behavior in `test_asyncio_runner.py`, plus blocking streaming.

`tests/control.rs` replays 47 real Python wire fixtures in `tests/fixtures/control.json`.
Each fixture records its upstream test name where applicable, inputs, HTTP requests,
server replies, and returned output or error. They cover every ctl_commands operation,
template precedence, per-GPU host minima, storage, registry credentials, GPU exclusions,
CPU parsing, and rejected legacy parameters.

`tests/worker.rs` replays 19 Python result envelopes captured by
`tools/worker_fixtures.py`. Worker transport tests check job intake and result webhooks.
`tests/worker_runtime.rs` exercises startup/intake interleaving, startup failure reporting,
stream aggregation, and cancellation isolation against actual local HTTP servers.
Credential tests verify atomic replacement, comment preservation, and refusal to overwrite
invalid files. Utility tests cover typed validation and Python's default/constraint quirk.
`tests/apps_spec.rs` checks 24 Python manifest/validation fixtures. Apps value and volume
tests check model references, secret expansion, GPU exclusions, mount restrictions, and
worker binding lookup. Static catalog enums and GraphQL documents are extracted directly
from pinned source by reproducible tools; SDK API/worker operations execute natively.

Regenerate with the upstream SDK installed in an isolated environment:

```sh
python tools/inventory.py ../runpod-python
python tools/control_fixtures.py
python tools/apps_api_fixtures.py
python tools/placement_fixtures.py
python tools/invocation_fixtures.py
python tools/monitor_fixtures.py
python tools/task_fixtures.py
python tools/deploy_fixtures.py
python tools/ssrf_fixtures.py
python tools/download_fixtures.py  # Windows capture, including simulated POSIX paths
python tools/upload_fixtures.py
python tools/scaling_fixtures.py
python tools/health_fixtures.py
python tools/package_fixtures.py
python tools/build_fixtures.py
python tools/dev_fixtures.py
cargo test --test apps_protocol
python tools/verify_pickle.py
```

## Preserved quirks

- Browser login polls once even with a zero or already-expired deadline. Approval/consumed
  states carrying a key win before deadline checks; consumed without a key fails immediately.
  Browser keys are not trimmed. Email login validates nonblank keys but returns their original
  text. Only verification HTTP 422 prompts for another code; retries share the attempt and
  verifier. All other HTTP/transport failures stop without replay. Creation alone sends a fresh
  UUIDv4 idempotency key. Existing account keys, cookies, and netrc credentials are not imported;
  redirects are never followed and server response bodies are omitted from login errors.
- Development sessions adopt by stable endpoint name, track adopted endpoints before an
  update, and skip standing infrastructure for tasks. Refresh increments generation first,
  deletes removed resources before any saves, saves even unchanged survivors, and ignores
  generation-only changes in event diffs. Failed deletions stay tracked; later children are
  still attempted. Secret preflight runs on start, including tasks, but not on refresh.
  Volume resolution is cached for the session. Replaced application instances keep their old
  bindings; current instances are cleared on stop, matching upstream ownership behavior.
- Dev payloads preserve duplicate reserved environment entries: user values can override
  generation, credentials, app name, and concurrency by appearing later, while resource name
  and API ports are forced. Custom-image package overrides are appended last. Runtime image
  overrides alone do not enable the custom-image bootstrap. GPU locations remain absent
  unless pinned, unlike deployed endpoint payloads.
- Management lists applications alphabetically despite the Python docstring's newest-first
  wording. Deletion flags report completed calls, even when a non-raising GraphQL response
  is false. Application-level deletion aggregates endpoint counts and failures but leaves
  `environment_deleted` false. A failed child deletion retains its parent and does not stop
  attempts on later children; lookup failures propagate immediately.
- Deployment validates resource specs and explicit secret references before any build.
  App/environment creation happens after successful packaging and size validation. The
  exact uploaded manifest is finalized; activation precedes endpoint reconciliation.
  Environment names default on absent/empty overrides, duplicate existing environment
  names keep the last value, and returned resources retain manifest order. Failure after
  a completed remote mutation does not roll it back. Artifact size equal to the limit
  succeeds; even one extra byte fails, including the upstream rounded-size diagnostic.
- Installer progress reports each `Collecting` line and resets its count for each pip run.
  stdout/stderr share one pipe; failures use the last thirty lines and at most 3,000 Unicode
  characters. Empty requirements launch no tools. Missing pip triggers ensurepip; URL/path
  SDK overrides build a wheel first, then choose the last sorted `runpod-*.whl` filename.
  SDK installation uses `--no-deps` before the runtime/user dependency installation.
- Any custom image disables automatic size-based exclusions. Mixed CPU/GPU apps still
  prune image-provided packages, but record a package for CPU cold-start installation only
  when CPU requirements need it (or the user explicitly excluded it). API resources add
  FastAPI/uvicorn dependencies. The SDK overlay retains unrelated stale files, as upstream.
- Requirement collection keeps the first specification for each name; that helper replaces
  underscores but does not normalize dots/repeated separators. Installed distribution
  matching uses full Python package-name canonicalization. Dependency closure evaluates
  extras separately, including an empty-extra context, and conservatively ignores builder
  platform/version conditions. Missing/invalid wheel metadata includes every installed
  distribution in a closure and limits pruning to the explicit exclusions. Shared files
  survive if any retained distribution owns them. User exclusions keep their dependency
  trees unless those dependencies belong exclusively to image-provided packages.
- Packaging prunes ignored directories before looking for nested rules. Root `.runpodignore`
  overrides nested `.gitignore`; protected source paths cannot be unignored. Dependencies
  are copied separately under `env/`, retaining their certificate/data files regardless of
  source ignore rules. Both trees omit symlinks and the output archive itself. Only the
  supplied root manifest is inserted; a nested source manifest remains an ordinary file.
- Task jobs clean up on every exit from wait, including remote failure and cancellation.
  A successful result (including null) is cached. Dropping an unused job alone leaves its
  pod running; explicit cancel terminates it. An execution timeout of None is unlimited.
- Task submit/result requests retry only proxy 404 propagation errors, for six attempts.
  Finite timeouts must be acknowledged by the runner before execution continues.
- Task env values can override the reserved token and port keys. Nonempty package
  environment overrides win over user env values only for custom images.
- Pod capacity fallback tries individual datacenters in order, then returns the initial
  capacity error if all fail. Other creation failures are not replayed. Termination
  retries only transient failures and keeps the pod ID when cleanup fails.
- Deployed endpoint names equal resource names for sentinel routing. Reconciliation
  saves provisionable resources in name order and only then deletes removed endpoints
  in their original environment order. Tasks do not provision persistent endpoints.
  A failed save prevents deletion but does not roll back earlier successful saves.
- Deployed worker env forces the selected environment after user env values, while
  API credentials and runtime package/channel overrides fill only missing keys. Custom
  images append their runtime-kind entry even when the user already supplied that key.
  Mixed CPU3/CPU5 endpoints use CPU5 locations; task placement instead uses their union.

- `Endpoint.run` wraps requests whose `input` field is absent or false-like, including
  empty objects. `AsyncioEndpoint.run` always wraps its argument.
- CANCELLED is terminal for status polling but absent from the Python stream terminal
  list. Streaming therefore retains that behavior; dropping the Rust stream cancels it.
- The synchronous-compatible job output uses zero timeout for a single immediate fetch.
  AsyncioJob instead times out immediately at zero unless a non-null output is cached.
- Null cached output causes another fetch; false/zero/empty-string outputs are cached.
- CPU instance memory is validated syntactically but not sent to REST v2.
- Unsupported legacy pod/resume parameters remain errors rather than being ignored.
- Template command `None` inherits, while an empty string explicitly clears it.
- Placement ranks the worst resource's capacity first, then aggregate capacity; ties use
  the first sorted datacenter. Pod/serverless and GPU counts have independent stock caches.
- Stock queries are advisory and failures score zero. Like Python, a canceled stock fetch
  leaves its configurations marked fetched for that provisioning run.
- Existing volumes keep their datacenter regardless of new-volume capability catalogs.
  Resolving a cached reference does not reconsider changed placement options.
- Apps queue submission is not replayed after timeouts, disconnects, or server failures.
  Safe reads retry transient failures; job propagation routes allow seven attempts,
  while cancel/retry mutations repeat only explicit 404 propagation responses.
- Apps streaming yields all chunks on its first terminal page, including CANCELLED.
  This differs from the older endpoint runner's terminal list described above.
- Apps status observers receive admission and completion, and result failures still
  cache terminal job state. Polling retains the upstream check-before-sleep behavior,
  so an in-flight status request can finish beyond the wait deadline.
- Apps HTTP timeout zero disables the transport deadline. A zero polling/streaming
  deadline still prevents another poll, while already-terminal output is returned.
- Live HTTP source sync uses a SHA-256 hash, coalesces concurrent callers, and records
  success only after upload completes. A canceled or failed upload remains retryable.
  Changing only dependencies without changing source does not invalidate that hash.
- Log streams retain event IDs across frames, deliver a cursor only with valid JSON,
  and ignore incomplete final events. Idle timeout begins after response headers;
  the independent socket-read timeout also applies while awaiting those headers.
- Worker logs deduplicate by timestamp/text across reconnects. Timestamp comparisons
  truncate nanoseconds to microseconds and exclude lines equal to the start time.
  Routine SDK frames are hidden, but even a hidden nonempty raw line suppresses the
  final snapshot fallback. A tail length of zero preserves Python's full-list slice.
- Metrics suppress the first steady ready/running pool, report subsequent changes,
  and stop reporting pool changes once a job has a worker. Worker snapshot recovery
  follows assignment order. Failed requests emit failure before recovering logs;
  successful requests emit completion after recovery.

## Deliberate language/runtime differences

- Configuration is owned per client. Rust does not mutate module globals or execute
  fitness checks at import time. No Python interpreter is embedded or launched.
- API results remain JSON values to preserve unknown server fields. Options use typed
  structs. Timeouts use Duration; sizes/counts use bounded unsigned integers.
- Job wait deadlines use monotonic time including HTTP time, instead of Python's
  decrementing integer counter. Network errors are returned through Result.
- GraphQL reads the response envelope even on non-401 HTTP failures, as Python does.
  Malformed responses return errors rather than Python KeyError/IndexError.
- Redirects are disabled, and resource mutations are never implicitly replayed after
  ambiguous failures. Pagination and selected Apps GraphQL reads retry transport errors.
  Presigned artifact PUTs retry up to four times and reopen the file for every attempt;
  like upstream, HTTP client errors as well as server errors trigger upload retries.
- Environment scalar coercion preserves Python True/False/None. Complex nested values
  currently use JSON formatting rather than Python repr; this remains tracked work.
- Blocking clients own a Tokio runtime and must be constructed/used outside async tasks.
- Worker state and startup registries are owned values instead of Python singletons.
  Async handlers and heartbeats share the caller's executor; no multiprocessing mirror
  or Python GIL is needed. Synchronous CPU work must be scheduled by the caller.
- Worker handler failures carry Rust error type/backtrace information. Rust panics are
  not converted into Python exceptions. Direct output delivery failures propagate to callers;
  the worker dispatcher logs individual delivery failures and continues sibling jobs.
- Dynamic scaling, cancellation, and Ctrl-C/Unix SIGTERM shutdown are implemented. Full
  process-global stdout interception, debugger integration, and CLI arguments remain
  pending. Native HTTP hosting and volume-cache helpers are described below.
  The `start` convenience function uses `test_input.json` locally.
- Credential profiles require a string API key. Validator schemas use Rust types and
  closures, so malformed Python schema objects are outside the Rust input domain.
- Apps mount definitions use an ordered Vec of Mount values; bindings are owned snapshots.
  Exact GPU device exclusions accept an explicit live catalog; None uses the pinned static
  fallback. GpuCatalog caches success and delays retries for sixty seconds after failure.
  Cache ownership follows client clones instead of a process-global credential map;
  independently constructed clients fetch independently. Cancellation does not cache failure.
- Apps use Rust enum/FromStr validation; some malformed-input error text differs from
  Python repr-based messages. The pinned 24 resource fixtures preserve exact messages.
- Mount-binding JSON uses compact encoding instead of Python's spaced encoding; decoded
  values and ordering are preserved. Event callbacks return Result; returned errors are
  logged and ignored, while Rust panics retain their normal behavior.
- Queue/API stubs use `ResourceRef::Name` or `::Id`, making the exactly-one reference
  requirement structural. Environment defaults resolve once at construction.
- Python's Invoker/StreamInvoker wrappers become native async methods and explicit
  blocking clients/iterators. Dropping an async future or stream cancels its local work.
- Native serialization reads Python protocol-5 fixtures and writes protocol-3 pickle
  values that Python/cloudpickle can decode. Wire bytes differ while decoded values match.
  Serde types and JSON represent native data; executable Python classes, closures,
  imports, and object identity are not reconstructed. Raw serialized response strings
  remain available in FunctionResponse. No Python runtime bridge is used.
- Rust has no runtime function signature/source reflection. FunctionDefinition supplies
  parameter names, and FunctionSource supplies complete module text or a file to re-read.
  Source passed to an existing Python worker must be source that worker understands;
  this does not yet provide native Rust function deployment. Native queue/task registration
  and local execution are available; native artifact/runtime integration remains pending.
  JSON argument types enforce the queue boundary.
- FunctionResponse fields enforce their declared types; malformed non-boolean success
  or non-string error/result fields return decoding errors. Load-balancer endpoint IDs
  must contain only ASCII letters, digits, hyphens, or underscores to keep routing within
  the configured service host. Sensitive job options and attached source/environment
  are omitted from client Debug output.
- Log clients own their HAPI configuration rather than a module global. Streams use a
  separate authenticated connection pool with a 60-second socket-read timeout, a
  one-hour total timeout, and optional idle completion. Snapshot timeouts remain separate.
  Log sources and SSE payloads retain unknown JSON fields; malformed source arrays return errors.
- Monitor tasks are owned and aborted on drop. Explicit async stop performs final snapshot
  recovery; dropping a canceled invocation stops background I/O without awaiting snapshot
  recovery. Callbacks can run on background Tokio tasks and are never called under state locks.
- Timestamp helpers accept RFC3339 or extended ISO timestamps without a timezone. Python's
  additional fromisoformat variants (such as week-date forms) are outside that native domain.
- Task runtime images use an explicit `RuntimeImages::python_version`, default 3.12.
  Rust does not inspect or start a local Python interpreter. Custom-image launchers run
  only inside the remote container. Local closures/classes are not serialized as executable
  Python objects; explicit source and the existing built-in-value pickle codec are used.
- A task creation runs in an owned Tokio task so cancellation cannot lose the returned ID.
  Scoped cancellation cleanup runs asynchronously after the dropped caller has returned;
  unlike Python's cancellation handling, Rust Drop cannot wait for it. Keep the runtime alive
  until cleanup finishes. Explicit cleanup waits thirty seconds, then leaves its owned task
  active. Errors are logged, and a prior execution failure takes precedence over cleanup.
- TaskExecution is single-use and rejects a second start. Task timeouts use nonnegative
  Duration values; raw submit payloads still reject booleans, strings, and negative numbers.
  Cleanup state uses watch channels and serialized termination, with no process-global task set.
- Deployment helpers take an owned `AppDefinition` snapshot with explicit `SymbolReference`
  module/qualified-name metadata. `App::definition` also supplies such a snapshot; source
  discovery remains pending. `DeploymentConfig::from_client` supplies worker credentials from the owned
  client instead of Python globals; its Debug output hides credentials and override values.
- `AppRegistry` is caller-owned; creating an App does not alter a global registry. Resources
  retain registration order and reject duplicates before changing shared state. `resources`
  returns a vector snapshot, while handles share the owning app and registered hook metadata.
  Resource identities are immutable after registration; mutable Python-spec tests therefore
  map to validation at the Rust registration boundary, not externally mutable invalid state.
- Function bodies are explicit `NativeFunction` values, accepting positional/named JSON
  arguments. Rust closures provide their own argument defaults and validation. Local and
  remote calls are async; `local` collects a generator into an array and `local_stream` exposes
  incremental output. Synchronous bodies run in Tokio's blocking pool. A panic there retains
  its JoinError, and already-running synchronous work finishes after caller cancellation.
  Async/streaming-body panics retain normal Rust panic semantics. No Python interpreter is
  involved in native function execution. Blocking registered handles own a runtime facade.
- The invocation context is read on each call unless AppConfig supplies a fixed snapshot.
  Workers take precedence over dev sessions; dev app/resource identity overrides deployed
  resource identity. Own-worker calls run locally, sibling calls use sentinel routing or a
  cached named development endpoint. Tasks resolve to dedicated pods before dev routing.
  Discovery forbids remote/spawn/stream calls, including own-worker shortcuts; local calls
  and queue job reconnection remain available. Discovery's diagnostic uses native terminology.
- Queue options accumulate on cloned handles without mutating the original; task options and
  task reconnection are rejected. Hooks are shared registrations rather than copied decorator
  attributes, never run by local calls, and execute only through explicit initialization.
  Hook replacement drops user captures after releasing the registry lock. Source/symbol
  metadata and typed secret references are explicit. Schedule intent remains undeployable
  until backend support exists. `FunctionJob` owns its concrete queue/task job and exposes
  borrowed access plus common wait/cancel operations; dropping it preserves upstream lifecycle.
- Native API declarations use an ordered map of handler names instead of Python class
  inspection. RouteSpec carries method/path/name metadata; NativeFunction closures own method
  state. Names shared by queue, task, and API resources are rejected. Resource snapshots now
  contain `ResourceHandle::{Function, Api}`. API routes retain handler-name order, and replacing
  a named method replaces its route. Multiple differently named init hooks are rejected.
  Explicit setters take the place of marker attributes; no runtime reflection is introduced.
- `ApiHandle` HTTP methods always make client calls, including inside the resource's own
  worker. Explicit live source is attached to the shared target synchronization state;
  failed synchronization prevents the HTTP call and can be retried. Native `initialize` and
  `call_handler` expose registered JSON bodies directly; `serve` hosts JSON and raw HTTP
  methods, and `call_http_handler` preserves raw responses. ASGI factories are explicit Python-worker metadata and cannot be
  initialized/executed as native Rust closures. Factory lifespans own their initialization.
- `blocking::apps::App` owns a multi-thread Tokio runtime and wraps existing native App state.
  Function/API handles, submitted jobs, reconnectable queue jobs, and output iterators retain
  that runtime independently of the originating app. `as_async` exposes the native handle
  for async calls and metadata configuration. Per-job options clone their binding, and no
  hidden process-global loop thread is created. Native sync bodies can recursively call
  blocking handles from the blocking pool; async callers use native async handle methods.
- Construct blocking apps outside Tokio and call their blocking methods from synchronous
  code, never while polling an async task. This deliberately replaces Python's ability to
  block an already-running asyncio loop with Rust's explicit async/blocking API separation.
  The constructor returns a structured error for nested-runtime creation. Python's generic
  global bridge and KeyboardInterrupt cleanup machinery remain pending the native lifecycle
  audit; the registered facade does not claim to install OS signal handling.
- `package_project` returns an `Artifact` that owns automatic temporary storage. Dropping
  it removes that storage; `persist` explicitly keeps it. A caller-specified output is
  always retained. Archive member paths/order and file bytes match the Python fixtures;
  gzip headers, tar extensions, manifest metadata and JSON Unicode escaping need not be
  byte-identical. Source files retain native file metadata (including executable mode on
  Unix). Ignore files must be UTF-8 and use the `ignore` crate's Git syntax; exotic patterns
  and platform-specific filename restrictions follow native libraries. Packaging is
  synchronous filesystem work; async orchestration must run it on a blocking executor.
- Dependency analysis uses native `pep508_rs` and `csv` parsers. Unusual malformed specifiers,
  CSV, and contradictory platform expressions follow those libraries rather than emulating
  Python parsing internals. Pruning accepts relative wheel RECORD paths (including pip's
  `../../bin/...` staging convention), rejects absolute/drive paths, and verifies resolved
  containment before deleting. Unlike Python, even absolute paths pointing inside the
  environment are ignored. Symlinks resolving outside the environment remain untouched.
- `BuildTools` explicitly owns the external interpreter command; build/installer functions
  are synchronous and require a blocking executor in async flows. Python/pip are external
  build tools, not an SDK runtime bridge. Child stdin is closed, console windows are hidden
  on Windows, and a failed read or progress callback kills/reaps the installer. The default
  command is `python` on Windows and `python3` elsewhere; callers can select a venv binary.
- `BuildOptions` captures the package override and takes an optional SDK package directory.
  Rust has no installed Python package tree to reflect on. With no explicit source/override,
  `build_environment` builds and installs the exact pinned upstream GitHub source revision.
  An explicit source is overlaid with native file copying (bytecode excluded); overlapping
  source/destination trees are rejected. BuildResult owns automatic scratch until dropped
  or persisted. Caller-specified scratch is retained even after a failure. Source file
  timestamp preservation follows native copying rather than Python copy2 metadata behavior.
- ResourceDefinition retains `referenced_secrets` separately from rendered environment
  values. Use `set_environment` for typed environment values, or supply explicit names
  when assembling metadata yourself. Ordinary strings that resemble secret templates
  are not inferred to be secret objects. `module_path_for` accepts an explicit source
  path and uses native canonicalization; inaccessible/nonexistent paths use the fallback.
- `deploy_app` uses an owned blocking task for artifact creation. The Tokio executor stays
  responsive; cancellation stops subsequent cloud operations, but an already running
  synchronous build is allowed to finish, after which its temporary output is dropped.
  Build/package callbacks may run on that blocking thread. EventSink errors are suppressed,
  including progress/endpoint-ready observers that Python invokes directly. Use BuildTools
  directly when a fallible installer progress callback is required. Worker credentials come
  from the supplied Client, and the captured build package override also reaches worker env.

## Verification log

- Windows x86_64, Rust 1.99.1: **42 integration tests and one doctest passed**, including
  47 control-plane and 19 worker differential cases. Cargo build, rustfmt, Clippy with
  warnings denied, and documentation generation passed for the first two milestones.
- Python 3.13.15: API/endpoint/error subset reported **157 passed**. The command's
  exit status was nonzero because upstream enforces 90% whole-repository coverage even
  for this subset (5% overall). The broader offline suite is being evaluated separately.
- Live Runpod deployment and GPU execution have not been performed.
- Broader offline Python baseline, with a fresh workspace-local pytest temporary directory:
  **2,132 passed, 37 failed, 10 skipped, 6 subtests passed**. The initial run's 322 fixture
  errors were caused by Windows permissions on the default pytest temporary directory.
  Remaining upstream failures are retained in local test logs for classification.
- Apps definition milestone: **49 integration tests and one doctest passed** (90 captured
  Python cases across control-plane, worker-output, and resource-manifest fixtures).
  Clippy with warnings denied and documentation generation also passed.
- Apps control-plane milestone: **61 integration tests and one doctest passed**, including
  161 Python differential cases. Build, formatting, Clippy with warnings denied, and docs passed.
  Actual truncated TCP responses verify read retry exhaustion and non-replayed mutations.
- Placement and storage milestone: **77 integration tests and one doctest passed**, including
  180 captured Python cases. Build, formatting, Clippy with warnings denied, and docs passed.
  The matching upstream Apps API/placement/catalog/secret/registry subset passed **145 tests**.
  [Rust review notes](reviews/apps-placement.md) record the fixed deserialization invariant,
  retained error causes, and remaining advisory review limitations.
- Apps invocation milestone: **106 integration tests and one doctest passed**, including
  **231 captured Python cases**. The new upstream target/retry/job subset passed 69 tests,
  and all seven upstream stub tests passed. Python/cloudpickle 3.1.2 decoded all 14
  Rust-generated pickle values, including Unicode. Actual disconnects and delayed HTTP
  responses test non-replayed submissions; tests also cover retry exhaustion, source
  changes/cancellation, failed-result caching, and blocking runtime ownership.
  Build, formatting, Clippy with warnings denied, and documentation generation passed.
  [Invocation review](reviews/apps-invocation.md) records fixes and remaining limitations.
- Logs and monitoring milestone: **121 integration tests and one doctest passed** on
  Windows with both stable Rust 1.99.1 and the declared **Rust 1.88.0 MSRV**. There are
  **264 captured Python cases**. All **38 upstream log/monitor tests** passed. Added tests
  cover fragmented UTF-8, SSE cursor/frame boundaries, idle/socket deadlines, reconnect
  deduplication, snapshot ordering/failures, and cancellation closing active connections.
  Build, formatting, Clippy with warnings denied, and documentation generation passed.
  [Monitoring review](reviews/apps-monitor.md) describes lifecycle and timeout fixes.
- The previous invocation commit 4cdb636 passed all three remote stable jobs on Linux,
  Windows, and macOS: [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/40943317560).
  The new log/monitor commit's remote checks are not covered by that earlier result.
- The first monitoring CI run exposed an ambiguous order between test mocks. Commit
  8f61681 makes their priorities explicit; all three stable platforms and the Linux MSRV
  job then passed: [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/40948839568).
- Ephemeral task milestone: **138 integration tests and one doctest passed** on Windows
  with stable Rust 1.99.1 and MSRV 1.88.0, with **339 captured Python cases** overall.
  The task/image/launcher Python subset reported **98 passed, five failed** on Windows:
  four tests require POSIX `sh`, and one formats an endpoint mount as a Windows Path.
  The Rust sibling-placement equivalent passes. Unix CI additionally executes the real
  launcher against a shell stub, without installing packages or invoking Python.
  Build, formatting, Clippy with warnings denied, and documentation generation passed.
  [Task review](reviews/apps-tasks.md) records lifecycle guarantees and native adaptations.
- Commit 5c3a42b passed all three stable platforms and the Linux MSRV job, including
  Unix shell launcher execution: [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/40951799258).
- Deployment contract milestone: **143 integration tests and one doctest passed** on
  Windows stable Rust. All five new deployment tests also passed on Rust 1.88.0.
  There are **382 captured Python cases** overall; the six matching upstream manifest
  and endpoint-configuration tests passed. The HTTP tests cover save/delete ordering,
  existing IDs, model and registry references, shared volume caching/placement, live GPU
  membership, task-only apps, and failure boundaries. Build, formatting, Clippy with
  warnings denied, and documentation generation passed. See [deployment review](reviews/apps-deploy.md).

- Commit ca6617f passed all three stable platforms and the Linux MSRV job:
  [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/40954045767).
- Packaging adds sixteen Python archive fixtures and twenty Windows tests, including
  binary/long-name/hard-link handling, temporary storage ownership, IO failures, and
  exclusion logs without file contents. All twenty pass on stable and MSRV 1.88.0.
  Unix additionally tests file/directory/broken symlinks, symlinked ignore files, and
  executable mode preservation. The upstream packaging subset reported six passed and
  one failed on Windows because creating symlinks requires privileges this session lacks;
  its whole-project coverage threshold also fails for this small subset. See
  [packaging review](reviews/apps-package.md).
- The full packaging milestone passed **163 integration tests and one doctest** on Windows
  stable Rust, with **398 captured Python cases** overall. Build, formatting, Clippy with
  warnings denied, and documentation generation passed. The separate pedantic review
  reported no warnings in the new packaging files; older advisory warnings remain.
- Commit b84c575 passed all stable platform jobs and Linux MSRV, including the Unix
  symlink/mode packaging tests: [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41025108861).
- Dependency analysis adds **73 Python cases** for requirement names, selection, extras,
  transitive dependencies, and exact remaining file/directory trees after pruning.
  All **34 matching upstream Python tests** pass. Additional Rust regressions check
  absolute/traversal record paths and quoted CSV filenames; Unix tests cover symlink
  containment. Installer subprocesses, SDK overlay, and build orchestration remain pending.
- Dependency-analysis milestone: **192 integration tests and one doctest** pass on Windows
  stable Rust. All **29 new tests** also pass on Rust 1.88. There are **471 captured Python
  cases** overall. Build, formatting, Clippy with warnings denied, and docs pass; the
  separate pedantic review has no warnings in the new files. See
  [dependency-analysis review](reviews/apps-build-analysis.md).
- Commit deaf5ae passed all stable platform jobs and Linux MSRV, including Unix pruning
  containment: [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41026474247).
- All **66 upstream build tests** pass. New Rust process tests use shell/PowerShell stand-ins
  to exercise real pipe handling, large stderr, bootstrap failures, wheel selection, exact
  target flags, fallible progress callbacks, SDK overrides, and scratch ownership. No pip
  install or package download is performed in those tests. See
  [installer review](reviews/apps-installer.md).
- Installer/build milestone: **212 integration tests and one doctest** pass on Windows
  stable Rust. All **20 new installer/build tests** pass on Rust 1.88. Build, formatting,
  Clippy with warnings denied, and documentation generation pass. Pedantic review fixed
  documentation formatting; its function-length and test-helper allocation suggestions
  remain advisory. Real package-index/GitHub installation has not been exercised by tests.
- Commit d4e3e0f passed stable Linux/Windows/macOS and Linux MSRV, including Unix installer
  process fixtures: [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41028795901).
- All four upstream deployment pipeline tests pass. New Rust tests verify actual archive
  bytes at a local upload service, exact finalized manifests, request ordering, environment
  reuse/propagation, explicit-secret preflight, failure boundaries, standalone artifacts,
  source module paths, executor responsiveness, and cancellation before cloud mutations.
  A unit test verifies the size-limit boundary without creating a 1.5 GiB artifact.
- Deployment pipeline milestone: **221 integration tests, one unit test, and one doctest**
  pass on Windows stable Rust. The nine new integration tests and size-limit unit test
  also pass on Rust 1.88. Build, formatting, Clippy with warnings denied, and docs pass.
  See [pipeline review](reviews/apps-deploy-pipeline.md) for the cancellation contract and
  remaining advisory review notes. No live Runpod resource was created or changed.
- The deployment pipeline passed all four CI jobs (Linux, Windows, macOS, and Linux MSRV):
  [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41033921728).
- Native App tests cover owned registration, rejected duplicates/configuration, local bodies
  and hooks, context precedence, discovery guards, sentinel payload/options, cached dev lookup,
  task job ownership, queue reconnection, lazy streaming, scheduler intent, secret metadata,
  Send/Sync, redacted Debug, and synchronous-body executor responsiveness/panic causes.
  Upstream App/context/dispatch subset: 65 passed and one failed on Windows because Python
  converts the endpoint mount path to a backslash form. The Rust mount contract remains POSIX.
- Native function milestone: **239 integration tests, one unit test, and two doctests**
  pass on Windows stable Rust. All eighteen new integration tests also pass on Rust 1.88.
  Build, formatting, Clippy with warnings denied, and documentation generation pass.
  See [native function review](reviews/apps-native-functions.md) for adaptations and limits.
- Native function registration passed all four CI jobs:
  [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41036209242).
- Nine API-handle integration tests cover route validation/order/replacement, mixed-resource
  registration, explicit native initialization, manifest/secret metadata, discovery guards,
  all HTTP verbs, own-worker routing, shared source synchronization, and failed-sync retry.
  All fifteen upstream reserved-path marker cases pass on Windows. A unit regression verifies
  hooks, events, and targets release application locks before dropping user-owned captures.
- API-handle milestone: **248 integration tests, two unit tests, and two doctests** pass on
  Windows stable Rust. All 27 function/API integration tests and both unit tests pass on
  Rust 1.88. Build, formatting, Clippy with warnings denied, and docs pass. See the
  [API-handle review](reviews/apps-api-handles.md) for explicit native/runtime boundaries.
- Native API handles passed all four CI jobs:
  [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41038108570).
- Eight blocking-handle tests cover local/remote execution, explicit initialization, all HTTP
  verbs, task/queue registration, option binding, retained-runtime job and iterator behavior,
  queue reconnection/cancellation, lazy remote streaming, nested-runtime rejection, and
  worker-local recursion from synchronous and concurrent asynchronous callers.
- Blocking registered-handle milestone: **256 integration tests, two unit tests, and two
  doctests** pass on Windows stable Rust. All eight new tests also pass on Rust 1.88.
  Build, formatting, Clippy with warnings denied, and docs pass. See the
  [blocking-handle review](reviews/apps-blocking-handles.md) for runtime ownership and limits.

- Blocking registered handles passed all four CI jobs:
  [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41040245510).
- All nine upstream management tests pass. Fifteen Rust tests check ordered real HTTP
  requests, missing records, retained environments, partial failures, empty children,
  event suppression, cancellation, and upstream deletion-result quirks. See the
  [management review](reviews/apps-manage.md).
- Management milestone: **271 integration tests, two unit tests, and two doctests** pass
  on Windows stable Rust. All fifteen new tests pass on Rust 1.88. Build, formatting,
  Clippy with warnings denied, and docs pass. Pedantic review prompted documentation fixes.
- Management passed all four CI jobs:
  [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41042431402).
- All 29 upstream development-session tests pass. The Rust tests replay 47 captured Python
  payload/diff cases (518 captured cases overall), plus local HTTP tests for adoption, refresh
  ordering, real target dispatch, secret preflight, registries/models/volume caching, task
  events after refresh, retained failed IDs, observer errors, and cleanup error precedence.
  Cancellation regressions interrupt both creation and cleanup, and verify that explicit
  shutdown waits for creation and deletion before returning.
- Development-session milestone: **293 integration tests, two unit tests, and two doctests**
  pass on Windows stable Rust. All 22 new integration tests and both unit tests pass on
  Rust 1.88. Build, formatting, Clippy with warnings denied, and docs pass. Pedantic review
  found only advisory test-style suggestions after the source fixes.

Development sessions use owned clients, runtime configuration, and app-local routing flags;
they never change process environment variables. `DevConfig::images` selects the interpreter
for explicit Python source used by existing live workers. It does not ship native Rust
closures. `DevSession::run` replaces context-manager startup/body/error cleanup. Prefer
`run_until` for shutdown signals: it settles a shielded creation and awaits teardown. When a
lifecycle future itself is dropped, keep its session and Tokio runtime alive and call `stop`.
There is no async network work in a session destructor. See the
[development-session review](reviews/apps-dev.md).

- Development sessions passed all four CI jobs:
  [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41044871415).
- All 39 matching upstream browser/email login cases pass. Twenty Rust tests cover the
  anonymous verifier/challenge exchange, unique idempotency keys, exact paths and headers,
  rejection retries, terminal errors, malformed bodies, input cancellation, transport timeout,
  media types, callback failure, and approval/deadline precedence. See the
  [authentication review](reviews/apps-auth.md).
- Authentication milestone: **313 integration tests, two unit tests, and two doctests** pass
  on Windows stable Rust. All twenty new tests pass on Rust 1.88. Build, formatting, Clippy
  with warnings denied, and docs pass. Review notes explain the native date/clock adaptation.

Authentication uses explicit connection/options and an asynchronous code-reader callback.
Browser expiry accepts RFC 3339 timestamps with explicit offsets; other formats accepted by
Python's broad `datetime.fromisoformat` parser are ignored. Elapsed timeout checks use Tokio's
monotonic clock. Native string validation rejects malformed non-string credential fields.
Email responses use UTF-8 JSON and retain the SDK's native Rust user-agent identity.

Local entrypoints use owned registries. Registration is lazy, selects the last declaration,
and returns a native function sharing its captured state. Clearing only changes that registry;
retained callables still work. Async execution delegates to the native function interface:
synchronous bodies use the blocking pool and streams collect into JSON arrays. The blocking
runner owns a runtime for that call and rejects use from inside Tokio. Four focused tests
cover these contracts; they do not claim coverage of the pending discovery/CLI integration.
See the [entrypoint review](reviews/apps-entrypoint.md).
All four new entrypoint tests pass on Windows stable and Rust 1.88. Formatting, Clippy with
warnings denied, and docs pass; pedantic review reports no new source warnings. The preceding
complete suite passed 317 tests; these four tests extend it without changing execution logic.

Authentication and entrypoints passed all four CI jobs: [authentication run](https://github.com/kodxana/runpod-rust/actions/runs/41047207031)
and [entrypoint run](https://github.com/kodxana/runpod-rust/actions/runs/41047730289).

### Download security transport

`utils::ssrf` ports all twenty source symbols through native async interfaces. Each redirect
resolves and validates all DNS answers before creating a single-use pinned connection; failed
connections advance through the validated list. HTTP responses and post-connect timeouts do
not advance. An observed connection-completion flag distinguishes the overlapping reqwest
connect/header timers, including Windows connect timeouts. TLS SNI, Host, certificate identity,
and chain verification remain bound to the requested hostname. An explicit Host header cannot
override it. Proxies are rejected unless private downloads are enabled; NO_PROXY exclusions,
lowercase environment precedence, and CGI HTTP_PROXY suppression are supported.

The 498 captured address/configuration/authority cases bring the total captured Python cases
to **1,016**. Twenty-one Rust tests use local HTTP/TLS servers, including refused connections,
mixed DNS results, integer/octal/mapped-IP URL spellings, repeated resolution on redirects,
gzip expansion, missing Content-Length, exact limits, proxy routing, cancellation, socket
cleanup, and TLS trust/hostname rejection. All 26 upstream SSRF tests pass. The metadata-hop
test maps to separate native hop-revalidation and literal-metadata-rejection checks, avoiding
a real public-network request. The public test certificates/keys are fixtures only.

The complete Windows stable suite passes **338 integration tests, two unit tests, and two
doctests**. The new tests pass on Rust 1.88. See the [Rust review](reviews/ssrf.md).

Native adaptations are explicit:

- `Error::Ssrf` is distinct from `Error::Http`. Error text is native and proxy diagnostics
  omit credentials/URLs. Owned responses and streams replace monkey-patched close methods;
  cleanup cannot mask a transport failure. Every public body reader enforces the cap, and
  an optional stream cap may tighten it. Dropping a future, response, or stream releases
  transport state. No asynchronous network operation runs in Drop.
- Configuration snapshots replace repeated reads of mutable process environment. Positive
  ASCII integer limits (including separators) must fit u64; overflow/invalid values fall
  back to 5 GiB. Non-Unicode environment entries are ignored. Only explicit/environment
  proxies are used; system proxy registries and PAC files are not imported. Private opt-in
  still applies scheme and size limits but intentionally gives up proxy DNS pinning.
- URL parsing uses reqwest/WHATWG normalization, rejects ambiguous backslashes/control
  characters and scoped IPv6 URLs, and checks the actual normalized literal IP before
  dialing. It may canonicalize Unicode hosts, numeric IP spellings, and redirected default
  ports. Direct Host construction retains explicit default ports and IPv6 brackets.
- The IP classifier mirrors pinned CPython 3.13, including its inherited `fec0::/10`
  site-local IPv6 acceptance. It is not a live IANA registry or an additional network
  allowlist. Applications that route that deprecated range internally need additional policy.
- Responses are always streamed until consumed. Reqwest transparently decompresses gzip,
  deflate, and Brotli; decoded bytes are capped. It removes Content-Encoding/Content-Length
  when decoding, so compressed responses use the consumed-body check instead of the raw
  compressed Content-Length precheck. Chunk boundaries are native and at most the requested
  size. HTTP parser failures (including malformed length/framing) remain transport errors.
- Reqwest's initial read deadline starts before connecting; it is more conservative than
  Python's separate connect-then-header deadlines. Body reads have an inactivity timeout.
  Automatic request retries, redirects, cookies, Referer, netrc, and account credentials are
  disabled/not imported. Explicit caller headers are retained on each redirect as upstream;
  callers must not supply secrets for untrusted redirected URLs.

This file will be updated as each remaining component and test is ported.

### File downloads and ZIP extraction

The SSRF transport passed all four CI jobs: [GitHub Actions run](https://github.com/kodxana/runpod-rust/actions/runs/41051344163).
`utils::download` now provides all five upstream download functions. `FileDownloader` owns
an absolute project root, transport options, optional resolver, concurrency, and timeouts.
Convenience functions capture the current directory. Native async operations replace the
Python thread pool without changing cwd or environment. Batch results retain input order;
completed slots admit queued items even while an earlier item waits. All items finish before
the first policy/filesystem error in input order is returned. HTTP failures get three attempts
with full-jitter exponential delays, then return `None`. Policy and local I/O errors are never
retried. Missing inputs return `None`, and empty batches still create their output directory.

Single downloads reject HTTP errors before writing, return `DownloadedFile` with the original
dictionary field names under Serde, and extract lowercase `.zip` inputs. Original URL paths
determine fallback names/extensions; query strings and fragments cannot become extensions.
Content-Disposition keeps Python's simple parser, including quoted-semicolon and `filename*`
quirks. Files get OS-random UUIDv4 names. Retry attempts retain the same UUID, including partial
files under an earlier response's different extension. Failed/canceled writes may leave partial
files, matching upstream. Buffered disk writes finish before a returned body/policy error.

ZIP decoding is native through `zip` 9.1.3, with stored, DEFLATE, BZIP2, and LZMA members and
ZIP64 covered. Extraction removes absolute roots and `.`/`..` components instead of resolving
them, applies Windows drive/UNC and illegal-name rules, and writes symlink entries as ordinary
files. Duplicate-name records use their last central-directory offset to retain final overwrite
precedence when sanitized paths collide. CRC errors propagate. Blocking extraction observes
cancellation at entry/read boundaries; cancellation does not roll back completed files.

The 66 new Python captures include separate Windows/POSIX metadata and ZIP-tree expectations,
bringing the total to **1,082 captured cases**. The archive has fixed timestamps and includes
duplicate/aliased names, Unicode, drive and UNC paths, ZIP64, all four compression methods, and
symlink metadata. The generator must run on Windows because native `ntpath.abspath` removes
trailing dots; POSIX behavior is captured using `posixpath` and mocked filesystem operations.
All nine upstream download tests pass. Fifteen Rust integration tests plus one new unit test
pass on Windows stable and Rust 1.88. See the [download review](reviews/download.md).
The complete stable suite passes **353 integration tests, three unit tests, and two
doctests (358 total)**. The final reusable-buffer adjustment is covered by the targeted
stable/MSRV tests. Formatting, Clippy with warnings denied, documentation, and build checks pass.

Adaptations and inherited limitations:

- The returned paths use native Rust/OS normalization; Windows returned filenames lose a
  trailing dot as upstream. Sizes parse as signed ASCII i128; invalid or overflowing header
  values return zero for chunk sizing. The typed Rust boundary excludes Python's arbitrary
  header value types. Parameter maps are ordered by key instead of Python insertion order.
- Hyper rejects malformed wire Content-Length before it reaches the sizing helper. The two
  upstream tests using fake malformed headers map to parser fixtures, not a claim that native
  HTTP accepts malformed framing. Decompression/timeout differences from SSRF still apply.
- Owned configuration snapshots replace per-call environment reads. The native user-agent
  identifies Rust; caller-provided headers can override it. Exhausted batch errors use tracing
  rather than stdout and omit URL details. Random jitter has the same bounds, not Python's RNG
  sequence. Source request deadlines remain five seconds for batches and thirty for single
  files by default; both are configurable.
- ZIP parsing coalesces duplicate raw names. Final successful overwrite behavior is tested;
  intermediate duplicate writes and partial output after conflicting file/directory entries
  can differ from Python. Parse/decode/filesystem errors use native `Error::Zip`/`Error::Io`.
- As upstream, the download cap does **not** bound expanded ZIP size, and `job_id` is a trusted
  filesystem path that may contain absolute or parent components. The project root is a
  location default, not a filesystem sandbox. Native Windows drive-relative path joining can
  differ from `ntpath`; ordinary IDs, absolute paths, and parent traversal follow OS semantics.
  Extraction assumes its newly created directory is not concurrently modified by another
  process. These inherited policies are explicit rather than silently changed during a port.
- Cancellation stops active HTTP futures and queued work. A submitted disk write may finish;
  an extraction already parsing a ZIP checks cancellation at its next entry/read boundary.
  Callers own completed and partial artifacts and can use existing cleanup utilities.

## File and S3 uploads

`utils::upload` now covers every function in `rp_upload.py`, with native async methods
and free wrappers. The AWS Rust SDK handles SigV4, HTTPS, request serialization, streaming
checksums, and standard retries. Its current minimum is Rust 1.94.1; Cargo metadata and CI
now declare that version. Historical Rust 1.88 verification above refers to earlier commits.

- Explicit credentials use the source dictionary names through `BucketCredentials` Serde
  fields. Otherwise options capture the three BUCKET environment variables. Any missing/empty
  credential selects local fallback; S3/network failures never select fallback. The original
  URL region extraction is preserved, including returning `digitaloceanspaces` for a bare
  `https://nyc3.digitaloceanspaces.com` endpoint. Unknown URL regions use an explicit region,
  AWS_DEFAULT_REGION, then us-east-1. `bucket_upload` deliberately skips URL extraction.
- Local fallback creates only its top-level directory, gives a nonempty source path
  precedence over bytes, accepts empty bytes, and copies contents without copying permissions.
  Same-file and hard-link copies are rejected before truncation. Returned relative strings
  retain duplicate separators. Names/paths remain trusted and may contain parent components.
- File and memory transfers preserve bucket defaults from local-time `%m-%y`, literal prefix
  concatenation, and a seven-day presigned GET. Images retain case-sensitive extensions,
  `image/<extension>` content types, random eight-hex names, and `simulated_uploaded` fallback.
  A borrowed optional result slot replaces the shared Python list/index pair. Concurrent
  `files` preserves order and turns each failure into None; `bucket_upload` is sequential,
  uses full input paths as keys, stops on error, and returns raw unescaped endpoint URLs.
- Multipart starts at exactly 25 KiB. The configured 25 KiB chunks are adjusted like
  s3transfer to at least 5 MiB and at most 5 GiB, aiming for at most 10,000 parts. Parts run
  with bounded concurrency, completion is ordered, and file ranges can be reopened on retry.
  Botocore's three retries mean **four total attempts**. Native standard retry classification
  and jitter are supplied by AWS. Connect/read settings are 60 seconds, as in botocore;
  native read-deadline semantics and streaming framing can differ between HTTP libraries.
- Every boto3 upload ExtraArgs name supported by the characterized botocore 1.43.121 stack
  is forwarded to its applicable operations, including all ten explicit checksum fields.
  Full-object checksums select FULL_OBJECT/algorithm for multipart; metadata is only sent on
  creation; part ETags and returned checksums appear in completion. SSE customer keys are
  UTF-8/base64 encoded and MD5 hashed unless the caller supplied an MD5, matching botocore.
- Failed multipart work awaits an abort and retains its original error if abort also fails.
  Cancellation after an UploadId is known schedules best-effort cleanup on the current Tokio
  runtime. Runtime shutdown or cancellation before the creation response reveals its ID can
  leave an upload requiring bucket lifecycle cleanup. Completed objects are not deleted.

Native adaptations: Cargo resolves the S3 dependency at build time, so runtime boto3 import
failure has no equivalent. Native tracing replaces logger/tqdm output without configuring a
global logger. Owned roots/options replace mutable process globals. JSON ExtraArgs accept
strings/maps/numbers; dates support RFC3339/HTTP-date strings or Unix seconds, rather than
Python datetime objects. Dynamic Python coercions and unbounded integer sizes are not copied.
Native default concurrency uses available parallelism. Presigned URLs and standard headers
use AWS Rust SDK spelling/order (including `x-id=GetObject`), not byte-identical boto3 URLs.
The default client uses path-style custom endpoints and explicit static credentials. It does
not load boto3 profile files; `with_client` accepts a caller-configured AWS client for custom
providers/transports. No Runpod account credential is used for S3.

`tools/upload_fixtures.py` captures **47 Python cases**, bringing the total to **1,129**.
Cases cover region quirks, transfer sizing, buckets/prefixes, image metadata, and a real
botocore request with every ExtraArgs key intercepted before network access. All ten upstream
upload tests pass. The native suite has 18 Windows tests plus a Unix-only permissions test,
covering real local HTTP signing, multipart ordering/thresholds/metadata, body replay,
four-attempt retries, presigning, fallback, same-file detection, abort, and cancellation.
See [reviews/upload.md](reviews/upload.md) for review and verification details.

Verification: the complete Windows stable Rust 1.99.1 suite passed **371 integration tests,
three unit tests, and two doctests (376 total)**. The final explicit 60-second timeout setting
and documentation annotations were then checked by all 18 upload tests on stable and Rust
1.94.1. All 15 download tests also pass on both toolchains. Build, formatting, Clippy with
warnings denied, and documentation checks pass; pedantic review found no remaining correctness
issue. Python successfully decoded all 14 native pickle fixtures. No live S3/Runpod resources
were used. Subsequent CI run [41061119350](https://github.com/kodxana/runpod-rust/actions/runs/41061119350)
passed on Linux, Windows, and macOS stable, plus Linux Rust 1.94.1, including Unix copy permissions.

The previous download CI run [41056779984](https://github.com/kodxana/runpod-rust/actions/runs/41056779984)
failed on a Windows-generated POSIX ZIP fixture. The capture now switches os.sep before
opening the ZIP, preserving literal backslashes. All 19 POSIX fixture files match actual
Linux Python extraction in WSL; Windows download tests pass. Native extraction code and ZIP
bytes were unchanged by that correction.

## Worker scaling and startup coordination

The native worker now applies a `ConcurrencyModifier` once per intake iteration. A changed
capacity waits for existing work to drain, while the reported concurrency changes immediately.
A full worker wakes as soon as all jobs drain or after the one-second partial-refill bound.
Returning zero pauses intake without spinning. Python's occupancy quirk is preserved: queued
jobs count both in the queue and in the progress tracker. Duplicate IDs retain both queued
invocations, although progress and cancellation remain keyed by ID, as in Python.

`WorkerControl` observes concurrency/occupancy, cancels a named tracked job, and wakes shutdown
from another task or thread. Shutdown stops intake and drains held work. Dropping the run future
cancels handlers and clears shared state; a second concurrent run is rejected. Result-delivery
failures do not stop siblings, and cancellation does not request a worker refresh. Empty/error
stop polls retain the source's one-second delay, with five seconds for rate limiting. Intake
yields without a default delay, matching Python, with an optional configured pause.

Startup hooks run alongside intake, while dispatched jobs wait on a startup gate. Held jobs
can be canceled before startup finishes. On failure, intake and stop polls are canceled before
reporting. If no request was ever held, one fresh fallback claim has a ten-second deadline;
the entire reporting phase has a fifteen-second deadline. Claim errors are ignored, while
fallback delivery errors propagate, matching the source's exception precedence. Shutdown
during startup reports `prestart_cancelled` and drops the unfinished hook future.

`run_until_signal` explicitly installs Ctrl-C and Unix SIGTERM listeners; constructing a worker
does not install signal handlers. Native ownership and Tokio futures replace Python queue/task
objects and the multiprocessing shutdown mirror. The library returns typed startup errors;
Fitness and prestart process entrypoints now report and hard-exit, and production startup
installs a native panic hook. Full captured-log/traceback failure payloads remain pending. Strict startup-race, injected
queue, and reflection-specific tests remain pending rather than being counted as covered.

`tools/scaling_fixtures.py` captures **33 Python cases**, bringing the total to **1,162**.
The focused upstream scaler, mirror, and prestart suites pass **72 tests with one skip**.
The ledger adds 22 adapted symbols and 29 mapped upstream tests. Native coverage includes
16 new Windows integration tests and one fixture-replay unit test; a separate Unix subprocess
test exercises real SIGTERM delivery. Subsequent CI run
[41069882750](https://github.com/kodxana/runpod-rust/actions/runs/41069882750) passed on Linux,
Windows, and macOS stable, plus Linux Rust 1.94.1, including the SIGTERM test. See
[reviews/worker-scaling.md](reviews/worker-scaling.md) for review and final validation.
The full Windows stable suite passes **393 tests** (387 integration, four unit, two doctests).
All **24 focused unit/worker tests** pass on Rust 1.94.1. Build, formatting, Clippy with
warnings denied, and documentation checks pass.

## Native health probes

`serverless::health` now supplies the native probe layer used by Python's health modules.
GPU allocation parsing preserves metadata, repeated GPU count lines, case-sensitive headers,
case-insensitive error words, negative error-message slice limits, and the source's decision
to trust successful stdout even when the allocation process exits nonzero. A device allocation
failure is a distinct `HealthError::GpuAllocation` and never falls back. Missing/nonexecutable
binaries and failures before device enumeration permit a bounded `nvidia-smi --list-gpus`
fallback. Combined failures retain the fallback error as their source.

The unchanged 830,408-byte upstream `gpu_test` binary is embedded on Linux x86_64, extracted
to an owned temporary file with mode 0700, and removed after its final owner drops. An existing
`RUNPOD_BINARY_GPU_TEST_PATH` overrides it. Other platforms can supply a native binary or use
the fallback; no unsupported Linux executable is launched there. See [binaries/README.md](binaries/README.md)
for provenance, license, and platform requirements. Its SHA-256 is
`fa26cfe85d7d3c403d592f7707c66894773fd09b2cdfd55f30b433b7afb48494`.

Native OS memory counters replace psutil, with a separate `/proc/meminfo` parser preserving
the source fallback. Root-filesystem available space uses native filesystem statistics.
Thresholds remain strict less-than comparisons, including zero-total and negative-threshold
behavior. TCP readiness uses three attempts within one shared budget, includes connection
teardown, sends no HTTP request, and reports only the host/port. CUDA detection tries bounded
nvcc then nvidia-smi processes and skips unavailable version information. The existing
`utils::is_cuda_available` now uses the same bounded process helper and timeout warning.

Configuration is owned rather than mutable global state. Integer settings use signed 64-bit
values; version pairs use unsigned 64-bit values. Exotic unbounded Python integers, numeric
underscores, and non-ASCII version digits are outside this native input domain. Nonpositive
GPU timeouts become an immediate zero-duration deadline. Rust URL parsing replaces urlsplit;
the captured malformed URLs, zero ports, credentials, and IPv6 cases match. OS-specific error
types/messages are retained through `HealthError`; absence of a native binary on unsupported
platforms is reported as missing. Memory counters may differ between native OS and psutil
implementations. The packaged GPU probe has not been exercised on physical GPU hardware.

The [sysinfo 0.38.4](https://docs.rs/sysinfo/0.38.4/sysinfo/) dependency supports the existing
Rust 1.94.1 minimum; only its system feature is enabled. Native filesystem statistics use
[fs2 0.4.3](https://docs.rs/fs2/0.4.3/fs2/fn.statvfs.html). Neither starts Python.

`tools/health_fixtures.py` captures **43 Python cases**, bringing the total to **1,205**.
The full upstream health suite has **158 passed and two Windows-only test-fixture failures**:
its two successful-binary integration cases launch Unix-style script executables directly.
Native process tests instead compile a small Rust fixture executable and verify success,
fallback, permission errors, timeout/cancellation cleanup, and version detection. Unit tests
control the retry clock and prove timed-out probe futures are dropped. The ledger adds
20 symbols and 41 upstream test mappings; executable permissions and embedded resource
ownership additionally require Unix/Linux CI. See [reviews/health-probes.md](reviews/health-probes.md).

Verification: the full Windows stable Rust 1.99.1 suite passes **417 tests** (409 integration,
six unit, two doctests). All **28 focused unit/health tests** pass on Rust 1.94.1. Build,
formatting, Clippy with warnings denied, and documentation checks pass. Unix/Linux-specific
new probe tests passed CI for commit `442b06e`.

## Fitness lifecycle and framework checks

`FitnessChecks` owns environment values, registration order, completion identities, and the
early-pass snapshot. `FitnessCheck` wraps synchronous or asynchronous native callbacks.
Clones retain identity: duplicate registrations run twice on their first pass, while distinct
handles with the same name are independent. Clearing the registry deliberately preserves the
Python registration latches; `reset_registration_state` clears those separately.

Call `run_startup_fitness_checks` synchronously before initializing the handler or entering
Tokio to opt into early checks. Rust has no import-time side effect. The same eligibility,
defer/skip flags, and inherited-marker gates apply. A successful early marker is stored in the
owned environment; callers forward it to subprocesses explicitly, without unsafe global
environment mutation. Setup errors defer until worker start; failed hardware checks report
unhealthy and hard-exit immediately. An already-running Tokio runtime defers the early pass.

Early checks run only safe built-ins. Network, CUDA-context initialization, compute, and custom
checks wait for worker start. Disk is checked again once after handler setup; changed tuning
values invalidate only related completed checks. Changed skip flags remove or re-register
their groups. Failed registration rolls back appended checks and latches so it can be retried.
`serverless::start` now runs these checks before any intake, heartbeat, or prestart work.
`start_with_fitness` and `Worker::run_with_fitness` retain explicitly prepared registries;
the low-level `Worker::run` remains usable independently and local mode skips fitness.

`run_fitness_checks` and worker entrypoints report failures through the unhealthy GET heartbeat,
with raw worker authorization, worker-ID substitution, a 256-Unicode-character reason limit,
and a two-second total deadline. Reporting is best effort. `FitnessChecks::check` returns a
typed `FitnessFailure` instead for callers that own failure policy. Native error categories
are translated to Python-compatible reason labels; native configuration/OS details can differ.

Process termination uses a single documented `libc::_exit` call after best-effort stdio flush.
`std::process::exit` would run C exit callbacks, which does not match Python `os._exit`.
The crate denies unsafe code, with a narrow expectation for this FFI primitive and the test
that registers an exit callback. There are no pointers or allocations crossing that boundary.
Child-process tests verify exit codes, bypassed destructors/callbacks, live background threads,
report-before-exit ordering, and termination if an in-progress failure report is canceled.

`GpuFrameworks` explicitly supplies already-loaded native Torch-style and CuPy-style adapters.
No framework is imported, linked, or initialized by default. Torch's initialized-context gate
is checked before availability, preserving the ability to fork later. Initialization checks
device counts, memory, allocation, and synchronization; CuPy visits each device and leaves the
last selected, as in Python. Benchmarks allocate/multiply 1024-square matrices through the
adapter and include synchronization in elapsed time. They measure after completion rather
than interrupting synchronous device operations. Compute failures propagate; setup failures
permit the source's fallback/skip behavior. Physical GPU execution is not claimed by mocks.

The ledger adds 26 adapted symbols and 81 mapped upstream tests. Uncovered reflection,
injected-stream failure, import machinery, and combined end-to-end health cases remain pending.
The focused upstream registry/startup/safe-startup/force-kill/system suite passes 130 tests.
See [reviews/fitness-lifecycle.md](reviews/fitness-lifecycle.md) for native validation and review.

## Worker process startup and blocking hooks

`Worker::start` runs fitness checks, replaces the process-global panic hook, and runs the
signal-aware production worker. After a prestart failure, existing bounded job-failure reporting
finishes before `_exit(1)` forces respawn. `Worker::start_local` uses the same prestart exit
policy without installing the panic hook or contacting webhooks. The convenience serverless
entrypoints delegate to these methods. Successful local output and non-prestart errors remain
returned values for native callers; remaining local CLI/output-comparison behavior is pending.

`Worker::run`, `run_until_signal`, `run_with_fitness`, and `run_local` retain library control
over prestart failures through `Error::Prestart(PrestartError)`. Named hook failures retain the
original `Error` as their source. A failed fallback result delivery still propagates its HTTP
error before the prestart exit, preserving Python's exception precedence. A timed-out report
retains the prestart failure and therefore hard-exits at the process boundary.

The native panic hook logs the panic payload, source location, and a forced native backtrace
through `tracing`. Construction and local execution leave the existing hook alone. Like
Python's assigned `sys.excepthook`, production installation replaces the previous hook and
persists after worker shutdown. Rust invokes panic hooks before unwind, including panics later
caught by an executor or caller; ordinary `Result` errors remain under caller control. No panic
is caught or converted by this logger. This is a documented language-runtime adaptation.

Blocking hooks use `register_sync_prestart_hook`, which creates a named `rp-prestart` thread
and returns its result through an async channel. Dropping the join handle deliberately detaches
it, so Tokio runtime shutdown never waits for a stuck hook. Timeout/cancellation stops waiting;
the blocking work itself may continue until completion or process termination. Spawn failures
and returned hook errors retain their typed cause. If a hook thread panics before sending its
result, the channel yields a broken-pipe failure; the panic's diagnostics follow the installed
hook. Capture context and the current tracing dispatcher propagate into blocking hooks.
Async factories should return promptly; blocking work belongs in the sync API.

`run_prestart_hooks_async` executes an ordered hook sequence under one deadline and emits phase,
hook, and ready progress. A zero timeout invokes neither the hook nor its factory and identifies
the hook as `unknown`, as in Python. The outer `run_prestart_phase` preserves the empty-registry
no-op even for zero timeout. A hook's own `Error::Timeout` remains a named hook failure rather
than being mistaken for the phase deadline.

The ledger adds eight adapted symbols and 15 mapped upstream tests; registration and phase
execution are relabeled adapted to reflect the explicit native APIs and typed return values.
The matching upstream worker/prestart lifecycle suite passes **60 tests**. Child processes
exercise queued failure delivery, empty and stalled reporting, hook timeouts, background threads,
local failure exit, panic-hook scope, and fallback-delivery error precedence. Native captured
logs and bounded failure payloads are implemented below; hosted failure handling and reflection
cases remain pending.
See [reviews/worker-process.md](reviews/worker-process.md) for validation and review.

## Capture policy and text bounds

`serverless::capture` ports the capture-mode policy and text bounds used by failure reports.
`CaptureMode::from_env` reads `RUNPOD_LOG_CAPTURE`; `From<&str>` parses an explicit value
without mutating the process environment. Modes are trimmed and case-insensitive. Missing
or unknown values select `Auto`; only `All` enables handler log capture. These helpers do
not install a stream interceptor or enable capture by themselves.

`RingBuffer` retains the latest characters across writes, with a default limit of 16,384.
Write counts and bounds count Unicode scalar values, matching Python characters on valid
Unicode text, rather than UTF-8 bytes. `clip` keeps the head and tail and inserts the exact
omitted-character marker. The marker adds to the requested limit; unchanged text is borrowed.
Python's unusual zero-limit behavior is preserved: a zero-sized buffer is unbounded, and
clipping limits zero or one retain the whole tail because Python's `[-0:]` means the full
string. Native `usize` limits exclude negative limits; Rust strings exclude lone surrogates.
Mode whitespace follows Rust's standard Unicode trimming; Python-only control separators
are not emulated.

`tools/capture_fixtures.py` records 26 Python cases in `tests/fixtures/capture.json`, bringing
the differential fixture total to **1,231**. Seven Rust tests cover these cases and four
directly mapped upstream tests. The full upstream capture module passes **22 tests**.
The following milestone adds scopes and worker integration with explicit native adaptations.
See [reviews/capture-bounds.md](reviews/capture-bounds.md) for the scoped review.

## Scoped capture and failure reports

`Capture` owns a shared bounded buffer. `scope` follows future polling, so concurrent jobs
on one executor thread retain separate output. `scope_sync` covers blocking calls. Nested
scopes, pause/resume, unwind, and canceled futures restore their enclosing context. Snapshots
return owned strings; no lock is held across a callback or await. Capture bookkeeping is best
effort and cannot replace a successful output write with a capture error.

Python's replaceable `sys.stdout` and `sys.stderr` become explicit Rust text writers:
`capture::stdout()`, `capture::stderr()`, or `TeeWriter<W>` around an `io::Write` destination.
`write_text` forwards the complete UTF-8 text, then mirrors it and returns a character count.
`write_lines` preserves order without inserting newlines; `flush` and I/O errors are forwarded.
Direct access to the underlying destination replaces Python attribute forwarding. These writers
do not replace process-wide stdout handles: ordinary `println!`, raw destination writes, and
external-process output bypass capture. This is a native API adaptation that handler authors
must adopt, not a claim to intercept all operating-system output. Python dynamic `TextIOBase`
identity and global-proxy identity tests remain unmapped.

Worker configuration owns capture policy. `Auto` captures a prestart phase with hooks;
`All` additionally captures single and streaming handlers; `Off` attaches no captured logs.
Public `run_job` reads the environment, while `run_job_with_capture` takes an explicit policy.
`run_job_generator` and its explicit-policy counterpart yield output envelopes followed by at
most one terminal error envelope. Sync factories and async stream polling both run in the
appropriate scope. Caller-returned error objects retain their normal protocol meaning and are
not treated as thrown failures. Generator policy is resolved when the Rust stream is created.

Blocking prestart threads copy capture context and the current tracing dispatcher. User-created
threads/tasks copy context explicitly through `Capture::current()` and `scope`/`scope_sync`;
Tokio spawning does not automatically inherit Python-style context variables. SDK prestart
progress is forwarded while capture is paused, so it does not obscure hook diagnostics. A
timed-out detached thread may finish later; already-created payloads retain their earlier snapshot.

Startup failures include the original error message, explicit hook name, worker identity,
native SDK version, native backtrace, and optional log tail. Message and traceback are clipped;
logs retain their latest 16,384 Unicode scalar values. Empty hook/log fields are omitted.
Shutdown reports retain the separate `PrestartCancelled` event. Worker startup logs the report
once and preserves report-before-exit ordering and transport-error precedence. Lower-level
hook methods keep their typed-error contract, and local worker failures log the structured
report before returning or applying process exit policy.

Rust reports `runpod::Error` or `PrestartTimeout`, with native backtraces captured at the reporting
boundary; it cannot reconstruct Python exception classes or interpreter traceback objects.
Handler error messages and tracebacks remain untruncated. Single failures carry optional `logs`
in their JSON error; streamed failures append `\nlogs:\n` and the captured tail to the native
error text. Successful results and disabled/empty capture omit added logs.
The native local stream collector retains its existing typed-error return contract; use
`run_job_generator` or production worker execution for streamed error envelopes and log tails.

This milestone maps 13 additional API symbols and 23 adapted upstream tests. `run_job` and
`handle_job` are relabeled adapted to reflect native errors/output capture. The upstream
capture/prestart/worker suites pass **96 tests**, and the job module passes **29 tests**.
Native tests cover forwarding, isolation, cancellation, thread propagation, excluded progress,
bounded fields, opt-in behavior, generators, and real mock-webhook reports. The prior MSRV CI
failure exposed an upload-test assumption about concurrent request arrival order; commit
`6005be8` fixes that assertion while retaining multipart completion-order checks.
See [reviews/capture-context.md](reviews/capture-context.md) for validation and review.

## Native development HTTP worker

`serverless::WorkerApi` owns a native Axum HTTP server with `/run`, `/runsync`,
`/status/{job_id}`, `/stream/{job_id}`, and optional `/{endpoint_id}/realtime` routes.
`WorkerApi::new` starts no tasks. Register hooks, select an endpoint with `with_endpoint_id`,
then use `serve(listener, shutdown)` for embedding or `start(host, port, concurrency)` for
process startup. `start_realtime` rejects prestart hooks. The environment-based
`serverless::start` selects realtime hosting when `RUNPOD_REALTIME_PORT` is nonzero,
using the owned fitness environment's endpoint ID and concurrency; this path skips queue
fitness checks and local test-input loading. Explicit `WorkerApi::start` selects hosted
mode independently of that environment. Python command-line flag parsing remains pending.

```rust,no_run
use runpod::{Result, serverless::{Handler, WorkerApi, WorkerConfig}};

#[tokio::main]
async fn main() -> Result<()> {
    let handler = Handler::sync(|job| Ok(job.input));
    WorkerApi::new(WorkerConfig::from_env()?, handler)?
        .start("127.0.0.1", 8000, 1).await
}
```

The development API preserves deferred execution: `/run` only stores a job, `/status`
executes and removes it, and `/stream` accepts generator handlers and returns one collected
JSON array. Missing jobs return HTTP 200 with `FAILED`. Unsupported streaming retains the
job. Generator failures become HTTP 500 and retain the queued job; empty object results
can produce HTTP 500 after scheduling their callback. Response-level null fields are
omitted while nested null values remain. Handler inputs retain the simulation-only null
`webhook` field and never receive the submitted callback URL.

Callbacks carry the source result envelope or stream array, without worker authorization.
`serverless::send_webhook` also exposes the delivery helper's boolean success result.
They follow redirects, have a ten-second native request deadline, and are best effort;
callback failures cannot change the HTTP response. Native Tokio tasks replace daemon
threads. The source requests timeout bounds individual socket waits; the native deadline
bounds the whole delivery. Callback tasks can outlive listener shutdown until the caller's
runtime ends. Explicitly shared `JobsProgress` replaces the Python process-global singleton.
Queued jobs and active realtime requests appear in heartbeat snapshots; synchronous
simulation calls do not. Heartbeats start with `serve`, including its prestart phase.

Hooks finish before the listener accepts requests. Failed hooks return a typed error from
`serve`; `start` logs the structured startup report and force-exits with code 1, including
when a hook leaves a detached thread. Shutdown stops accepting connections and drains
active HTTP requests. A shutdown signal during prestart cancels the startup future without
accepting work. Native server concurrency comes from Tokio; more than one server process
is rejected. Upstream also cannot launch multiple Uvicorn workers from its application
object, and additionally rejects multi-process hosted mode when hooks are registered.

Request validation matches the captured Pydantic field/type errors and ignores unknown
fields. JSON syntax failures retain HTTP 422 and its error structure, but use the native
parser's diagnostics and positions. Rust integer/Unicode limits and native typed errors,
backtraces, panic semantics, and explicit capture writers remain the documented runtime
adaptations. A native streaming handler works on realtime routes; Python synchronous
generators work there, while Python async-generator encoding fails. The native adapter
unifies the two stream kinds. No default two-megabyte body cap is introduced.

OpenAPI schemas and route descriptions are captured from Python; version and introductory
text describe the native SDK. Swagger UI is served at `/`, `/docs` redirects there, and
ReDoc and the OAuth redirect page are also available. The latter preserves FastAPI's MIT
notice in `src/serverless/http/LICENSE-FASTAPI`. Trailing-slash redirects preserve encoded
paths and query strings. Reverse-proxy trust and forwarded-header policy are left to the
embedding/deployment layer instead of implicitly trusting Uvicorn proxy defaults.

`tools/http_worker_fixtures.py` records **85 Python HTTP scenarios**, bringing the
differential total to **1,316**. Real local HTTP tests compare response bodies, status
codes, handler arguments, tracker state, and callbacks. Additional tests cover startup
gating, heartbeat snapshots, shutdown, process exits, realtime environment selection,
large bodies, documentation, and structured handler failures. Source HTTP/prestart tests
pass **22 tests**. See [reviews/http-worker.md](reviews/http-worker.md) for final verification.

## Native Apps class-API hosting

The Python SDK's `ApiHandle` declares routes and sends client requests; the actual server
is maintained separately in [runpod-workers/sdk-runtime](https://github.com/runpod-workers/sdk-runtime).
The native adapter is characterized against runtime commit
`14f70227008c1ec317e3fe1865a4688b14f5d432`, specifically
[`api/server.py`](https://github.com/runpod-workers/sdk-runtime/blob/14f70227008c1ec317e3fe1865a4688b14f5d432/src/runpod_sdk_runtime/api/server.py).
This reference does not change the pinned Python SDK commit or inflate its symbol/test
inventory. Separate contract coverage is recorded in [parity/runtime-api.json](parity/runtime-api.json).

`ApiHandle::serve(listener, shutdown)` initializes an owned snapshot of the registered
native API, then accepts HTTP requests. `start(host, port)` additionally binds the listener
and handles shutdown signals. Handlers keep their closure-owned state; clones share that
state rather than implicitly constructing Python class instances. Startup failures retain
their typed Rust error and never open the request gate. `/ping` returns `{"status":"healthy"}`
after initialization. The embedding API does not expose the framework-only pre-lifespan
204 response that Python tests can observe by bypassing Uvicorn's startup gate.

The source route wrapper is deliberately simple: GET calls have no arguments. POST, PUT,
PATCH, and DELETE pass one positional argument when the body parses as non-null JSON.
Empty, malformed, or null bodies produce no arguments, and content type does not control
JSON parsing. Query strings and captured path parameters are not passed to the handler.
Native JSON values preserve this contract; non-finite Python numeric values remain outside
the native serde_json domain. Handler-returned `error` fields are ordinary data here.
Returned native errors become HTTP 500; this route API does not apply serverless job envelopes.

Routes retain handler-name ordering, first-full-match precedence, and first-partial-match
405 responses. The standard `str`, `int`, `float`, `uuid`, and `path` converters are supported;
unknown converters and duplicate parameter names fail before initialization. Trailing-slash
redirects preserve encoded paths and query strings. `/docs`, `/redoc`, `/openapi.json`, and
the OAuth redirect page follow framework route precedence. Documentation titles are escaped.
The Apps and development-worker servers share the same documentation renderer/assets.

Synchronous native functions use the existing blocking executor; asynchronous functions
remain on the caller's runtime. Native streams collect into a JSON array. Python synchronous
generators can also encode as arrays; Python async-generator encoding has different failure
semantics, as already documented for the native streaming adaptation. Shutdown stops new
connections and drains accepted HTTP requests. Dropping the server future is not an explicit
handler-cancellation API. Axum owns HTTP connection settings; Uvicorn's 600-second keepalive
configuration and implicit proxy trust are not claimed as transport parity.

The existing `NativeFunction` JSON contract remains available alongside raw HTTP handlers
and native factories described below. Live source/binary loading remains pending integration.
`NativeApi::asgi_factory` is still explicit Python-worker metadata and
is rejected by the native host. Deployed class serving does not expose `/execute` or
`/_runpod/sync`; those belong to the separate live dispatcher contract. Native binary
packaging/launch remains pending. Artifact extraction and manifest-selected native
declarations with mount bindings are covered by the later bootstrap/loading milestones.

`tools/apps_http_fixtures.py ../assets/sdk-runtime` captures **18 scenarios / 150 requests**
from the pinned runtime. Real TCP tests compare response status/body, Allow/Location headers,
handler calls, and the full generated OpenAPI document. Additional tests cover startup
gating/failure/cancellation, shutdown draining, invalid route patterns, escaped docs,
large bodies, streams, and calls through the SDK's actual `ApiHandle` client. These are
additional runtime fixtures, separate from the **2,328 SDK differential cases**.
See [reviews/apps-http.md](reviews/apps-http.md) for verification and limitations.

### Raw responses and native factories

`NativeHttpFunction::{new, sync}` returns an owned Axum 0.8 `Response`. Register it with
`NativeApi::http_route` to retain status, repeated headers, cookies, redirects, and binary
or streaming bodies. The class adapter's input-body convention and route validation/order
are unchanged. `call_http_handler` also wraps existing JSON methods as JSON responses;
`call_handler` rejects raw-response methods instead of consuming or reinterpreting their
bodies. Synchronous response construction runs on Tokio's blocking pool. Streaming bodies
remain lazy and flush before completion; a body error after headers aborts the transport,
since the already-sent status cannot become an HTTP 500. Axum/Hyper retain responsibility
for protocol framing and statuses that prohibit response bodies.

`NativeApi::http_factory` is lazy at registration and called once per server invocation.
It performs asynchronous startup and returns `NativeHttpApp` containing an owned Axum
router. The router can contain ordinary Axum handlers/middleware or a Tower fallback
service. Factory failures never open the request gate. Cancellation drops the startup
future and owned guards. `on_shutdown` runs after accepted requests drain, and cleanup
errors propagate; dropping the server future does not run asynchronous cleanup.

The Python runtime detects a framework app's route inventory. Axum keeps that inventory
private, so the factory explicitly chooses `FactoryHealth`: `Add` supplies GET `/ping`
and 405 for other methods; `Preserve` delegates to an existing route even when it only
accepts POST; `Wrap` intercepts every method, matching opaque ASGI health wrapping.
Added/wrapped health handles decoded `/ping` before router dispatch, outside the router's
middleware. Select `Preserve` and define the health route in the router when middleware
must apply to it. Factory routes, redirects, method defaults, schemas, and middleware use
the chosen native framework's behavior; this adapter does not recreate FastAPI or ASGI.
Factories do not inherit the class adapter's route restrictions or generated docs.

Native factories emit neither fabricated route inventory nor a Python `asgiFactory`
qualified name. Compiled factory closures are not serializable Python deployment artifacts;
native image/binary deployment remains pending. Explicit Python source/symbol metadata
continues to describe separately supplied Python worker code.

`tools/apps_http_response_fixtures.py` records **9 response cases and 4 factory scenarios
(33 requests)** from the pinned runtime. Tests compare exact bytes/status, selected headers
including repeated cookies, health ownership, and lifecycle events. Seven native integration
tests also cover startup gating/failure/cancellation, real Axum extractors, request/body
draining, typed cleanup errors, and streaming transport failures. These fixtures remain
separate from the SDK symbol inventory and its 2,328 differential cases.
See [reviews/apps-http-responses.md](reviews/apps-http-responses.md).

## Directory volume cache

`utils::VolumeCache`, re-exported by `serverless`, ports the pinned SDK's complete
directory-mirror workflow. `VolumeCacheConfig` owns its namespace, volume path, best-effort
policy, and optional positive worker/chunk sizes. A missing/empty namespace falls back to
`RUNPOD_ENDPOINT_ID`; no namespace or absent volume directory makes operations no-ops.
Directories are resolved at construction. Rust paths and `NonZeroUsize` replace Python's
scalar/bytes dispatch and runtime integer validation. The default copy pool is
`min(32, available_parallelism * 4)`; hydrate uses explicit `max_workers` or 64 workers,
with 8 MiB ranges. Native CPU availability can reflect process affinity/quota.

The volume layout remains `.cache/<namespace>/{manifest.json,small.tar,big/}`. Files
below 256 KiB are packed together; larger files stay unpacked. `.lock`, `.rpvc.tmp`,
`refs/`, `.no_exist/`, and symlink sources are excluded. Two-second modification-time
tolerance, unchanged-file skipping, deletion-triggered small-bucket repacking, pack-failure
fallback to unpacked files, and omission of failed big copies from the manifest are retained.
Big-file sync compares against the manifest's source metadata, so volumes that cannot
retain timestamps do not force repeated copying. Deletion-only archive repacks return zero
changed entries. Old unreferenced big files are not pruned, matching the source.

Publication uses unique adjacent temporary files and atomic replacement, with the manifest
written last. No cross-file transaction or writer lock is added; concurrent writers retain
last-writer-wins behavior. Hydration checks both configured destination boundaries and
the big-mirror source boundary after resolving symlinks. As upstream, configuring a
filesystem root itself does not authorize descendant extraction; use concrete cache dirs.
Windows paths must share the current filesystem drive, matching Python's relative-path
constraint. Drive-relative namespaces are explicitly rejected to prevent path replacement.

Native tar/PAX replaces external `tar` selection and its temporary listing file. Archive
bytes/header ordering are not promised identical, but Python/Rust interoperation checks
verify member names, content, and fractional mtimes. Manifests escape Unicode to ASCII,
matching Python's default JSON writer and locale-based reader. Non-UTF-8 native paths and
Python surrogate strings are outside this JSON interchange domain. Sparse inputs are
written as regular members; regular, contiguous, and GNU sparse files can be read. Links,
directories, and devices are skipped on extraction. Hard-linked input names retain link
entries, so only the original regular member is restored, preserving the source behavior.
Extraction validates all headers before writing and rereads the same open archive handle.
Permission filtering follows Python's regular-file data filter on Unix; Windows permission
bits retain their native filesystem meaning.

Large files hydrate with offset-major ranges, sorted by source path, and at most the
configured number of workers. Scheduling holds O(files + workers) state and buffers at most
workers × chunk bytes. Every range opens independent handles, checks pinned size/mtime,
and rejects short reads or a visible source replacement. Rust seeks on each private handle
instead of using POSIX-only `pread`/`pwrite`. Unix ENOMEM/EIO/EAGAIN retain eight attempts
with jittered exponential backoff; native out-of-memory/would-block errors are retryable
across platforms. Other Windows error codes are not interpreted as POSIX errno values.
Successful files receive the manifest mtime before atomic publication. Metadata failures
remain best-effort, and failed files retain their prior local contents. Prepared temporaries
are owned and cleaned during errors/unwinding. Identity deliberately excludes inode numbers,
as in the source; it is a size/mtime check, not a content-integrity guarantee.

`hydrate` and `sync_inline` are blocking operations returning counts; `sync` schedules a
background thread. `enter` returns a dereferencing guard that hydrates first and schedules
sync on drop, without swallowing caller errors/panics. Clones share an owned pending-sync
registry. `flush(timeout)` waits per current sync and returns the number detached after
timeout; dropping the last owner uses 30 seconds per pending sync. Rust has no implicit
Python `atexit` hook: keep the cache alive and flush/drop it before process exit. Process
termination and SIGINT are not translated to `KeyboardInterrupt`. Panics unwind rather
than becoming best-effort Python exceptions; scoped range workers stop taking new work
on failure/unwind. Explicit filesystem errors preserve typed causes. Individual file errors
are logged/skipped even with `best_effort=false`, while operation-level failures propagate.

`tools/volume_cache_fixtures.py` records **154 Python differential cases**, bringing the
SDK case total to **1,470**. Native tests cover public round trips, manifest failures,
atomic publication, range ordering/retries/replacement, bounded scheduling, scope and
background completion, corrupt archives, Unicode, hard links, sparse files, and path
boundaries. `tools/verify_volume_cache.py` checks Rust output in Python and Python PAX
output in Rust; stable CI runs it on Linux, Windows, and macOS. No live FUSE/network volume
was used. The ledger maps all 48 module symbols as native adaptations and 84 source test
functions; 13 source tests remain pending, including additional filesystem fault injection,
interrupted-wait behavior, and environment/path-type cases. See
[reviews/volume-cache.md](reviews/volume-cache.md) for validation evidence.

## Compute selection and catalog requests

`compute` ports the helpers shared by Python's catalog, pod, and endpoint commands.
`Client::list_catalog_gpus`, `list_catalog_cpus`, and `list_catalog_datacenters` query
REST v2 using owned credentials and service URLs; matching blocking-client methods
use the existing owned runtime. `GpuCatalogOptions` groups the optional GPU filters.
Without a nonempty product, stock/count/cloud/CUDA filters are omitted. An explicitly
empty product still sends `product=`, and empty include/CUDA lists are omitted.
Raw JSON records preserve unknown fields, null values, and absent fields for CLI output.

GPU matching prefers exact IDs/names, then pools, token inclusion, and compact ID
substrings. Matching preserves catalog order; vendor noise tokens are ignored only
at the token stage. Endpoint selection deduplicates pools in first-seen order and
excludes unselected members of those pools. CPU resolution distinguishes unknown
and ambiguous queries. Region aliases, ID-prefix matching, ordered deduplication,
and error messages listing known choices are retained. Unknown availability labels
rank zero; ties preserve the first label, including an unknown nonempty label.
Empty GPU/CPU queries retain Python's matches against absent/empty optional fields.

GPU counts and CUDA components use checked `u64` ASCII parsing. Unicode digits,
Python's unbounded integers, and its additional control-character whitespace are
native adaptations rather than runtime emulation. Ordinary Unicode text uses Rust's
case conversion and whitespace rules. CUDA floors compare numeric major/minor values;
equal floors keep the first spelling and exact versions retain duplicates and order.
Mixed floor/exact filters still fail. Resolution errors have a distinct typed
`Error::Compute(ResolutionError)`; malformed JSON records produce `InvalidResponse`
errors rather than Python lookup/type exceptions. Record validation can occur earlier
than Python's short-circuit matching on malformed data.

The original compute tests pass **35 parameterized cases**. Seventeen named Rust tests
map all source test functions, with four additional tests covering malformed records,
numeric limits, API error causes, blocking calls, authentication, and unknown fields.
`tools/compute_fixtures.py` records **111 actual Python cases**, bringing the SDK total
to **1,581**. All requests are tested offline. See [reviews/compute.md](reviews/compute.md)
and [examples/compute.rs](examples/compute.rs). The native catalog command now uses
these helpers; other CLI commands remain pending.

## Native catalog CLI

`rp catalog` and `runpod catalog` implement GPU, CPU (`--cpu`), and data-center
(`--datacenters`) queries through the existing owned client and credential store.
All source catalog options are available. Explicit defaults still count as supplied
options when rejecting flags in the wrong mode. Repeated scalar flags and pod/serverless
flags use the last value; repeated region/CUDA filters retain their source order.
Usage errors return 2 without network access, API/resolution errors return 1 on stderr,
and successful JSON output contains only the result array on stdout.

GPU prices retain source zero/null handling, per-device maximum-price filtering,
four-decimal total-price rounding, and stock/price/name ordering. Region filters
recompute stock from matching data centers. CPU rows retain stable stock ordering
and four-significant-digit per-vCPU prices. Unknown JSON fields remain intact.
CLI GPU/vCPU counts are checked `u32`, and minimum VRAM is checked `i32`; arbitrary
Python integers and Unicode numeric spellings are not emulated. Invalid catalog
field types return structured errors instead of Python type/lookup exceptions.

Native Clap help and parser diagnostics replace Click's layout. Table headings,
values, order, literal markup, and empty messages are retained in unwrapped columns
using Unicode display widths; Rich colors, terminal folding, and Rich table objects
are not reproduced. JSON uses two-space indentation and escapes Unicode; number
spellings follow Serde while preserving JSON numeric values. General output helpers
accept JSON data rather than arbitrary Python objects and their `str`/`repr` methods.
Background update checks and shell completion remain pending with the other CLI work.

`tools/catalog_fixtures.py` captures **120 actual pinned Python invocations**, bringing
the SDK differential total to **1,701**. The 24 native executable tests cover all 21
catalog source test functions plus added edge, alias/parser, and malformed-response
checks. Two output-helper counterparts cover cell rendering and intact identifiers.
The unchanged upstream file passes 69 parameterized cases, including its still-pending
completion test; the two selected output-helper tests also pass. All API traffic in
these tests uses local mocks and synthetic credentials. See [reviews/catalog.md](reviews/catalog.md).

## Native volume CLI

Both executable names now support `volume list`, `get`, `create`, `update`, `expand`,
and `delete`. Network/global selection, paginated listing, JSON, fields, table values,
creation payloads, name-only updates, and size checks retain the source behavior.
Creating at an exact data center skips catalog lookup; region-based placement filters
by offered storage tiers, picks a sole match, or asks the user when several remain.
Updating to the current size is allowed, while `expand` requires growth. Expansion
accepts either a total or `+N`; neither update nor expansion can shrink storage or
exceed 4096 GB. Deletion fetches the current record before confirmation unless `--yes`
is supplied, and sends no DELETE when confirmation is declined or unavailable.

Shared pagination follows cursor pages with original query parameters and no added
retry policy, matching the CLI helper rather than the SDK's retrying pagination API.
Malformed collections/cursor fields return typed errors. Native usage errors return 2,
API errors return 1, and declined/cancelled prompts return 1 with `Aborted!`.
Network tests use Python's real HTTP error conversion; in particular, 401 responses
produce the fixed authentication message rather than a mocked arbitrary exception text.

Terminal eligibility requires both stdin and stdout to be terminals. Dialoguer provides
native fuzzy selection and confirmation, and selected IDs remain separate from display
labels. Labels truncate to available Unicode display cells, retaining complete IDs for
requests. Native prompt layout, terminal styling and key handling replace InquirerPy
and Click; field/table values remain intact. Counts use checked native integers: create
and update sizes are `u32` constrained to 10..4096, and expansion syntax uses ASCII
`u64` with checked addition. Arbitrary Python integers/Unicode digits are not emulated.

`tools/volume_cli_fixtures.py` captures **58 pinned Python invocations**, bringing the
SDK differential total to **1,759**. Nineteen process tests cover 18 named source tests
and additional edges; three native interaction tests cover the remaining command
contracts with recorded choices/confirmations. Eight new shared-helper tests map cursor
pagination, error conversion, field rendering, terminal checks, selection and confirmation.
The unchanged upstream volume/API/output files pass **67 parameterized cases**; that
source run also includes completion tests whose native mappings remain pending.
A real Windows terminal smoke check exercised declined/accepted deletion and fuzzy
selection against a local mock HTTP server. No real Runpod resource was changed.
See [reviews/volume-cli.md](reviews/volume-cli.md).

## Endpoint configuration and shared options

`configuration::{ContainerOptions,ScaleOptions,EndpointOptions}` replace untyped option
bags. Async and blocking clients resolve GPU pools, CPU flavors, regions, and registry
names into request payloads. They do not apply changes to resources. `EndpointType`
and `Flashboot` supply typed choices, CLI-spelling `Display`/`FromStr`, and explicit API
wire values. `parse_env`, `scale_patch`, `mutable_endpoint`,
`validate_endpoint_document`, and `endpoint_patch` are also available independently.
See [examples/configuration.rs](examples/configuration.rs).

Omitted values remain omitted; explicit zero/false/empty container values are retained.
Assignments split only at the first equals sign, later keys win, and removals happen
after environment merging. Invalid assignment errors never echo the value. Empty registry
strings clear credentials without lookup; nonempty values use ordered, paginated lookup.
Container command strings preserve the source's quoting, empty arguments, literal `#`,
and escaped-newline behavior without executing a shell or expanding variables.

Queue endpoints receive the source default queue-delay scaler; load balancers require
request-count scaling. Partial worker changes preserve other worker fields. GPU counts
must agree across selectors; pool/exclusion order and CUDA floor/exact/`any` transitions
are preserved. Existing CPU/GPU endpoints cannot switch hardware kind. Network/global
mount replacements retain the other mount class and unknown current mount keys.

Editable documents drop runtime fields and mount metadata while retaining mutable values.
Validation checks allowed top-level types, nested field names, string environment values,
FlashBoot spelling, and hardware-kind restrictions. It deliberately leaves nested value
types/ranges to the service, matching Python. Missing edited fields are unchanged; missing
and null values compare equally. Numeric JSON equality includes `4` versus `4.0` and
boolean versus integer values, without losing distinctions between large integers.
GPU pools/exclusions are updated together, and a null CUDA floor clears it with `""`.

Native counts are `u64`, timeout multiplication is checked, and queue delays must be
finite. Python's arbitrary-size integers, Unicode count digits, and nonstandard NaN/
Infinity JSON are not emulated. JSON parser diagnostics use Serde's wording. Malformed
response records return typed errors rather than Python lookup/type exceptions. Helper
APIs do not enforce CLI-required image/name flags or CLI numeric ranges. The native
endpoint resource CLI applies those constraints as described below; log orchestration
remains pending in the ledger.

`tools/endpoint_config_fixtures.py` captures **102 actual pinned Python cases**, bringing
the SDK differential total to **1,861**. Fifteen named Rust counterparts map all ten
shared-option source tests and five direct endpoint-document helper tests; additional
groups cover configuration payloads, scaling, quoting, and patch edge cases. Four native
tests cover enum conversions, the blocking facade, numeric bounds, and malformed data.
The unchanged upstream subset passes **30 parameterized cases**. No live resources are
created or modified. See [reviews/configuration.md](reviews/configuration.md).

## Native endpoint resource CLI

Both executable names support `endpoint list`, `get`, `create`, `update`, `edit`, `scale`,
`delete`, `workers`, and `releases`. Configuration flags preserve repeated selector order,
last scalar value, required creation flags, numeric ranges, empty registry clearing and
the shared payload builder's merge behavior. IDs are percent-encoded as path segments.
List filters are case-insensitive name globs and endpoint type. Repeated stars, bracket
classes, unmatched brackets and literal braces retain name-matching behavior; the native
`glob` crate performs matching after a small normalization step.

Missing IDs for `get` and `edit` use the existing native fuzzy selector. Noninteractive
invocations fail before credential loading or listing endpoints. Deletion always reads
the current name before confirmation, including when `--yes` is supplied. Fields, list/release tables and worker
summaries retain source values and ordering, including compact GPU selection, stale
versions, uptime, sorted environment keys and filtered release differences.

Editing accepts a file or stdin, or invokes `$VISUAL`, then `$EDITOR`, then a native editor
fallback. File options are opened during option processing before client construction or
API requests. Native editor execution passes parsed arguments directly without a shell;
quoted paths, save detection, nonzero exits, Windows BOM/newlines and unsaved changes are
handled. Modified times are moved back two seconds before launch, preserving reliable
save detection on coarse filesystems. Invalid documents can reopen with the invalid text
retained and a default-yes retry prompt. Changed documents print their patch before the
apply confirmation; unchanged documents return `no changes` or an empty JSON object.

Updates flush their result before observation. Rollout failures therefore retain output
from an already-applied update. Runtime-only changes (name, workers, scaling, timeout and
FlashBoot) skip release queries. Otherwise `--wait` captures the old version, applies the
patch, suppresses unchanged-version progress, and reports the new release until live or
the observation deadline. A failed baseline read warns on stderr and still applies the
update. JSON update results stay on stdout, with rollout progress on stderr. Deadline
expiry and Ctrl-C detach successfully without cancelling the remote rollout. The rollout
deadline does not shorten HTTP request timeouts, matching the source distinction.

Worker watch refreshes at the requested interval and exits successfully on Ctrl-C,
including while awaiting a request. JSON mode emits one complete object per line. Human
terminal frames clear their previous physical rows, accounting for Unicode width and
wrapping. Native unwrapped tables/prompts replace Rich layouts; nonterminal human watch
prints successive snapshots, while JSON watch is the machine-readable interface. Compact
JSON number/spacing/Unicode spellings follow Serde while preserving values.

Counts are checked `u64` values; numeric CLI bounds are retained and floating durations
must be finite. Arbitrary Python integers, non-ASCII integer spellings and nonstandard
JSON numbers are not emulated. Native parser, filesystem and JSON diagnostics replace
Click/Python wording where documented. Usage/file-option failures exit 2; API/editor/
validation failures and declined confirmation exit 1. Successful observation detachment
exits 0. Streams are flushed incrementally, and broken output pipes exit successfully.

`tools/endpoint_cli_fixtures.py` runs **75 parameterized upstream cases** and captures
their invocations, then adds direct rollout and edge characterizations. Its **116 pinned
Python fixtures** regenerate byte-identically, bringing the SDK differential total to
**1,977**. The ledger adds 32 adapted symbols and 42 mapped source tests. Tests compare
requests, payloads, queries, exit codes, JSON values, table/field values, editor inputs,
selection choices and polling sequences. Additional native checks cover a real editor
child process, a real transport timeout, wrapped terminal row accounting, and argument
validation without credentials. Windows terminal smoke checks exercise editor confirmation,
rollout Ctrl-C and JSON worker watch
against loopback mocks. No real Runpod resource is created or modified.
See [reviews/endpoint-cli.md](reviews/endpoint-cli.md).

Endpoint log workflows are covered by the later shared log command milestone below.
Shell completion remains pending rather than being inferred from the resource commands.

## Native job inspection and control

`rp endpoint job get`, `cancel` and `retry` address the queue data plane directly, with
no control-plane endpoint lookup. IDs are encoded independently as URL path segments;
an empty job ID retains the source's omitted suffix. Each request has the source runner's
ten-second deadline. POST requests send no body. `endpoint purge` confirms before loading
credentials or making a request, so a noninteractive invocation requires `--yes` first.

Job fields retain their source order, lowercase status, milliseconds-to-seconds conversion,
and string/JSON output distinction. Raw JSON preserves unknown response fields. Cancellation
messages keep the requested ID; retry messages prefer the returned ID and fall back to the
requested ID. Purge distinguishes an absent removed count (zero) from explicit null (`None`).
HTTP errors retain the numeric status and trimmed raw body, including JSON error text;
empty bodies use a standard reason phrase, and 401 keeps the runner's authentication message.

The native client retains the documented no-redirect/no-implicit-replay policy and owned
environment/profile credential lookup. Reqwest/Serde diagnostics replace Python exception
wording; custom HTTP reason phrases are replaced by standard reason phrases. JSON displayed
inside a human-readable output cell uses native compact spacing and Unicode spelling.
These formatting changes preserve the decoded value and do not affect raw JSON results.

`tools/endpoint_job_fixtures.py` passes three unchanged upstream test functions and captures
**31 actual Python invocations** across eight groups. Eight native process tests compare
methods, encoded paths, authorization/content headers, absent request bodies, exit codes,
JSON values, field values and success messages. Additional groups exercise output shapes,
ID encoding, missing-field fallbacks, HTTP errors and confirmation without credentials.
Fixtures regenerate byte-identically; all requests use loopback mocks. The ledger adds
nine adapted symbols and three ported source tests, taking the SDK differential total to
**2,008**. See [reviews/endpoint-jobs.md](reviews/endpoint-jobs.md).

## Native job waiting and streaming

`endpoint job wait` prints final output, with optional positive finite `--timeout` seconds.
Each status request is bounded by ten seconds and the remaining observation deadline.
`endpoint job stream` prints chunks immediately with a sixty-second request bound, retrying
read timeouts while the last observed status remains nonterminal. Missing, null and empty
chunk lists permit polling or fallback to the final status request. Empty polls sleep one
second for wait and half a second for stream; polls with chunks do not sleep.

Once a response has a terminal status, the deadline no longer prevents draining chunks or
fetching a final result. Successful streamed output is not duplicated by a final status
request. Failed streams drain partial output before fetching failure details. Strings join
without added newlines until required by a JSON chunk or the final output boundary. JSON
mode emits individual JSON chunk values; a failed final result is printed before exit 1.

Ctrl-C while polling or sleeping prints a resume command and exits 1 without cancelling the
job. The source's subsequent empty `Error:` line is intentionally retained: Click's Abort
inherits RuntimeError and is caught by the source data-plane wrapper. The fallback status
read sits outside `_follow_job`'s interrupt handler; an interrupt there prints a newline
and `Aborted!`, with no resume hint. This distinction is captured in Python fixtures.

Native async request deadlines bound the whole request, including response-body reads;
they are not Python's socket read-idle deadlines. Reqwest timeout classification separates
connection timeouts from other timeout failures, retaining native causes. Native JSON
spacing/Unicode spellings preserve decoded values. The native wait command emits final
output without the source's Rich terminal spinner. Required status/chunk fields produce
typed malformed-response errors instead of Python key/type exceptions.

`tools/job_follow_fixtures.py` runs two unchanged upstream tests and captures **47 actual
Python observer invocations** in sixteen groups. Named Rust counterparts and additional
fixture groups compare request order/timeouts, sleeps, output values and exact stderr/exit
behavior. Three executable tests verify routing, argument validation before credentials,
and a real delayed HTTP response bounded by the wait deadline. Real Windows terminal tests
interrupt both wait and stream against loopback servers. The ledger adds four adapted
symbols and two ported source tests; the SDK differential total is **2,055** at this stage.
Additional run-specific fault-injection tests remain separately tracked in the test ledger.
See [reviews/job-follow.md](reviews/job-follow.md).

## Native endpoint submission and load-balancer calls

`endpoint run` reads a file, stdin (`--input -`) or inline `--data` JSON before loading
credentials. File options open before callback-level flag checks. Async and streaming modes
are mutually exclusive; a private enum represents the parsed choice. Invalid JSON and
conflicting inputs fail before endpoint lookup. Valid requests read endpoint metadata to
choose queue submission or a load-balancer HTTP call; mode-specific checks then follow the
source order. Whole-body JSON null is treated as no body, so it fails for queue endpoints.

Queue payloads preserve an existing `input` key even when its value is null, false or empty.
Otherwise the whole JSON value is wrapped as input. Webhook and policy overrides retain
unrelated values; execution timeout and TTL are converted from seconds to milliseconds.
An empty/falsy policy remains unchanged unless an override adds fields. Async submission
prints the raw result or the job ID and resume command. Waiting and streaming reuse the
tested observer, and the wait deadline begins after submission. Human streaming prints
the job ID on stderr before chunks; JSON streaming omits that announcement. Failed reads
or observation deadlines do not resubmit the job.

Load-balancer requests select the metadata base URL or `https://<id>.api.runpod.ai`, trim
joining slashes and preserve the requested query. Method defaults to POST with a body and
GET otherwise; an explicit method is uppercased. Requests use Bearer credentials and JSON
Accept, with Content-Type only when a JSON body exists. The default deadline is 600 seconds.
Responses preserve structured JSON, plain JSON strings and non-JSON text. JSON mode quotes
plain text; error messages include method, URL, status and trimmed raw response text,
including 401 errors. Queue-only flags are rejected for load-balancer endpoints.

Native adaptations retain owned environment/profile credentials, no implicit redirects or
request replay, total HTTP deadlines and UTF-8 input files. Durations use checked `u64`
milliseconds; nonfinite CLI timeouts and nonstandard JSON numbers are rejected. Truthy
policies must be JSON objects; Python's coercion of pair lists or other non-record values
into dictionaries is not emulated. Native parser, filesystem and transport diagnostics
replace Python exception spelling. Non-object endpoint metadata fails before submission.

`tools/endpoint_run_fixtures.py` runs **22 unchanged parameterized upstream cases** and
captures **51 actual invocations** across sixteen groups, including envelope/policy edges,
encoded endpoint IDs, URL joining, HTTP errors and load-balancer response shapes. Process
tests compare the complete control/data-plane request sequence, payloads, auth/content
headers, exit codes and outputs. Load-balancer origins are substituted with loopback URLs;
a separate native test compares original and fallback URL construction to the same Python
fixtures. Additional process checks verify preflight ordering, request-bounded observation
after delayed submission, and rejection of malformed metadata before any mutation.
The ledger adds three adapted symbols and eleven ported source test functions. The SDK
differential total is **2,106**. Further run timeout/interrupt fault-injection source tests
remain explicitly pending. See [reviews/endpoint-run.md](reviews/endpoint-run.md).

## Native pod and endpoint log commands

`rp pod logs POD_ID` and `rp endpoint logs ENDPOINT_ID` share the SDK's SSE reader.
Both support `--follow` / `-f`, `--source container|system`, `--tail 0..5000` (default 100),
`--since` and `--json`. Endpoints additionally accept repeated `--worker` filters. Worker
discovery and stream URLs encode IDs independently, and requests retain Bearer auth.
Without a source filter, the commands request both REST sources rather than HAPI `all`.

Snapshots capture the start instant before worker discovery, collect backfill until the
first event at or after that instant, and end established streams after three idle seconds.
Connection establishment and authentication are not shortened by that idle deadline.
Collected records are stably sorted by their original timestamp strings, as in Python,
even when offsets make that ordering differ from chronological ordering. A failed snapshot
prints no buffered backfill. Human endpoint output pads worker IDs using Unicode character
counts; JSON lines add a worker field, with an event's existing worker field taking priority.
An empty endpoint prints `no workers`, or nothing in JSON mode.

Follow mode emits records immediately, reconnects after one second using the SDK event-ID
observer, and refreshes worker discovery every five seconds. Existing streams continue
during discovery requests. IDs attach once; a completed/disappeared worker can attach again
on the next refresh. Width grows when longer worker IDs appear. Initially empty endpoints
print `waiting for workers ...` in human mode. Endpoint worker 404s end that worker silently;
pod 404s and authentication failures propagate. HTTP 429/5xx, connection/request failures
and timeouts retry only in follow mode, including a disconnect before headers. Payload/
decoder errors propagate. Fatal worker errors propagate at the next discovery tick.

Owned streams replace Python tasks: dropping the observer, errors and Ctrl-C cancel the
active HTTP bodies without detached work. Ctrl-C returns success, without job-resume text.
Native snapshots parse RFC3339 timestamps with explicit offsets. Python's additional ISO
date forms and malformed non-string source/line fields are not emulated. Native transport
errors retain status and causes rather than aiohttp text. JSON lines use compact UTF-8
serialization, preserving values and field order instead of Python's spaces/ASCII escapes.
The public SDK API and dependencies are unchanged. Pod resource commands are covered by
the later milestone below; SSH connect/sync and completion remain pending.

`tools/cli_log_fixtures.py` runs **39 unchanged parameterized source cases** across shared
log behavior and both command routers, then captures **14 deterministic snapshots**. Native
processes compare their requests, output and exit status against those captures. Named unit
counterparts cover all eleven shared-log source functions and both option routers. Virtual
time keeps discovery/retry tests deterministic; real TCP/SSE checks verify cursor reuse,
HTTP failures and closure on cancellation, idle timeout and the snapshot cutoff. The ledger
adds four adapted symbols and fourteen ported source tests, bringing the SDK differential
total to **2,120**. See [reviews/cli-logs.md](reviews/cli-logs.md).
The final local suite passes **818 tests**, zero failures and three ignored child entry
points exercised by parent tests. Build, strict Clippy, formatting and rustdoc pass.

## Native pod resource commands

`rp pod` and `runpod pod` support listing, inspection, creation, updates, start/stop/restart,
termination (`delete` alias) and logs. Missing IDs open a terminal picker; action pickers
offer only pods listing that action. Missing IDs in noninteractive mode fail before
credential lookup. Creation can prompt for a missing name after validating hardware flags.
The log command now shares optional-ID resolution, fixing the initially required pod ID.
SSH connect/sync and shell completion remain pending.

Listing paginates, optionally includes cluster pods, filters names by case-insensitive
substring, and filters the displayed status. A RUNNING pod without nonzero uptime is
displayed as `starting`, while JSON retains the original record. Human inspection includes
resources, volume mounts, lifecycle timestamps, exposed URLs, SSH/TCP connection details
and environment variable names without their values. Native rendering keeps field order
and uses the existing table/field helpers.

Creation reuses native container, registry, region, CUDA and catalog resolution. GPU
selection preserves the source's stock/price/name ordering and cloud/count eligibility;
a unique match bypasses the multi-match eligibility filter. Stock is restricted to the
requested placement when present. Notices and no-stock/SSH-key warnings go to stderr in
JSON mode. CPU requests use the selected flavor's minimum vCPU count when omitted.
Regions merge with explicit data centers in order; a network volume supplies placement
only when neither provided IDs. SSH setup checks keys through REST, with no GraphQL call.

Updates read existing state only for environment or mount edits. Environment assignments
merge before removals; ports replace the list; omitted mount paths preserve the current
path. Persistent/network mounts default to `/workspace`; global paths remain absent when
unspecified. Empty registry/CMD/argument options preserve clearing behavior, and explicit
false lock/networking flags are retained. An explicitly empty name still counts as an
update request but is omitted from the PATCH body, matching Python. Malformed truthy
non-object current settings fail before PATCH; source exceptions become typed errors.

Create/start/restart observation polls every five seconds with a 600-second wait deadline.
The action start instant is captured before POST; the wait deadline starts afterward.
Restart captures the prior record and requires a changed start timestamp plus evidence
of reset runtime (a non-running observation or reduced uptime). The ready uptime must fit
the time since the action plus one second. Readiness wins over an expired deadline on the
same response. Error/terminated states fail immediately; other states eventually time out.
Ctrl-C aborts observation with exit 1 without repeating or undoing the action. Async owned
requests replace blocking sleep/HTTP; animated Rich status rendering remains pending with
the console UI work.

Termination fetches the name before confirmation and deletes only after approval. Lookup
failure or rejection prevents DELETE. `--yes` bypasses lookup and confirmation. Native
resource IDs are encoded as path segments; ordinary Runpod IDs retain their source routes.
Credentials remain owned environment/profile configuration. Counts/sizes use `u64`, and
mount sizes accept ASCII decimal digits rather than Python's wider Unicode integer forms.
Human uptime uses nonnegative integral seconds. Native parser/transport/terminal diagnostics
and plain warning rendering replace Click/Rich styling. Tests compare request payloads,
JSON values, error classes/exit codes and read-before-write ordering.

`tools/pod_cli_fixtures.py` runs **82 unchanged parameterized Python cases** and captures
**89 invocations across 48 source functions**. It also verifies three malformed-state
cases stop before PATCH. Native named counterparts replay every capture with controlled
interaction and time; **71 noninteractive cases** additionally execute real binaries.
The source's injected negative wait timeout becomes a zero `Duration` in native tests.
Windows PTY checks cover name prompting, log selection, declined termination and Ctrl-C
during restart observation. The ledger adds 22 adapted symbols and 48 ported tests; SDK
differential captures total **2,209**. See [reviews/pod-cli.md](reviews/pod-cli.md).
The final local suite passes **872 tests**, zero failures and three ignored child entry
points exercised by parent tests. Build, strict Clippy, formatting and rustdoc pass.

## Native registry and secret commands

The `rp registry` and `rp secret` groups implement `add`, `list` and `delete`. They reuse
`AppsApiClient` directly: registry operations use REST and secrets retain the source
GraphQL contracts. Input prompts and deletion confirmation happen before credential
loading or API requests. Missing names/usernames prompt normally; passwords and secret
values use the existing console library's raw key reader without echo. A small input
loop handles Unicode backspace, nonempty submission and cancellation. The higher-level
password prompt terminated the Windows process on Ctrl-C; raw keys allow a clean
`Aborted!` exit 1 before any request. No dependency or public SDK API was added.

Lists sort names case-sensitively and stably. Registry output includes IDs; secret output
includes descriptions, with names padded to the longest Unicode character count. Values
are never displayed. Deletion looks up an exact name and deletes the first matching ID;
it does not accept IDs as an alternative or reject duplicate names. Registry IDs are URL
encoded by the existing transport. Explicit empty options are sent unchanged and do not
trigger prompts. Values beginning with dashes are accepted, and repeated scalar options
use the last value, as in Click. Creation responses are ignored. The source's successful
message after a false `secretDelete` response is also preserved.

Native adaptations: text is unstyled and unwrapped, markup-like names are literal,
metadata descriptions are strings, parser/transport failures have typed diagnostics,
and terminal interaction uses dialoguer. Piped input is not an interactive terminal:
scripts supply fields explicitly and use `--yes`, receiving usage exit 2 for omitted
input/confirmation before credentials are read. An interactive refusal exits 1 without
any request. Python Click accepts piped prompt input; this remains an explicit CLI
adaptation, consistent with the existing resource commands.

`tools/credential_cli_fixtures.py` runs all **10 unchanged upstream registry/secret tests**
and **34 additional source characterizations**. It executes the actual Python SDK methods
against stubbed transports to capture request bodies and response envelopes. The resulting
44 fixtures regenerate byte-identically. Ten named Rust counterparts and one edge-case
test replay every fixture; two executable tests replay 31 noninteractive cases through
both aliases and verify missing inputs fail before credentials. Real Windows terminal
checks verify hidden password/value entry, backspace, and cancellation before any request. All requests
use loopback servers and synthetic values.

The ledger adds **8 adapted API symbols and 10 ported test functions**, bringing mapped
API coverage to **804/1,135 (70.8%)** and test mappings to **902/1,659**. SDK differential
fixtures total **2,253**. See [reviews/credential-cli.md](reviews/credential-cli.md).
The final Windows suite passes **887 tests**, zero failures and three ignored child
entry points exercised by parents. Locked build, formatting, strict all-target/all-feature
Clippy and rustdoc pass. Pedantic review leaves only test-harness length advisories in
the new code; string assembly and assertion advisories were fixed.

## Native login command

`rp login` / `runpod login` supports direct `--api-key`, browser approval with optional
`--no-open`, named `--profile`, and `--agent --email` authentication. It reuses native
anonymous browser/email transports and `CredentialStore` atomic writes. No Python process
or credentials from the existing account are involved in authentication requests.

Conflicting options fail before authentication or storage changes, without echoing supplied
keys. Presence of `--api-key` conflicts with `--agent` even when its value is empty; an
empty key without `--agent` instead selects browser login. Whitespace-only direct keys,
empty profile names and repeated options retain Python behavior. Email is trimmed; codes
are trimmed and must be six ASCII digits. Local format errors do not submit a request.
An API rejection prompts again within the same attempt and proof-of-possession verifier,
prints the rejection notice once per retry, and does not repeat the initial email notice.

Success writes only the selected profile with overwrite enabled. Authentication errors,
EOF, Ctrl-C, malformed existing TOML and write failures do not print success or replace
existing credentials. Native Unix writes retain mode 0600 through the existing temporary
file implementation. Windows uses native file permissions instead of Unix mode semantics.
Browser approval displays its URL before polling. Default launching uses `rundll32` on
Windows, `open` on macOS and `xdg-open` elsewhere, with separate arguments and no shell
command interpolation; launcher failures are ignored like Click's return status. The
printed URL remains available. `--no-open` does not start a launcher. Ctrl-C during network
waiting or hidden code entry exits 1 with `Aborted!` and leaves the credential file intact.

Native adaptations: email login requires a real terminal instead of accepting Click's
piped prompt input, terminal styling/spinners and EOF blank lines use native rendering,
and filesystem/transport errors retain native diagnostic types. Platforms without the
standard desktop opener can use the printed URL; Python's additional desktop/browser
fallbacks are not emulated. Rich's progress UI remains a separate pending console item.

The fixture generator executes **18 unchanged source cases across 11 functions**. On
Windows, **17 pass** and one reaches a final Unix permission assertion that reports 0666
instead of 0600 after all its behavior assertions pass. The generator accepts only that
exact known failure; any additional failure stops generation. That test is mapped as a
platform adaptation, and its native counterpart checks mode 0600 on Unix. Eleven further
source characterizations bring the fixture set to **29**, regenerated byte-identically.
Eleven named native counterparts plus a characterization test check prompts, warnings,
browser callbacks, auth calls, stored profiles and failure paths. Three process tests use
both executable aliases and real loopback transports. Windows terminal checks verify
hidden code validation/retry with PKCE, selected-profile storage, and interruption of email
and browser login. All auth traffic uses local mocks and synthetic keys; existing user
credential files are never accessed by the fixture generator or process tests.

The ledger now maps **805/1,135 API symbols (70.9%)** and **913/1,659 test functions**.
SDK differential fixtures total **2,282**. See [reviews/login-cli.md](reviews/login-cli.md).
The Windows suite passes **902 tests**, zero failures and three ignored child entry
points exercised by parents. Formatting, locked build, strict all-target/all-feature
Clippy and rustdoc pass. Pedantic review is recorded with the native review report.

## Native app management commands

`rp flash app` and `runpod flash app` provide `list`, `get APP_NAME`, and
`delete APP_NAME [--yes/-y]`. They use the existing native `apps::manage` lifecycle.
Lists sort names stably and show environment names in server order, with `-` for none.
Inspection retains environment order and truncates active build IDs to twelve Unicode
characters, with `no build` for absent or falsey IDs. Empty lists, spacing and final
success/failure messages match the captured source output.

Deletion confirms before loading credentials or making requests. It attempts later
endpoints after a deletion failure, retains an environment if one of its endpoints
failed, and attempts subsequent environments. Any collected failure retains the app.
Lookup failures stop immediately. Failures print in order and return exit 1 with
`undeploy incomplete; app kept`. As in Python, non-raising false mutation confirmations
count as completed deletions; the CLI does not silently reinterpret that source quirk.
Ctrl-C drops the operation and stops later requests, returning `Aborted!` and exit 1.
Completed or already submitted remote mutations are not rolled back.

Native adaptations: confirmations use the existing terminal interaction layer and
require `--yes` in a noninteractive shell. Cleanup events print plain progress lines
instead of Rich spinners; the separate console classes remain pending. Output is plain
text without Rich markup interpretation or automatic wrapping. Malformed response
metadata returns typed errors rather than Python indexing/type exceptions. A private
management boundary permits source-test replay; no public SDK API or dependency is added.

`python tools/app_cli_fixtures.py` runs all **seven unchanged source tests** and records
**15 transport characterizations**, yielding **22 deterministic fixtures**. Each original
test has a named Rust counterpart. Process tests replay the complete request sequence
through both executables, including Unicode build IDs, absent apps, lookup errors,
multiple endpoint failures, environment/app failures and false deletion confirmations.
Windows terminal checks exercise accepted/declined confirmation and interruption during
an in-flight deletion: declining sends no requests and interruption sends no later
deletions. Tests use synthetic keys and loopback servers only.

The ledger maps **809/1,135 API symbols (71.3%)** and **920/1,659 test functions**.
SDK differential fixtures total **2,304**. The broader `flash` group, discovery and
deployment commands remain pending. See [reviews/app-cli.md](reviews/app-cli.md).
The Windows suite passes **912 tests**, with no failures and three ignored child
entry points exercised by parents. Formatting, locked build, strict Clippy and rustdoc
pass; test-only lint fixes were followed by focused tests and strict Clippy.

## Native environments and compiled discovery

`rp flash env` / `runpod flash env` support `list`, `get`, `add` and `delete`, with
`--app/-a`; `flash undeploy` accepts `--app/-a`, `--env/-e` (default `default`) and
`--yes/-y`. Both destructive commands delete endpoints and their environment while
retaining the app and build history. Endpoint failures are collected in order and retain
the environment. False mutation confirmations preserve the source's success behavior.
Declined confirmation exits 1; noninteractive confirmation requires `--yes` and exits 2
before credentials or network requests. `env delete` and `undeploy` retain their distinct
source confirmation, success and incomplete-cleanup messages.

Environment and endpoint order is preserved. Build IDs truncate at twelve Unicode
characters; dates truncate at ten. Falsey missing values use the source `-` fallback.
Endpoint references print their IDs and arrow as plain text; Rich hyperlinks, wrapping,
styles and progress animations remain native rendering adaptations.

An explicit nonempty `--app` bypasses directory lookup and discovery. Otherwise, the CLI
requires exactly one unique app name from a Cargo project. `apps::discovery::discover_apps`
uses versioned Cargo metadata and compiler-artifact output to locate and build binary
targets, then launches each executable with a private declaration-export path. The
project registers apps and calls `export_if_requested` before local entrypoints; see
`examples/discovery.rs`. App definitions are validated before any target's exports are
accepted. A failed process, missing export or invalid declaration cannot return a partial
app. Directory scans tolerate failed targets when another exports apps; individual source
or manifest targets are strict. Targets are sorted by source path, and first-name wins
deduplication preserves declaration order. No shared import registry or parent environment
is modified. A process discovery guard overrides even explicit app routing settings.

Cargo's normal build directory and freshness rules are used. The caller needs a host
toolchain and host-runnable binaries. Cancellation terminates and waits for the active
child, rejects partial exports and stops later targets. Dropping the async future uses
Tokio's kill-on-drop behavior for the direct child. The SDK does not promise control of
arbitrary subprocess trees created by user declarations or build scripts.

**This is metadata discovery, not the completed port of Python's live discovery API.**
It returns `DiscoveredApp` definitions and executable/source/project paths, not imported
live callable handles. Source locations identify the Cargo binary entry; declarations
may reside in other compiled modules. Python module identity, package rollback, arbitrary
module-file imports and forced cache invalidation (including preserved-mtime edits) are
not claimed. The five discovery implementation entries and all five upstream discovery
test functions remain pending. The error class alone maps to the native typed enum.

`tools/env_cli_fixtures.py` passes eight unchanged source tests and records sixteen wire
characterizations: **24 deterministic fixtures** replayed by named unit tests and both
executables. Real Cargo test projects verify unique/ambiguous/empty app resolution and
explicit-name bypass. Native discovery tests cover failed/invalid exports, retries,
deduplication, interruption, guard enforcement and process isolation. A real consumer
project using this SDK passed Cargo build, `AppRegistry` export, CLI resolution and a
loopback environment query without running its local entrypoint. Windows terminal
interruption rejected an in-flight partial export and never ran the later binary.

`tools/discovery_fixtures.py` passes all six parameterized Python discovery cases and
records nine reference observations. These are not counted as Rust differential fixtures
or mapped source tests while live-handle semantics remain pending. Cargo integration
follows the [metadata format](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
and [artifact messages](https://doc.rust-lang.org/cargo/reference/external-tools.html#artifact-messages).

The ledger maps **818/1,135 API symbols (72.1%)** and **928/1,659 test functions**.
SDK differential fixtures total **2,328**. See
[reviews/environment-discovery.md](reviews/environment-discovery.md).
The complete Windows suite passes **930 tests**, with no failures and four ignored child
entry points exercised by parents. Formatting, locked build, strict Clippy and rustdoc
pass; focused tests and strict Clippy were repeated after review fixes.

## Native deployed queue adapter

The separately maintained worker runtime at commit
`14f70227008c1ec317e3fe1865a4688b14f5d432` now has a native counterpart for its
deployed function adapter. `FunctionHandle::queue_worker(WorkerConfig)` snapshots the
registered body and init hook, finishes initialization, and returns an initialized
`serverless::Worker`. No intake or heartbeat occurs during initialization. Reusing that
worker does not rerun init; constructing another worker initializes again. Native
initialization errors preserve their typed cause, and cancelled async initialization
drops its owned future without starting a worker.

`FunctionHandle::start_queue_worker` runs init before parsing core configuration, then
uses the existing production/local/realtime startup path. `RUNPOD_MAX_CONCURRENCY`
defaults to one, floors at one, and falls back to one for invalid strings. Values above
one install a constant concurrency modifier, leaving the initial core capacity unchanged.
Explicit `queue_worker` configuration controls its own concurrency and reads no environment.
Both paths force aggregation for streaming bodies while preserving other caller settings.

| Runtime Python function | Native counterpart |
| --- | --- |
| `_job_kwargs` | `apps::worker::job_kwargs` |
| `_make_deployed_handler` | `apps::worker::deployed_handler` |
| `_run_init` | initialization before `FunctionHandle::queue_worker` / `start_queue_worker` |
| `_max_concurrency`, `_worker_config` | `apps::worker::max_concurrency`, `config_from_values`, aggregation in preparation/startup |
| deployed serving portion of `main` | `FunctionHandle::start_queue_worker`; resource loading/bootstrap remains pending |

Preserved behavior: job input becomes keyword arguments with no positional arguments;
missing/null/falsey input becomes an empty map; `__empty` is removed regardless of its
value without changing the remaining keyword order. Sync and async single-result bodies return raw values. Streams remain incremental,
send each chunk through the existing worker transport, and finish with an aggregate.
An empty stream produces an empty aggregate. Errors after partial output keep already
sent chunks and finish with the core's failure response. Returned `error` fields retain
the existing core's interpretation rather than introducing a function-response envelope.

Native adaptations: owned compiled functions replace interpreter callables. Synchronous
bodies and init hooks use Tokio's blocking pool; already-running blocking calls cannot
be cancelled by dropping their future. Async functions/streams stay on the caller's
runtime. Native streaming init hooks follow `NativeFunction`'s collection semantics;
Python's `_run_init` does not iterate a returned generator. Truthy job input must be a
JSON object: Python's incidental `dict()` coercion of arrays of pairs is not emulated.
JSON values retain serde_json's numeric domain. Concurrency uses trimmed Rust `usize`
parsing; Python digit underscores, Unicode decimal digits and unbounded positive integers
are not supported and use the invalid-value fallback. Native errors use the existing
typed error/core traceback adaptation. Native argument validation belongs to the closure
or its typed decoder; Python function-signature reflection is not introduced.

`tools/apps_queue_fixtures.py ../assets/sdk-runtime` records **35 deterministic runtime
cases** (22 calls, nine concurrency inputs and four init cases). Native tests compare
outputs, streaming classification, failure occurrence, parsed limits, modifier presence,
and init effects. Error-class/message spelling is a native adaptation. A child process
proves that the environment-installed modifier actually runs three jobs concurrently,
with exact partial/final webhook bodies. Additional tests cover initialization gating,
typed failure, cancellation, ownership after handle drop, local startup and incremental
delivery before a generator resumes. Seventeen unchanged source tests pass and each has
a named native counterpart. See [parity/runtime-queue.json](parity/runtime-queue.json)
for the mapping and [reviews/apps-queue-worker.md](reviews/apps-queue-worker.md) for review.

These runtime contracts are separate from the pinned Python SDK inventory, so SDK
coverage remains **818/1,135 symbols (72.1%)**, **928/1,659 upstream test functions** and
**2,328 SDK differential fixtures**. Native binary packaging/deployment and the queue
live-source executor remain pending. Manifest selection and mount configuration are
covered by the later deployed resource loading milestone.

Validation: the complete Windows suite passes **958 tests**, with zero failures and four
ignored child entry points exercised by parents. Locked build, formatting, strict
all-target/all-feature Clippy and rustdoc pass. Review found and fixed a keyword-order
regression: serde_json's ordinary removal swaps the last entry into the removed position;
`shift_remove` now matches Python's order-preserving `pop`. The new source fixture failed
before that fix and passes afterward. Focused unit/integration/process tests and strict
Clippy were repeated after review fixes.

## Artifact bootstrap location and attachment

`apps::bootstrap` ports the artifact-loading phases from sdk-runtime commit
`14f70227008c1ec317e3fe1865a4688b14f5d432`. The native package format can now be read back
without a Python interpreter. This is a prerequisite for full native deployment startup;
it does not claim the remaining bootstrap orchestration or native artifact launch.

| Runtime Python function | Rust counterpart |
| --- | --- |
| `PhaseError` | `apps::bootstrap::PhaseError`, wrapped by `Error::Bootstrap` |
| `_error_payload` | `PhaseError::payload` |
| artifact path/wait globals, `_locate` | owned `BootstrapConfig`, `from_env`/`from_values`, `locate` |
| `_attach` | `dependency_paths` |
| `_manifest` | `read_manifest` |

Prebuilt directories take priority over markers and artifacts. A marker is trusted solely
because it is a file; its contents, age and artifact version are not revalidated. Missing
artifacts are polled every two seconds, with existence checked before the deadline. The
returned path retains the configured spelling. Extraction keeps archive order, later
duplicate files replace earlier files, and a later failure leaves already-extracted files
in place. A successful extraction writes the timestamp marker. A marker directory is not
a warm marker and causes the final marker write to fail.

One source quirk is deliberately preserved: an artifact member named `.rp-unpacked` can
leave a marker behind even when a later member fails, so a subsequent boot may trust that
partial tree. The capture suite records this behavior instead of silently changing it.
Manifest loading accepts every valid JSON value, including null, arrays and scalars;
resource shape validation belongs to dispatch. Dependency paths place an existing `env`
directory before the source tree and do not mutate process search paths.

Tar, gzip, bzip2 and xz are detected from bytes, independently of the filename. The latter
two codecs were already locked/transitive ZIP dependencies; exposing them directly adds
no resolved package or version. Native artifacts accept regular files/directories and
GNU/PAX long names. Relative names are checked before extraction; existing destination
ancestors must resolve inside the app directory. Links, special members, absolute paths,
parent traversal, backslashes and colon-containing paths are rejected. This uses the
link-free portable format produced by the SDK packager. Python 3.12+ data filtering can
accept some safe links and strip leading slashes; those incidental tar inputs are a
documented native restriction, not claimed as equivalent. File extraction cannot silently
skip a regular member named `.` and then mark the tree complete.

Unix file modes follow the source data filter: high privilege bits and group/other write
bits are removed; owner read/write is retained; executable bits require owner execute.
Directory modes/ownership are not applied. Windows files are writable after extraction.
Filesystem metadata/errors use native platform behavior. Blocking extraction is not
cancelled by dropping a task waiting for its result, and callers must not extract
concurrently into the same tree.

Native differences: bootstrap paths and waits are owned values rather than import-time
globals. Wait parsing uses trimmed signed 64-bit Rust integers; negative waits become
zero-duration waits (the timeout diagnostic consequently says `0s`). Python underscore
digits, Unicode digits and unbounded integers are not emulated. Native EOF and marker/
directory filesystem failures are wrapped in locate-phase errors with their typed cause;
the source lets some of those exceptions escape its phase wrapper. Interpreter-specific
exception spelling and logging are not compared as identical. Phase identifiers remain
strings, including caller-defined phases, and structured error payloads match exactly.

Ten unchanged source tests pass. `tools/bootstrap_fixtures.py ../assets/sdk-runtime`
produces **33 deterministic cases**: twenty-two location cases, seven manifest values and
four error payloads. Native tests compare success/failure, source phase when present,
paths, extracted contents and marker state. Two cases record source exceptions without
a phase; the native typed wrapping is explicit. Further tests round-trip an actual SDK
artifact with source and dependency files, wait for a platform-published artifact, and
check malformed/native-only archive members. Unix-only tests verify permission filtering
and rejection of an existing symlink pointing outside the extraction root. See
[parity/runtime-bootstrap.json](parity/runtime-bootstrap.json) and
[reviews/apps-bootstrap.md](reviews/apps-bootstrap.md).

This separate runtime coverage does not change the SDK inventory: **818/1,135 symbols
(72.1%)**, **928/1,659 source test functions**, and **2,328 SDK differential fixtures**.
Native binary packaging/launch, system dependency installation and bootstrap error-serving
orchestration remain pending. Manifest selection and mount configuration are covered below.

The complete Windows suite passes **974 tests**, with zero failures and four ignored child
entry points exercised by their parents. Formatting, locked build, strict Clippy and
rustdoc pass. Focused bootstrap tests and strict Clippy were repeated after review changes.
Unix-only extraction tests run in CI; local Windows checks do not prove Unix permissions.

## Deployed native resource loading

`apps::runtime::DeployedContext` ports deployed selection and pre-import mount setup from
sdk-runtime `14f70227008c1ec317e3fe1865a4688b14f5d432`. It captures only worker settings,
never changes process environment, and validates mounts before calling native declarations.

| Runtime Python function | Rust counterpart |
| --- | --- |
| queue/API `_resource_name`, `_is_deployed` | `resource_name`, `is_deployed` |
| `mounts.configure_mounts` | `DeployedContext::from_env` / `from_values`, `mounts` |
| queue `_load_deployed_handle` | `load_queue` |
| API `_load_api_handle` | `load_api` |
| bootstrap `_validate_task_environment`, `_task_resource` | task validation inside `load_task` |
| task `deployed.load_handle`, `_initialize` | `load_task` and native handle initialization |

A compiled-module callback receives the selected module name and validated `MountBindings`,
then returns an `AppRegistry` containing that module's exposed declarations. Applications
and resources retain registration order. The callback replaces Python import/reflection;
native tasks use explicit `SymbolReference` metadata, including dotted qualified names.
Callbacks must finish registration before returning and expose only the selected module's
resources. No Python interpreter, mutable global mount table, or dynamic Rust code loading
is involved. Captured binding clones keep paths usable after the context is dropped.

Preserved quirks: a nonempty Flash name takes precedence over the Runpod fallback, including
whitespace. Deployment detection checks only name and manifest-file existence. Queue/API
selection takes the first matching manifest entry and the first matching handle. Queue
loading also accepts task handles and ignores manifest kind, app owner and qualified name.
Neither queue nor API loading initializes the handle; worker/HTTP startup does that.

Tasks require runtime kind `task` and four nonblank identity fields before manifest access.
Manifest app, unique matching task entry and nonblank module/qualified name are validated
before the callback. Exactly one task handle must match name, kind, owning app and qualified
name. Aliases of one application instance deduplicate; separate instances with identical
names are ambiguous. No hook runs after missing/ambiguous selection. Task initialization
is awaited on the caller's Tokio runtime and its typed failure propagates unchanged.
The subsequent task runner milestone covers deployed HTTP serving, timeout policy and
pod-deletion acknowledgement.

Missing `RUNPOD_MOUNTS` means an empty array, while an explicitly empty value fails JSON
parsing. Malformed JSON retains its native cause. Record/path validation and reference/ID
lookup match source diagnostics; duplicate identical paths deduplicate and distinct paths
are ambiguous. The older standalone `MountBindings::from_env` keeps its existing treatment
of an empty value; deployed runtime configuration uses the stricter source contract.

Native differences: worker paths remain POSIX on all hosts. The source's `pathlib.Path`
renders backslashes on Windows; two unchanged source integration tests consequently fail
only their expected `/shared` or `/data` output. Native integration tests preserve their
POSIX worker expectations. Missing-resource available names use JSON spelling rather than
Python repr. Malformed queue/API manifest shapes use typed native diagnostics, and file/JSON
failures retain the bootstrap reader's attach-phase wrapper. Non-Unicode environment values
are treated as absent; Python surrogate-escape environment behavior is not emulated.

Twenty-one unchanged source cases (thirteen test functions) pass. The two Windows path
failures above are recorded separately, not counted as source passes. The fifteen mapped
test functions and additional native checks are listed in
[parity/runtime-loading.json](parity/runtime-loading.json). The deterministic capture has
**57 cases**: 34 selections, 13 mount configurations and 10 deployment detections. Tests
compare selected values, error phase/message, module calls and init events. Queue startup
uses real loopback transport to verify both stream chunks and the aggregate; API startup
uses real HTTP to verify readiness, initialized routes and mounted paths. Async task init
gates loading and retains usable state; native registration/init IO failures keep their kind.

This runtime milestone does not inflate SDK inventory coverage: **818/1,135 symbols (72.1%)**,
**928/1,659 source test functions**, and **2,328 SDK differential fixtures**. See the
[Rust review](reviews/apps-runtime-loading.md) for final validation results.

## Deployed task HTTP runner

`apps::task_runner` ports deployed task execution from sdk-runtime
`14f70227008c1ec317e3fe1865a4688b14f5d432`. `TaskRunner` owns token, policy and state;
`serve` accepts a listener, task initializer, deletion callback and shutdown future.
The process-level `start` reads worker settings, binds port 8080 by default, loads the
pod-scoped deletion client and exits only after acknowledged deletion. It supports the
compiled manifest loader; native artifact build/launch orchestration remains pending.

| Source runtime contract | Native counterpart |
| --- | --- |
| `Handler.do_GET`, `do_POST`, auth/readiness/request helpers | task runner HTTP dispatch |
| `deployed.validate_request`, `deployed.execute` | strict JSON validation and registered native execution |
| `_configured_timeout`, initialization/readiness in `main` | `TaskRunner::from_env/from_values`, `serve` |
| `_set_timeout`, `_start_execution`, `_watchdog`, idle/contact state | owned deadline/contact state and watchdog |
| `_terminate_self` | `PodTermination::terminate` |
| runner process `main` | `task_runner::start` with an explicit native initializer |
| executor stdout tee and prepared-result aggregation | task-scoped `Capture`, native sync/async/stream functions |

Readiness is public; error details require an authenticated route. Authenticated POSTs
touch contact before route/body validation, and `/result` polling touches contact; `/ping`
and unknown GETs do not. Query strings remain part of exact route matching. The runner
rejects source, dependency, mount and serializer overrides, non-object bodies and invalid
args/kwargs. No content-type header is required and an empty body means an empty object.

An accepted execution reserves the sole slot before user code starts. `/submit` returns
RUNNING and retains the eventual response; `/execute` waits for the response, then leaves
DONE with a null `/result` response. While inline work runs, `/result` remains NONE but
further submissions conflict. Client disconnects do not cancel accepted work. Synchronous,
asynchronous and streaming functions execute natively; streams aggregate to one JSON result.
Ordinary `Result` failures use a failure response and retain captured stdout.

Initialization/execution caps must be positive. Invalid configured caps fail readiness
before polling the initializer. An initialization deadline is independent of the execution
cap, which starts only at the first accepted call. Client deadlines accept zero and can
shorten, but never extend or clear, an earlier deadline. Recent contact and active work
prevent idle cleanup but do not defeat an execution deadline. Idle monitoring starts after
initialization succeeds or fails; initialization expiry starts deletion retries directly.
Late initialization cannot restore readiness. Bootstrap errors can be supplied explicitly.

Deletion sends `flashTaskTerminate` plus `X-RunPod-Pod-Id` in task mode and requires exactly
true. The legacy pod variant uses `podTerminate` and requires an explicitly present null.
Both use a Bearer key. Missing credentials make no request. Transport failures, HTTP 429/5xx
and exclusively transient GraphQL error codes retry up to three times, with 0.5/1 second
backoff and five-second request limits. Rejection/malformed responses return false; exhausted
transport failures retain a native error. The watchdog retries unacknowledged deletion later.

Shutdown aborts owned job futures and cancels accepted sockets, including incomplete request
bodies and blocked response writes. This prevents graceful HTTP draining from indefinitely
delaying acknowledged deletion. `serve` returns a typed reason; `start` exits with code zero
after deletion so blocking user work cannot keep a terminated pod process alive. The caller's
Tokio runtime and arbitrary tasks it owns are not globally shut down by the embedded server.
User-created background tasks should use owned cancellation/RAII guards.

Task stdout is unbounded, excludes the SDK stderr writer, flushes writes, and survives a
closed container log. Synchronous NativeFunction calls propagate the capture into their
blocking thread. Ordinary process `println!` and independently spawned user tasks are not
intercepted; use the SDK writers and explicit Capture scopes. This avoids Python's global
stdout redirection. The existing bounded core-worker capture contract remains unchanged.

Native adaptations: task error messages use typed Rust diagnostics rather than Python
tracebacks. NativeFunction already requires JSON values, so Python non-JSON return objects
have no equivalent. NaN/Infinity and out-of-range JSON numbers are rejected by the parser;
HTTP status is 400, while incidental parser wording differs. Duration/Instant limits apply
instead of unbounded floating-point deadlines; exotic numeric spellings, nonfinite idle
timeouts and Python's huge finite durations are not emulated. Unsupported HTTP methods use
a native JSON 501 response. Native HTTP framing/header behavior and JSON whitespace differ.
The deployed runner always uses registered native code; live-source task mode is still pending.

The unchanged source suites passed **144 cases** locally. One test has the known Windows
`pathlib` slash-rendering mismatch; the source archive/bootstrap subprocess test exits with
code zero immediately after extraction, before its listener opens. Neither is counted as a
pass. The native counterpart verifies artifact extraction, manifest selection, mount binding,
HTTP execution, exact deletion request and process exit, including blocking user code.

There are **48 HTTP captures**, compared by status, JSON body, contact and deadline state.
The capture harness waits for each source request to finish before resetting global state;
this fixes a regeneration race after inline responses. Additional native tests cover concurrent
submission, disconnected clients, initialization expiry, failed-deletion retries, stdout and
socket cancellation. See [parity/runtime-task-runner.json](parity/runtime-task-runner.json) and
[reviews/apps-task-runner.md](reviews/apps-task-runner.md) for mappings and validation.

Runtime coverage remains separate: SDK inventory stays **818/1,135 symbols (72.1%)**,
**928/1,659 source test functions**, and **2,328 SDK differential fixtures**.

## Remaining work

The current milestone does not implement native binary deployment, the Apps HTTP live
dispatcher, full live-handle Apps discovery or the remaining CLI commands. Debugger integration and remaining cache/platform
fault-injection coverage are also pending.
All remain part of the requested full port, not exclusions from its scope.

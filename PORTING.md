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
| `worker_state.Job`, `JobsProgress` | `WorkerJob`, `JobsProgress` |
| `rp_validator.validate` | `utils::validator::validate` with typed ordered rules |
| `rp_model_cache` path helper, `rp_cleanup.clean`, CUDA availability | `utils` functions |
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

The detailed symbol and test ledgers are [parity/api.json](parity/api.json) and
[parity/tests.json](parity/tests.json). Pending entries must not be interpreted as
implemented merely because a broad Rust module exists.
The ledger labels serialization/reflection and runtime-lifecycle adaptations explicitly.
This milestone maps 376 implemented symbols, 88 adapted symbols, and 352 upstream
test functions; other entries remain pending.

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
  not converted into Python exceptions. Output delivery failures propagate to callers.
- The worker loop currently has a fixed concurrency limit. Dynamic scaling, full fitness
  checks, stdout capture/debugging, hosted HTTP API, CLI arguments, and the advanced
  volume/download/upload helpers remain pending. The `start` convenience function uses
  `test_input.json` locally and Ctrl-C for platform shutdown; SIGTERM handling is pending.
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
  `call_handler` expose registered bodies to a future HTTP router; server hosting and response
  binding remain pending. ASGI factories are explicit Python-worker metadata and cannot be
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

This file will be updated as each remaining component and test is ported.

## Remaining work

The current milestone does not implement native binary deployment, HTTP server integration,
discovery and the CLI. Worker fitness, dynamic scaling, hosted API,
capture/debugging, downloads/uploads/SSRF, and volume-cache operations are also pending.
All remain part of the requested full port, not exclusions from its scope.

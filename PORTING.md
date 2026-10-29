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

The detailed symbol and test ledgers are [parity/api.json](parity/api.json) and
[parity/tests.json](parity/tests.json). Pending entries must not be interpreted as
implemented merely because a broad Rust module exists.
The ledger labels serialization/reflection and runtime-lifecycle adaptations explicitly.
This milestone maps 318 implemented symbols, nine adapted symbols, and 215 upstream
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
from pinned source by reproducible tools; no Python runs inside the Rust library.

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
cargo test --test apps_protocol
python tools/verify_pickle.py
```

## Preserved quirks

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
  this does not yet provide native Rust function deployment. Native build/worker
  registration remains pending. JSON argument types enforce the queue boundary.
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
  module/qualified-name metadata. This does not yet implement App registration or source
  discovery. `DeploymentConfig::from_client` supplies worker credentials from the owned
  client instead of Python globals; its Debug output hides credentials and override values.

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

This file will be updated as each remaining component and test is ported.

## Remaining work

The current milestone does not implement deployment orchestration, native function
registration/deployment, artifact construction, discovery, development sessions, browser auth flows,
management and the CLI. Worker fitness, dynamic scaling, hosted API,
capture/debugging, downloads/uploads/SSRF, and volume-cache operations are also pending.
All remain part of the requested full port, not exclusions from its scope.

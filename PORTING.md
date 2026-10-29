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

The detailed symbol and test ledgers are [parity/api.json](parity/api.json) and
[parity/tests.json](parity/tests.json). Pending entries must not be interpreted as
implemented merely because a broad Rust module exists.

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
```

## Preserved quirks

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

## Deliberate language/runtime differences

- Configuration is owned per client. Rust does not mutate module globals or execute
  fitness checks at import time. No Python interpreter is embedded or launched.
- API results remain JSON values to preserve unknown server fields. Options use typed
  structs. Timeouts use Duration; sizes/counts use bounded unsigned integers.
- Job wait deadlines use monotonic time including HTTP time, instead of Python's
  decrementing integer counter. Network errors are returned through Result.
- Transport errors for non-401 GraphQL HTTP failures are reported before JSON parsing.
  Malformed responses return errors rather than Python KeyError/IndexError.
- Redirects are disabled, and mutations are never implicitly replayed after ambiguous
  failures. Pagination alone retries transport failures, matching upstream.
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
  fallback. Automatic live-catalog caching and provisioning are still pending.
- Apps use Rust enum/FromStr validation; some malformed-input error text differs from
  Python repr-based messages. The pinned 24 resource fixtures preserve exact messages.

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

This file will be updated as each remaining component and test is ported.

## Remaining work

The Apps definitions milestone does not implement remote Apps invocation, deployment,
task provisioning/cleanup, build artifacts, discovery, development sessions, auth flows,
management/logging/monitoring, or the CLI. Worker fitness, dynamic scaling, hosted API,
capture/debugging, downloads/uploads/SSRF, and volume-cache operations are also pending.
All remain part of the requested full port, not exclusions from its scope.

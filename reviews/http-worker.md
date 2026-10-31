# Rust review: native HTTP worker hosting

Reviewed with rust-port, rust-best-practices, and rust-review. Scope: Python
`rp_fastapi`, hosted startup, realtime environment selection, callbacks, and HTTP
contract fixtures. Edition 2024 and MSRV 1.94.1 remain unchanged. This milestone
does not cover Apps HTTP hosting, CLI flag parsing, or native binary deployment.

## Must fix

None outstanding. The server owns its handler, startup registry, capture policy,
and HTTP clients. Tracker sharing is explicit through `JobsProgress`; locks are
held only for synchronous snapshots/mutations, never across user code or awaits.
No unsafe code or interpreter dependency was introduced. Startup failures cannot
reach request dispatch. Process entrypoints force-exit after the startup report;
the embedding API returns the original typed prestart cause.

Callbacks use a separate client without worker credentials. Native delivery has
a bounded overall deadline, follows redirects, returns a boolean success value,
and preserves response success when the callback fails. Callback payloads own
snapshots. Heartbeats run during startup and observe the same tracker as handlers.

## Should fix

None outstanding. Strict Clippy identified a large HTTP-response error variant in
the request validator. Validation now returns small structured detail vectors;
the dispatch layer constructs the HTTP response. Pedantic review prompted removal
of an unconditional result clone: callbacks clone payloads only when actually
scheduled, and completed responses move their output value. Integer conversion
is checked rather than casting a signed concurrency value.

The heartbeat test waits for observed traffic within a deadline, avoiding an
assumption that an HTTP request must arrive within a fixed short sleep. Multipart
tests from the previous milestone retain their request-order fix.

## Consider

- Native Tokio tasks, explicitly shared state, typed errors/backtraces, explicit
  capture writers, and unified native streaming replace Python runtime objects.
  These differences are recorded in PORTING.md and adapted ledger entries.
- JSON syntax error wording/positions come from serde_json. Captured field/type
  validation errors match Pydantic. Rust's numeric and Unicode domains remain
  finite native types.
- Shutdown drains active HTTP connections. Detached callbacks can continue while
  their Tokio runtime is alive. Pending simulation jobs remain in an explicitly
  shared tracker after listener shutdown.
- Native hosting supports one process, with concurrent requests through Tokio.
  Python also rejects launching multiple Uvicorn processes from its app object.
  Hooks are rejected in realtime mode. CLI mode precedence is still pending.
- The large route dispatch match mirrors the small source endpoint set. Remaining
  pedantic suggestions concern optional match/let-else style and test length;
  they do not change protocol behavior. No lint suppression was introduced.

## What's already good

- Tests preceded implementation; the expected missing-API failure is retained.
- **85 Python scenarios** are replayed over real local TCP, comparing status,
  JSON bodies, handler arguments, callbacks, and job state after every request.
- Fixtures preserve source quirks: deferred `/run`, retained failed streams,
  omitted response-level nulls, non-string errors producing HTTP 500, and callback
  scheduling before a missing output produces HTTP 500.
- Tests cover prestart gating, heartbeat snapshots, graceful shutdown, startup
  failure/cancellation, large bodies, shared trackers, structured log capture,
  native process exits, and environment-selected realtime hosting.
- OpenAPI route/schema fixtures are generated from the pinned source. Native
  introductory text/version are intentional adaptations. The copied OAuth HTML
  retains FastAPI's MIT license. Interactive docs use the framework's normal CDN
  assets, which the test suite does not fetch.
- Axum's documented [serve lifecycle](https://docs.rs/axum/0.8.9/axum/serve/fn.serve.html)
  supplies listener handling and graceful shutdown. Its usual extractor
  [two-megabyte limit](https://docs.rs/axum/0.8.9/axum/extract/struct.DefaultBodyLimit.html)
  is not introduced: the adapter reads bodies with an explicit unrestricted limit,
  matching the source API. Axum 0.8.9 declares Rust 1.80, below this SDK's MSRV.

## Verification

Windows stable Rust 1.99.1: the full suite passed **490 tests**, with two ignored
child entrypoints invoked by parent tests. After adding the standalone callback
check and applying review fixes, the focused HTTP/process suite passed **13 tests**,
plus its ignored child entrypoint exercised by parent tests. Build, formatting,
strict Clippy across all targets/features, rustdoc, and a pedantic review ran.
The Python HTTP/prestart suites passed **22 tests**. No live Runpod resources or
billable operations were used. Cross-platform and MSRV checks run in GitHub CI.

Logs are retained under ignored `test-results/http-worker-*`. The differential
inventory now contains **1,316 cases**. This milestone maps **18 API symbols** and
**14 adapted upstream tests**; remaining entries retain their pending status.

After validation, scoped `cargo clean --package runpod` removed 2.6 GiB of crate
artifacts while retaining dependency caches. The target directory is **2.23 GiB**,
with incremental compilation disabled. Cleanup was restricted to the resolved
`runpod-rust/target` directory; source files and validation logs were retained.

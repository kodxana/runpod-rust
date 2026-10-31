# Rust review: native Apps class-API HTTP hosting

Applied rust-port, rust-best-practices, and rust-review to the Apps HTTP adapter,
route compiler, lifecycle, shared documentation renderer, and fixtures. Edition
2024 and MSRV 1.94.1 remain unchanged; no new dependency or unsafe code was added.

## Scope and source evidence

The pinned Python SDK declares Apps APIs and makes client calls; serving lives in
the separate [sdk-runtime API server](https://github.com/runpod-workers/sdk-runtime/blob/14f70227008c1ec317e3fe1865a4688b14f5d432/src/runpod_sdk_runtime/api/server.py).
Runtime commit `14f70227008c1ec317e3fe1865a4688b14f5d432` was read and pinned before
implementation. Its MIT notice matches the existing Runpod upstream notice.
The native adapter hosts the existing JSON-valued `NativeFunction` declarations.
This is additional runtime functionality, not an inflated count of newly ported
functions from the Python SDK inventory.

## Must fix

None outstanding in this scope. Route patterns compile before initialization;
unknown converters and duplicate parameters fail without invoking startup code.
The listener does not accept HTTP requests until initialization succeeds. Failure
and cancellation drop the listener. The startup result retains its typed cause.
Request execution uses an owned registration snapshot; registry locks do not
cross awaits or user callbacks.

Route precedence follows the source's ordered full/partial matching. A core docs
route that matches only the path does not shadow a user route for another verb.
Handler-name order is preserved, including dynamic routes shadowing later static
ones. Redirects retain percent encoding and query strings, and documentation
titles are HTML-escaped.

## Should fix

None outstanding. OpenAPI path building now appends parameter names directly
instead of allocating a formatted intermediate string. The fixture helper borrows
its shared call recorder and clones only the handles owned by handlers. The shared
HTML renderer removes duplicated documentation assets without changing the
development worker's routes. No lint suppression was added.

## Consider and remaining scope

- Closure-owned state replaces implicit Python class construction. Cloned handles
  share their captured state; the server does not create an interpreter instance.
- Synchronous functions use the established blocking executor. Native streams
  collect into JSON arrays, retaining the previously documented distinction from
  Python async-generator serialization failures. Native errors/panics and JSON
  numeric/Unicode domains remain Rust runtime adaptations.
- Uvicorn's pre-lifespan application object can report an unready 204 ping in tests.
  The native embedding API follows the real serving gate and accepts no requests
  before initialization. Its shutdown drains active HTTP requests; dropping the
  server future is not a promised handler-cancellation mechanism.
- Arbitrary HTTP response bodies/status/headers, native framework factories,
  live source/binary loading, manifest bootstrap, and automatic mount setup are
  pending integrations. Python ASGI metadata remains usable with Python workers
  and is explicitly rejected by the native server. No placeholder success routes
  were added for the live-only `/execute` or `/_runpod/sync` contracts.
- Axum owns transport settings. Uvicorn's keepalive and implicit proxy-trust
  configuration are not claimed as exact native transport behavior.

These limitations are retained in PORTING.md and parity/runtime-api.json. They
remain part of the overall port work; this review does not declare the full goal
complete.

## Verification

- Test-first compilation failed on the absent `ApiHandle::serve`, as expected.
- **18 runtime scenarios / 150 requests** pass over real TCP, comparing status,
  response body, Allow/Location headers, initialization state, handler arguments,
  and the entire OpenAPI document.
- Six native integration tests cover the differential replay plus startup gating,
  failure/cancellation, shutdown draining, route errors, docs precedence, large
  bodies, streams, and calls through the real SDK client. The focused Apps and
  development-worker checks passed **22 tests**.
- The full Windows Rust 1.99.1 suite passed **497 tests**, with two ignored child
  entrypoints exercised by parent tests. Build, formatting, rustdoc, strict
  Clippy for all targets/features, and pedantic review ran. After the small review
  cleanup, the six Apps HTTP tests and strict Clippy passed again; the runnable
  example is included in all-target checking.
- The unmodified runtime Python test file had **6 passed / 1 failed** on Windows.
  The failure expected `/data` but Python's native Windows Path rendered `\\data`.
  Re-running that test with only the volume module's path renderer replaced by
  `PurePosixPath` passed. This is recorded as a platform adaptation, not a claim
  that the unmodified suite was entirely green on Windows. The runtime checkout
  itself was not changed.
- Prior milestone `f4fe437` completed all four GitHub CI jobs successfully. This
  commit's cross-platform/MSRV verification is performed by its own CI run.

Validation logs are retained in ignored `test-results/apps-http-*`. No live Runpod
resources, image builds, package installations, or billable operations were used.

After verification, the crate's compiled artifacts were cleaned within the resolved
workspace target directory. Dependency caches and test logs were retained, and
incremental compilation remains disabled.

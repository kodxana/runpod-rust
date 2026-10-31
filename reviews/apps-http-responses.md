# Rust review: raw Apps responses and native application factories

Reviewed the library API and implementation using rust-port, rust-best-practices,
and rust-review. Scope includes the response callback, factory lifecycle/health
adapter, API registration/dispatch changes, differential fixtures, and example.
Edition 2024 and MSRV 1.94.1 are unchanged. No dependency or unsafe code was added.

## Source and contract

The separate sdk-runtime remains pinned at
`14f70227008c1ec317e3fe1865a4688b14f5d432`. Its
[`api/server.py`](https://github.com/runpod-workers/sdk-runtime/blob/14f70227008c1ec317e3fe1865a4688b14f5d432/src/runpod_sdk_runtime/api/server.py)
passes framework Response objects through and wraps application factories with
health handling. Characterization ran this actual runtime, using the SDK's existing
Python environment. The reference checkout is unchanged.

Nine response cases cover binary/text/JSON, status overrides, empty 204/304,
redirects, repeated cookies, and nonempty/empty streams. Four factory scenarios
cover missing health, existing GET, existing POST, and opaque ASGI applications.
Their 33 requests retain response bytes, selected headers, and lifecycle events.
Fixture regeneration is byte-identical. These are separate runtime contracts;
the Python SDK symbol/test ledger is not inflated by them.

## Must fix

None outstanding in this scope. Existing JSON handler behavior and direct calls
remain intact. Raw-response methods share the existing route validation and
ordered registration; a typed enum separates their output from JSON values.
Direct JSON calls explicitly reject raw methods without consuming the response.
No registration mutex crosses a callback or await.

Factory startup completes before acceptance. Failure/cancellation releases the
listener and owned resources. Normal shutdown drains requests and response streams
before cleanup. Cleanup errors propagate; a serving error retains precedence if
cleanup also fails, with the secondary failure logged. Dropping the entire serving
future does not promise asynchronous cleanup. Streams preserve backpressure and
errors after headers abort the connection instead of fabricating a new response.

## Should fix

None outstanding. Review replaced truncating fixture status casts with checked
conversion/widening and improved the hex fixture helper's remainder assertion.
Strict Clippy also required the stable slice `as_chunks` API, available before the
crate's MSRV. No warning suppression was added.

## Deliberate native adaptations

- Axum Response/Body/Router types are public at this framework integration boundary.
  This avoids an incomplete duplicate HTTP abstraction and retains arbitrary headers,
  streaming errors, and user middleware. Callers use compatible Axum 0.8 types.
- `FactoryHealth` is an enum, not an ambiguous boolean. Route reflection becomes an
  explicit choice. Added/wrapped health precedes router middleware; use `Preserve`
  for a health route that belongs inside the application's middleware stack.
- Factory routers use native framework method matching, redirects, extraction,
  schemas, and middleware. The adapter does not implement the Python ASGI protocol
  or reproduce FastAPI. Explicit factory startup/cleanup replaces lifespan messages.
- Factory registration is lazy and each serve invocation creates one application.
  Native factory metadata does not pretend the Rust closure is a Python qualified
  name. Native binary deployment, live dispatch, bootstrap, and automatic mount
  initialization remain pending parts of the overall port.
- Transport framing, disconnects, and bodyless status enforcement belong to
  Axum/Hyper; synchronous response constructors use Tokio's blocking pool.

## Verification

- Test-first compilation failed on the absent response/factory API as expected.
- All **22 focused tests** passed: nine existing API handle tests, six class HTTP
  tests, and seven response/factory tests.
- The full Windows Rust 1.99.1 suite passed **504 tests**, with zero failures and
  two ignored child entrypoints exercised by their parent tests.
- The seven new tests passed again after review cleanup. They include real TCP
  streaming before completion, transport error after headers, factory startup
  failure/cancellation, real Axum extraction, graceful request/body draining,
  health-route ownership, direct raw calls, and typed cleanup failures.
- Build, formatting, rustdoc, strict Clippy for all targets/features, and pedantic
  review ran. The new factory example is included in all-target checking.
- Previous commit `7776bcf` passed all four CI jobs: stable Linux/Windows/macOS
  and Linux MSRV. This commit's platform verification belongs to its own CI run.

Logs remain in ignored `test-results/apps-http-responses-*`. No billable Runpod
resources were used. Generated crate artifacts are cleaned after validation within
the resolved workspace target directory; dependency caches/logs remain, and
incremental compilation stays disabled.

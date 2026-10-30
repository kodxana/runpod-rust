# Rust review: blocking registered application handles

Library scope: blocking App, function/API handles, mixed resource snapshots, submitted
jobs, and stream adapters. Edition 2024, MSRV 1.88. Reviewed using the Rust porting, best
practices, and review skills. No blocking finding remains. This is an explicit owned-runtime
facade, not an implementation of Python's process-global event-loop or interrupt machinery.

## Preserved contracts

- Local, remote, submit, reconnect, stream, options, HTTP methods, initialization, and native
  API method calls delegate to the tested async implementations without rewriting protocols.
- Handles, jobs, and output iterators retain the runtime after the owning app is dropped.
  Submission occurs once; reconnect does not resubmit; stream submission remains lazy.
- Startup remains explicit, task ownership remains with the submitted job, and per-job
  options bind a clone. The native handle is available for metadata and async operations.

## Review decisions

- The runtime is privately shared through Arc. Public APIs accept concrete native types and
  expose borrowed async handles; callers never need to manage mutex guards or runtime Arcs.
- Runtime construction inside Tokio returns an error. Blocking methods are for synchronous
  code, including native sync handlers running in the blocking pool. Async tasks use the
  native facade. Runtime destruction follows the same outside-async restriction.
- Recursive worker tests use weak references to caller-owned handle storage, avoiding a
  callback/application ownership cycle. Both direct sync and concurrent async entry paths pass.
- No raw library threads, unsafe code, new dependencies, or global event-loop state were added.
  Tests use separate synchronous caller threads where actual mock HTTP services are needed.

## Verification

- Eight tests exercise native handlers and local HTTP: retained runtime ownership, queue
  payloads/options, jobs, streaming/cancellation, all HTTP verbs, initialization, nested runtime
  rejection, and recursive worker calls. Stable/MSRV gate results are recorded in PORTING.md.
- Source dispatch tests already passed in the preceding upstream subset; this milestone
  specifically adds Rust counterparts for synchronous sentinel dispatch and worker recursion.
- Full stable suite: 256 integration tests, two unit tests, two doctests; build, formatting,
  normal Clippy, and docs also pass. All eight new tests pass on Rust 1.88. Pedantic review
  prompted must_use annotations on lazy stream constructors; no correctness finding remains.

## Limits

- Python's dual callable/.aio wrapper becomes separate Rust async and blocking facades.
  This does not preserve blocking calls made from within a running async task.
- Generic bridge interrupt cleanup and OS signal behavior remain part of the lifecycle audit.
  A dropped blocking task job is not automatically terminated, preserving the existing SDK contract.

## What's already good

- The facade reuses validated routing and cleanup logic instead of maintaining a second copy.
- Tests keep using jobs and iterators after their originating app and function handles are dropped.
- Public Debug output delegates to redacted native objects; captured function state is not exposed.

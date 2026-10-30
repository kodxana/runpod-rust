# Rust review: fitness lifecycle and GPU framework checks

Library review using rust-port, rust-best-practices, and rust-review. Scope: the owned fitness
registry, early/worker-start coordination, configuration refresh, failure reporting/termination,
explicit GPU framework adapters, and worker integration. Edition 2024 and MSRV 1.94.1 remain.
No outstanding correctness or soundness finding was found within this implementation.

## Must fix

None outstanding. `FitnessChecks::check` retains typed failures, and process entrypoints own
the report/exit policy. Pending checks are collected before execution to preserve duplicate
identity semantics. Registration failures restore both entries and latches. Changed settings
invalidate only dependent completed checks, while disk reruns exactly once at worker start.
These follow rust-port's observable-behavior requirement and C-GOOD-ERR.

`src/serverless/health/reporting.rs::terminate_unhealthy` contains the sole production unsafe
operation: C `_exit`, with no pointers or owned buffers crossing FFI. It accepts any exit code
and never returns. The narrow expectation and SAFETY comment follow rust-best-practices §8.
Child-process tests verify the distinction from ordinary C/Rust process shutdown by registering
a C exit callback and a Rust destructor. Cancellation during unhealthy reporting also exits.
Miri was not used: this test requires actual process termination and operating-system behavior.

## Should fix

None outstanding. Worker intake, heartbeat, prestart hooks, and handler execution remain gated
until fitness completes. The library-only API returns failures without terminating its host.
Configuration values stay owned rather than changing the global process environment, following
rust-port's Python/global-state adaptation. Debug excludes environment credentials, late-change
warnings print variable names only, and invalid network targets warn without printing URLs.

Framework adapters retain allocations through synchronization. Their contracts document errors;
Torch availability is never queried before the already-initialized-context gate. No framework
is implicitly loaded. Runtime failures retain context and cannot silently become a successful
benchmark fallback. Review fixes clarified allocation lifetime and error documentation.

## Consider

- Python import hooks become an explicit early call; callers forward the owned marker to
  subprocesses. This is an intentional native API adaptation, not automatic pre-main execution.
- Standard `process::exit` is insufficient because C callbacks still run. The crate's unsafe
  lint changes from forbid to deny solely to permit the documented `_exit` call and test callback.
- Benchmark timing is checked after synchronous computation, matching Python; it cannot stop
  a stuck framework operation. Adapter implementations and physical GPU behavior need their
  own hardware integration validation. This milestone uses deterministic adapters and probes.
- Pedantic review suggestions remaining in new code concern source-shaped negative-condition
  ordering and empty-collection assertion style. They do not alter behavior or error contracts.
- Unmapped import/reflection, injected stdio failures, and combined lifecycle cases remain in
  the parity ledger. Prestart-failure process exit and the uncaught-exception hook remain pending.

## What's already good

- Real child processes prove hard exit, reporting order, and cancellation behavior.
- A mock HTTP worker proves that fitness gates all startup traffic and handler work.
- Registry tests cover identity, rollback, selective reruns, skip-flag changes, and disk rechecks.
- Framework tests verify device order, zero memory, allocation failures, and compute fallback.

## Verification

Windows stable Rust 1.99.1: full suite **450 passed**, with one child entrypoint ignored by the
parent harness and explicitly launched by subprocess tests. The final focused suite passes
**39 tests** on both stable and Rust **1.94.1** (plus the same ignored child entrypoint).
Build, formatting, Clippy with warnings denied, pedantic review, and rustdoc checks ran.
The matching upstream Python registry/startup/safe-startup/force-kill/system suite passes
**130 tests**. Prior probe commit `442b06e` passed all four CI jobs; this milestone's Unix/macOS
execution awaits its new CI run. No live Runpod resource or physical GPU was used.

# Rust review: worker scaling and startup coordination

Library review using the Rust porting, best-practices, and review skills. Scope: the new
scaling/control module, worker-loop changes, startup coordination, and regression tests.
Edition 2024 and MSRV 1.94.1 are unchanged. The full SDK port remains in progress.

## Must fix

No outstanding finding in the implemented scope. Review caught and corrected three behavior
differences: canceled jobs must not trigger refresh; duplicate IDs must retain both queue
entries; and fallback startup-error delivery failures must propagate even though fallback
claim failures are ignored. Regression tests cover each correction. This follows the
rust-port skill's requirement to preserve observable source behavior and error precedence.

## Should fix

No outstanding finding. Owned futures cancel when the run is dropped; the run guard clears
progress, task handles, and queue counts. A concurrent second run cannot clear the first
run's state. No user callback runs under the task-registry lock and no lock crosses await
(rust-best-practices, Concurrency and async). A watch channel wakes shutdown without polling.

## Consider

- Startup hard exit and Python's uncaught-exception hook remain pending with the health
  runtime. The library currently returns typed errors, and its captured error payload is
  not yet equivalent to Python's full log/traceback payload. These are documented limits.
- Progress and cancellation use job IDs while the queue retains duplicate invocations,
  preserving the source's collision behavior. Occupancy also retains double counting of
  queued jobs; changing that requires an intentional compatibility decision.
- Native futures and explicit handlers replace Python's injected task/queue objects.
  Only upstream cases with equivalent native evidence are mapped in the test ledger.
- Pedantic review found style/documentation suggestions and casts of small fixture values,
  with no remaining correctness issue. Public control methods document their lifecycle.

## What's already good

- Deterministic fixture replay checks concurrency changes and occupancy against Python.
- Local HTTP tests exercise scheduling, delivery errors, cancellation, and startup deadlines.
- Zero concurrency waits without spinning, and shutdown interrupts the wait promptly.
- Signal registration is explicit and the Unix test signals only its owned child process.

## Verification

The focused Python suites passed 72 tests with one skip. New native coverage consists of
16 Windows integration tests and one unit test replaying 33 captured cases. The complete
Windows stable Rust 1.99.1 suite passed 393 tests: 387 integration, four unit, and two doctests.
All 24 focused unit/worker tests passed on Rust 1.94.1. Formatting, build, Clippy with warnings
denied, and documentation checks pass. Subsequent CI run
[41069882750](https://github.com/kodxana/runpod-rust/actions/runs/41069882750) passed all four
jobs, including Linux/macOS SIGTERM testing and Linux Rust 1.94.1.
No live Runpod resources were used.

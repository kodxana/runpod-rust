# Rust review: worker process startup and blocking hooks

Library review using rust-port, rust-best-practices, and rust-review. Scope: typed prestart
errors, production/local process entrypoints, panic logging, blocking hooks, and shared
startup deadlines. Edition 2024 and MSRV 1.94.1 remain. Tests and Clippy ran before the
build directory was cleaned. No outstanding correctness or soundness finding remains
within this implementation.

## Must fix

None outstanding. `Error::Prestart` retains the named hook and original typed error
instead of converting them to an invalid-input string (C-GOOD-ERR). Lower-level worker
methods return that error; process entrypoints own the hard-exit policy. Held jobs receive
bounded failure reports first. A fallback report transport error still takes precedence
over process termination, preserving the upstream behavior required by rust-port.

No unsafe code was added. The process entrypoints reuse the separately reviewed unhealthy
termination primitive. Panic-hook replacement is documented and tested in child processes;
normal `Result` failures are not converted to panics or intercepted with `catch_unwind`.

## Should fix

None outstanding. Ordered hooks share one deadline. A zero timeout never invokes a hook
factory; an empty phase bypasses execution while an explicit empty hook run still rejects
zero, matching Python. A timeout returned by a hook remains its original failure rather
than being mistaken for expiration of the outer phase deadline.

The dedicated blocking-hook thread is a deliberate exception to rust-best-practices'
general preference for caller-provided executors. This worker runtime already owns
scheduling, and Python's daemon-thread behavior requires runtime shutdown to remain
possible while a hook is blocked. Tokio's blocking pool would wait for that hook. The
thread and its callback own their data; timeout drops only the receiver and cannot
invalidate references used by the detached callback.

## Consider

- Rust panic hooks run before unwinding, including panics that a caller later catches.
  Python's uncaught-exception hook has different timing. This native adaptation is
  documented, and only production startup replaces the existing process-wide hook.
- Explicit names and separate synchronous/asynchronous registration replace Python
  reflection. Captured context propagation, full captured-log/traceback failure payloads,
  hosted HTTP startup, and the remaining CLI behavior are still pending in the ledger.
- Review-mode Clippy suggestions concern existing getter annotations, source-shaped
  worker control flow, and long child-process test setup. Deliberate test panics exercise
  panic-hook behavior. No additional suppression was introduced.

## What's already good

- Child processes prove report-before-exit ordering, bounded reporting, blocked hooks,
  and local versus production panic-hook installation without contaminating other tests.
- Hook tests cover thread identity, execution order, shared deadlines, and typed causes.
- The implementation preserves the upstream transport-error precedence quirk explicitly.

## Verification

Windows stable Rust 1.99.1: full suite **460 passed**, with two ignored child entrypoints
explicitly invoked by their parent tests. Rust **1.94.1**: focused worker/prestart suite
**30 passed**, plus one ignored child entrypoint. After relaxing only the child tests'
successful-report timing budget for CI, the final worker-process suite passed **5 tests**
plus its ignored child entrypoint. Later production edits were documentation only.

Build, formatting, Clippy with warnings denied, pedantic review, and rustdoc succeeded.
The matching upstream prestart-lifecycle and worker Python suites passed **60 tests**.
The preceding fitness milestone `065941a` passed CI; this milestone's cross-platform CI
awaits its push. No live Runpod resource or physical GPU was used. Logs were retained
under ignored `test-results/` when compiled artifacts were removed to recover disk space.

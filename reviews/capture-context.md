# Rust review: scoped capture and failure reports

Library review using rust-port, rust-best-practices, and rust-review. Scope: capture scopes,
text forwarding, blocking-hook propagation, worker policy, structured startup failures, and
single/streaming handler reports. Edition 2024 and MSRV 1.94.1 remain. No outstanding
correctness or soundness finding remains in this scope; the native output API adaptation
is explicit in README and PORTING.md.

## Must fix

None outstanding. Capture context follows future polling through Tokio task-local scopes,
rather than remaining installed on a shared executor thread across awaits. Sync scopes
restore context on unwind. Callbacks and writes to the real destination run outside the
buffer mutex; no guard crosses an await. Shared ownership is intentional because detached
hooks may finish after their caller stops waiting. Already-built failure reports own a
snapshot, so late output cannot mutate a result sent to another job.

Only completed destination writes are mirrored. Destination I/O failures retain their
typed error, while failed capture bookkeeping is best effort (C-GOOD-ERR and rust-port's
source error contract). No unsafe operation or new dependency was introduced.

## Should fix

None outstanding. Prestart reporting unwraps the named failure to retain its original
message and adds hook, worker, version, native traceback, and bounded output fields. Phase
messages and ready diagnostics are excluded without suppressing real logging. Native
handler messages and backtraces are not clipped; only the optional captured tail is bounded.
Disabled capture and caller-returned error values do not acquire new log fields.

Review removed an unnecessary clone in the prepared handler stream: the stream now owns
its handler, while workers explicitly clone their reusable handle. This follows the
ownership/borrowing rules in rust-best-practices and resolves `needless_pass_by_value`
without changing the new public generator API.

## Consider

- Python global stream replacement becomes explicit `TeeWriter`/SDK stdout and stderr
  destinations. Ordinary `println!`, raw writes, and external-process output bypass them.
  This is documented as an API adaptation, not automatic operating-system interception.
- Native errors report native type names and backtraces at the reporting boundary. Rust
  cannot recreate Python exception objects or interpreter traceback frames. The local
  collecting-stream helper retains its existing typed-error return contract.
- New user tasks/threads copy context explicitly; SDK-owned blocking prestart hooks do so
  automatically and also preserve the current tracing dispatcher. Python-only stream
  identity tests and the remaining hosted/debugger cases stay unmapped or pending.
- Remaining pedantic suggestions concern optional `must_use` annotations, existing worker
  control-flow style, and lengthy integration-test setup. No lint suppression was added.

## What's already good

- Tests precede implementation and retain expected missing-API compilation logs.
- Concurrent jobs, nested scopes, pause/resume, cancellation, unwind, and blocking threads
  exercise context boundaries rather than merely checking implementation branches.
- Mock webhooks prove that the full startup phase is captured and that ordinary and
  streaming handler failures receive the correct bounded log tail.
- Generator tests cover factory failures, synchronous and asynchronous partials, terminal
  errors, and preservation of caller-owned partial values.

## Verification

Windows stable Rust 1.99.1: full suite **483 passed**, with two ignored child entrypoints
explicitly invoked by parent tests. Build, formatting, Clippy with warnings denied, and
rustdoc passed. Pedantic review ran and prompted the ownership cleanup above. After that
cleanup, the final focused worker/capture suite passed **31 tests**, plus one ignored child
entrypoint exercised by its parent tests, and strict Clippy passed again.

Upstream capture/prestart/worker suites passed **96 tests**; the job module passed another
**29 tests**. No live Runpod resource was used. Verification reuses the compact default
`target/` directory with debug information and incremental caches disabled.

The new scoped-subscriber progress test passed in isolation but lost a cached callsite
event alongside no-subscriber hook tests. It now has its own integration-test process,
following the existing process-isolation approach for process-global logging state.
Production behavior was not changed to accommodate this test interaction.

Prior CI found an unrelated upload-test assumption that concurrent parts arrive in numeric
order. Commit `6005be8` checks bodies by part number while retaining ordered-completion
assertions; its four CI jobs passed. This capture milestone's cross-platform/MSRV results
await its push, avoiding a second local toolchain cache.

# Rust review: deployed task HTTP runner

Reviewed the task runner library, deletion transport, socket cancellation, capture changes,
compiled example and parity tests under Rust 2024 / MSRV 1.94.1. The deployed native
protocol has no outstanding correctness findings in this review. Live-source execution
and native artifact packaging remain pending. Formatting, build, tests, strict Clippy,
rustdoc and scoped pedantic review were run.

## Must fix

None outstanding. Three regression tests exposed issues during review; each failed before
its correction and passed afterward:

1. `src/serverless/capture.rs:113`: a closed container log discarded task response stdout
   and returned an I/O error. **Rule:** preserve observable failure behavior (rust-port
   sections 1-2). Task capture now records first, tolerates destination write/flush failure
   and returns the character count. Ordinary worker capture retains its existing policy.

   ```rust
   capture.record(text);
   let _ = self.inner.write_all(text.as_bytes());
   let _ = self.inner.flush();
   return Ok(text.chars().count());
   ```

2. `src/apps/transport.rs:49`: graceful HTTP shutdown could wait indefinitely
   for an incomplete request body after deletion was acknowledged. **Rule:** cancellation
   must release owned I/O and allow lifecycle completion (rust-best-practices section 7).
   Accepted sockets now poll a shared close notification before reads, writes and flushes;
   dropping the runner's sender wakes blocked operations. The regression holds a partial
   authenticated body open while the execution deadline expires.

   ```rust
   if self.cancelled(cx) {
       return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
   }
   ```

3. `src/apps/task_runner.rs:359`: null or later timeout requests woke the watchdog even
   though the deadline did not change, accelerating failed-deletion retries. **Rule:**
   preserve timing and state transitions (rust-port sections 1-2). The deadline setter
   reports an actual earlier deadline; HTTP dispatch wakes the watchdog only then.
   A paused-clock regression verifies the normal fifteen-second retry interval.

   ```rust
   if shared.lock()?.set_timeout(timeout) {
       shared.wake.notify_one();
   }
   ```

## Should fix

None outstanding. Pedantic review identified unnecessary ownership of `PhaseError` in
`src/apps/task_runner.rs:165`; the public method now borrows it. Bootstrap error creation
moves its message into `Value::String`, and body-read validation uses `let ... else`.
**Rules:** borrow values that are only read, and use direct control flow
(rust-best-practices sections 3 and 6; `clippy::needless_pass_by_value`,
`clippy::manual_let_else`).

The source capture harness also had an observation race at `tools/task_runner_fixtures.py:24`:
the client could read a response before the source handler's final state update. It now
waits for handler completion before recording state and beginning another case. Two
successive generations produce the same fixture hash.

## Consider

- `src/apps/task_runner.rs:432` and `tests/task_runner_process.rs:45` exceed the pedantic
  function-length suggestion. Dispatch keeps authentication, readiness, validation and
  reservation order visible; the subprocess helper keeps setup, execution and cleanup
  together. **Rule:** `clippy::too_many_lines`. Further extraction can be considered if
  these flows grow; no broad suppression was added.
- `src/apps/task_runner.rs:183`: embedded hosting returns a typed exit reason and aborts
  owned futures/sockets. It cannot stop arbitrary caller tasks or a running blocking
  function. The process entry point exits after acknowledged deletion, verified with a
  genuinely blocked user function. **Rule:** explicit ownership and cancellation
  (rust-best-practices sections 3 and 7).
- `src/serverless/capture.rs:34`: task stdout deliberately uses an unbounded buffer and
  excludes the SDK stderr writer. Plain process output is not intercepted. This preserves
  the response contract through explicit Rust writers without global stdout replacement.
  **Rule:** preserve behavior while adapting language mechanisms (rust-port sections 3-4).

## What's already good

- Token and credential diagnostics are redacted. Configuration reads only relevant
  environment settings and does not mutate process-global values.
- Typed phase/job state reserves one execution under a short lock. User initialization,
  execution and deletion callbacks run outside locks; accepted jobs survive client exit.
- Deletion requires exact acknowledgement and uses bounded request retries. Deadline,
  idle and initialization policies are exercised independently, including failed deletion.
- Real subprocess tests combine artifact extraction, manifest selection, mount binding,
  HTTP execution, exact deletion headers/payload and process exit. No live Runpod resource
  or credentials are required.

## Verification

- The full Windows suite passed **1,025 tests**, zero failures, with five ignored child
  entry points exercised by parent tests. This run preceded the final retry-cadence fix.
- After that fix, all **32 task runner tests** and **7 existing capture tests** passed,
  including both real subprocess tests. One additional child entry point is ignored by
  the test harness and invoked explicitly by its parents.
- Locked build, fmt check, strict all-target/all-feature Clippy and rustdoc pass on the
  final code. Scoped pedantic findings were addressed or discussed above; unrelated
  pre-existing pedantic warnings remain outside this review.
- The unchanged source suites passed **144 cases** and failed two: Windows mount slash
  rendering, and a source bootstrap child exiting zero after extraction before opening
  its listener. The latter root cause is not established. Neither failure is counted as
  a pass; the native bootstrap/process counterpart passes.
- **48 HTTP captures** match status, JSON body, contact and deadline state. SHA256:
  `BCFD3100B0A1828FF2F6B7F579BEA017ABAEFEB9B6AD5094F9A3949DEE90F89D`.
  The separate runtime ledger maps 21 source test functions to native counterparts.
- SDK and runtime source checkouts retain their pinned commits and have no tracked/index
  modifications. SDK inventory stays at 818/1,135 symbols and 928/1,659 source tests;
  runtime coverage is recorded separately.

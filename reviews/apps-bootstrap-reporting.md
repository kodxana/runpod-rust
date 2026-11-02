# Rust review: bootstrap failure reporting

Reviewed the queue/API failure responders, environment dispatcher, task entry-point
refactor and shared connection cancellation under Rust 2024 / MSRV 1.94.1. This is library
code with an explicitly process-level dispatcher. The implemented failure-reporting
contract has no outstanding correctness findings. Full bootstrap startup, dependency
verification and native binary deployment remain part of the original port goal.

## Must fix

None outstanding. Review exposed a cancellation ownership bug at
`src/apps/bootstrap/reporting.rs:161`: passing the caller's shutdown future directly into
Axum left it running in a detached signal task after the serving future was dropped.
The regression at `tests/bootstrap_reporting.rs:172` failed before the fix and passes
after it. **Rule:** caller-owned state must be released on cancellation
(rust-best-practices sections 3 and 7).

The caller's future is now polled in the serving scope. Axum receives only an owned
oneshot receiver; dropping the scope closes that receiver and the accepted sockets.

```rust
tokio::select! {
    result = &mut server => result?,
    () = shutdown => {
        drop(closing);
        let _ = stop.send(());
        server.await?;
    }
}
```

## Should fix

None outstanding. Pedantic review found test-local items after executable statements
(`clippy::items_after_statements`); the drop-notification type now precedes setup code.
The queue client retains transport/JSON causes
and logs/retries request failures instead of terminating the error-reporting loop. Public
configuration diagnostics and `Debug` do not disclose credentials or webhook URLs.

## Consider

- `src/apps/bootstrap/reporting.rs:111`: the form content type with a JSON body, nested
  JSON error string, raw authorization and immediate empty-job repoll are deliberate
  source quirks. **Rule:** preserve observable protocol behavior (rust-port sections 1-2).
  Do not replace this with the normal worker client's different envelope or backoff policy.
- `src/apps/bootstrap/reporting.rs:240`: missing webhooks return an error from the library;
  the process caller chooses its nonzero exit. Task deletion retains the existing explicit
  process exit after acknowledgement. **Rule:** separate reusable library ownership from
  process policy (rust-best-practices sections 1 and 7).
- `src/apps/bootstrap/reporting.rs:161`: unsupported methods preserve 501 but use native
  JSON rather than Python HTML. HTTP framing and JSON whitespace differ. Reqwest applies
  total-request deadlines, whereas urllib uses socket-operation timeouts. These differences
  are documented instead of emulating the Python runtime (rust-port sections 3-4).

## What's already good

- The responders work before normal application initialization, so failures do not depend
  on the code that failed to load. Task failures cannot accidentally enter the queue loop.
- Real HTTP tests verify raw authorization, error envelopes, all supported API methods,
  request timeouts, retry intervals and recovery. Owned futures make embedding cancellable.
- Subprocess tests verify environment-based queue/API/task dispatch, task diagnostic privacy,
  exact pod authentication, acknowledged idle deletion and process exit without live resources.
- The task runner and API responder share the same private socket cancellation transport.
  No new dependencies, unsafe code or global environment mutation were introduced.

## Verification

- Five unchanged source cases pass, including the previously ported error-payload test.
  Four additional source functions are mapped in the separate runtime reporting ledger.
- Nineteen queue scenarios and seven routing scenarios regenerate deterministically.
  SHA256: `0ED6FB6F0374768F9A7F3BE75E05BC088C61707A095FBE8565A696CFDA20E38A`.
  Eighteen non-timeout queue traces compare requests and delays directly; the timeout
  contract is exercised with real pending HTTP requests and a controlled Tokio clock.
- Fifteen new native tests pass, including four subprocess tests. The ignored subprocess
  entry point is exercised explicitly by its parents. Task runner lifecycle/process tests
  also pass after sharing the transport and refactoring the process entry point.
- The complete Windows suite passes **1,041 tests**, zero failures, with six ignored child
  entry points exercised by parent tests. The request-timeout assertions were strengthened
  afterward and their focused tests pass. Strict all-target/all-feature Clippy passes;
  unrelated pre-existing pedantic warnings remain outside this changed-code review.
- Locked build, formatting check and rustdoc pass. The cancellation regression was rerun
  successfully after the test-only pedantic cleanup.
- Both source checkouts retain their pinned revisions with no tracked/index modifications.
  SDK inventory counts remain unchanged; runtime coverage is accounted for separately.

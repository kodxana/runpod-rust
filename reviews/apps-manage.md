# Rust review: application lifecycle management

Scope: native listing, lookup, environment undeploy, and application deletion. Reviewed
against the pinned Python implementation and Rust porting, best-practices, and review skills.
No blocking finding remains.

## Preserved contracts

- Stable alphabetical listing, missing-record error messages, ordered endpoint teardown,
  optional environment retention, and parent retention after any child deletion failure.
- Deletion failures are collected with operation names; later children are still attempted.
  Lookup errors propagate immediately. Observer errors are logged and suppressed.
- Flags describe calls that returned without error, including false confirmations. The
  application aggregate leaves its environment flag false, matching upstream behavior.
- Canceling the future stops subsequent requests without undoing completed mutations.

## Native design and review

- An explicit borrowed API client replaces implicit global construction. A small enum
  replaces the boolean environment-deletion flag. Typed error variants preserve absence
  separately from transport errors; malformed server metadata becomes structured errors.
- Results own their data; no locks, background tasks, globals, unsafe code, or dependencies
  were added. Sequential calls deliberately preserve externally observable mutation order.
- Endpoint names fall back to IDs for events and diagnostics. Missing endpoint IDs are
  captured as deletion failures so malformed children cannot silently allow parent deletion.

## Verification

- All nine upstream management tests pass on the pinned Python version.
- Fifteen Rust tests exercise local HTTP requests, including endpoint/environment/app
  failures, later-child continuation, false confirmations, empty collections, event
  observer failure, and cancellation while the first deletion is awaiting its response.
- Stable, MSRV, full-suite, and lint results are recorded in PORTING.md. No live Runpod
  resource was deleted by these tests.

# Rust review: endpoint submission and load-balancer calls

Application code, edition 2024, MSRV 1.94.1. This change adds `endpoint run` with queue
submission, async output, waiting/streaming and load-balancer HTTP calls. It reuses the
native observer and data-plane transport, preserves input/validation order, and leaves
the public SDK API unchanged. A malformed-metadata submission bug was found and fixed
before commit. Required checks and test results are recorded below.

## Must fix

### 1. Reject malformed endpoint records before submission (`src/cli/endpoint/run.rs:100`)

**Rule:** Preserve side-effect and failure ordering; return typed errors for invalid
external data (rust-port sections 1 and 3, rust-review correctness/error handling).

**Problem:** A null, array or string metadata response selected the queue branch, allowing
a POST before failing on the submitted result. Python fails on the metadata access first.
A process regression reproduced two requests when only the lookup should occur.

**Fix applied:**

```rust
if !record.is_object() {
    return Err(Error::InvalidResponse("endpoint must be an object".into()).into());
}
```

The regression now checks all three malformed shapes and verifies exit 1 with only GET.
No correctness finding remains outstanding.

## Should fix

None outstanding. `RunOptions::read` opens a file before callback-level mode checks,
validates input before credentials, and represents the mutually exclusive wait/stream/
async choice with a private `Mode` enum. Pedantic review motivated replacing the initial
pair of stored mode booleans; parser behavior and error ordering remain unchanged.

**Rule:** Use enums for closed sets and make invalid states unrepresentable (rust-port
section 3, Rust API Guidelines C-CUSTOM-TYPE). A native process regression verifies
file-opening, flag-conflict and invalid-JSON ordering without credentials configured.

## Consider

- `call_load_balancer` owns a single-request HTTP client so empty-body requests do not
  inherit the SDK control-plane Content-Type header. It retains explicit no-redirect and
  no-implicit-retry policies. JSON bodies supply their content type automatically.
- Queue observation starts after lookup and submission. A real delayed lookup and POST
  take longer than the configured wait timeout, followed by one bounded status/stream
  read. The regression proves the wait budget is neither consumed early nor lost, and
  that timeout handling does not submit another job.
- Python policy coercion from non-record values, unbounded integers and nonstandard JSON
  are documented native adaptations. Truthy policies must be objects, milliseconds use
  checked arithmetic, and file contents are UTF-8. Missing/falsy policy values retain
  the source's merge behavior on supported JSON records.
- Process fixtures replace load-balancer HTTP origins with loopback URLs. A separate
  test verifies original/default URL construction from those same Python fixtures, so
  the substitution does not hide fallback-host or path-joining errors.
- Pedantic review retains `too_many_lines` for the single process fixture verifier, which
  keeps request-sequence and output comparisons together. The production mode-flag
  advisory and a test `map(...).unwrap_or(...)` advisory were resolved. No suppression
  was added; strict default Clippy passes.

## What's already good

- Queue input wrapping tests distinguish key presence from truthiness and include null,
  false, empty input objects/arrays, whole-body null and unrelated policy keys.
- Load-balancer tests cover GET/POST/default/explicit methods, base paths and queries,
  JSON and plain text, scalar results, error bodies and empty-body headers.
- Validation and HTTP failures preserve output/exit ordering; asynchronous submission
  avoids polling, and waiting/streaming use the existing tested observer.
- File/stdin/inline input and encoded IDs are tested through actual native executables.
  All requests use ephemeral loopback servers and synthetic credentials.

## Verification

- All sixteen initial process groups failed while `run` was absent, then passed against
  the implementation. The later malformed-metadata regression also failed before its fix.
- Eleven unchanged source test functions pass as **22 parameterized cases**. The generator
  records **51 invocations** in sixteen groups and regenerates byte-identical fixtures.
- Nineteen native process tests and one URL unit test cover the new implementation.
  The full Windows suite passes **796 tests**, zero failures and three ignored child
  entry points exercised by parent tests, including two passing doctests. Locked build,
  formatting, strict all-target/all-feature Clippy and rustdoc pass; pedantic review is
  described above.
- Real Windows terminal execution performed GET lookup, one POST with `{"input":{}}`,
  then streaming reads. Ctrl-C printed the resume command and retained the source's
  empty `Error:` line, with child exit 1 and no cancellation or second submission.
- Previous commit `7fcc5fc` passed Linux, macOS, Windows and MSRV CI. This commit's matrix
  is checked separately. The pinned Python checkout remains unchanged. Remaining run
  fault-injection source tests stay explicitly pending in the parity ledger.

Ignored logs are in `test-results/endpoint-run-*`; the manual harness is
`test-results/endpoint-run-terminal.py`. Package artifacts are trimmed after verification.

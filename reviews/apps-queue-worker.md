# Rust review: deployed Apps queue worker

Reviewed the library adapter, FunctionHandle APIs, shared core startup extraction,
example, fixtures and tests under Rust 2024 / MSRV 1.94.1. No outstanding correctness
findings remain in the implemented deployed-function contract. Full native packaging,
manifest/bootstrap selection and live execution remain required by the goal. Formatting,
build, tests, strict Clippy, rustdoc and a pedantic review were run.

## Must fix

None outstanding. Review fixed one behavioral defect before publication:

- `src/apps/worker.rs:22`: ordinary serde_json map removal uses swap removal under
  `preserve_order`, moving the final keyword into the `__empty` marker's position.
  Python's `dict.pop` preserves remaining key order. Changed to `shift_remove`.
  A new upstream characterization returns keyword names as an array, reproducing
  `[first, third, second]` before the fix and proving `[first, second, third]` afterward.
  **Rule:** preserve observable ordering (rust-port sections 1–2).

## Should fix

None outstanding. Four scoped pedantic suggestions were applied in tests: explicit
map construction, an `if let` for one successful result case, and two empty-vector
assertions that now display unexpected arguments. No lint suppressions were added.

## Consider

- `src/apps/worker.rs:12`: truthy input requires a JSON object. Python's incidental
  `dict` coercion of pair arrays is deliberately not reproduced. Missing/falsey JSON
  values are supported; native typed errors replace Python exception formatting.
  **Rule:** avoid emulating the source language's runtime (rust-port section 4).
- `src/apps/worker.rs:51`: concurrency uses sized Rust integer parsing. Underscores,
  Unicode digits and overflowing positive values use the documented invalid fallback.
  The real worker test verifies the constant modifier, not just config construction.
- `src/apps/handles.rs:131`: synchronous initialization uses the existing blocking
  pool; already-running blocking calls finish after cancellation. Async initialization
  is cancellable and does not start transport when dropped. Native streaming init hooks
  collect according to existing NativeFunction semantics; Python does not iterate
  generator init results. These language differences are explicit in PORTING.md.
- `src/apps/handles.rs:151`: startup selects a caller-provided compiled handle. It does
  not claim the runtime's manifest loading, mount bootstrap, or live-source path.
  Discovery export must occur before startup, as demonstrated in the example.

## What's already good

- Registration snapshots release the app mutex before invoking any user function.
  Init finishes before core configuration/fitness checks, heartbeat or intake, and
  errors preserve typed sources. Worker reuse retains the prepared body without
  reinitializing or depending on the original handle's lifetime.
- Owned streams preserve incremental delivery; the transport sees the first chunk
  before the generator resumes. A terminal failure preserves sent chunks and suppresses
  later output. Streaming aggregation is enabled without discarding other worker options.
- Core startup was extracted into a private helper; the existing standalone worker API
  retains its behavior. Existing realtime/process and full-suite checks cover that seam.
- No unsafe code, dependency, mutable process-global registry or parent environment
  modification was added. Child-process environment is isolated, bounded and reaped.
- Runtime coverage is tracked separately from the Python SDK inventory; the 17 source
  tests and 35 cases do not inflate SDK completion percentages. No live Runpod resources
  or account credentials were needed.

## Verification

- Seventeen unchanged Python runtime tests pass. The capture script produces 35 cases
  deterministically, SHA256
  `5BE7A3146F89F98D8F9C374549033320C7507F031A35CF078217A5F04DA109D5`.
- Nineteen native unit tests, six worker integration tests and three added process
  scenarios pass. The process scenarios use the existing ignored child entry point.
  Actual webhook traffic proves three concurrent jobs, partials and final aggregates;
  local startup and init failure before invalid core configuration also pass.
- The complete Windows suite passes **958 tests**, zero failures and four ignored
  child entry points exercised by parents. Locked build, fmt, strict all-target/all-feature
  Clippy and rustdoc pass. Focused tests and strict Clippy were repeated after review fixes.
- Both upstream checkouts retain their pinned commits with no tracked/index changes.

# Rust review: job waiting and streaming

Application code, edition 2024, MSRV 1.94.1. The native CLI now waits for queued jobs and
streams their output using bounded requests, an owned monotonic clock and incremental
writers. This reuses the existing data-plane transport and does not expand the public SDK
API. No correctness finding remains outstanding. The full test suite, build, strict
Clippy and documentation checks are run; validation details are below.

## Must fix

None outstanding. Differential checks exposed two interrupt details in
`src/cli/endpoint/job_follow.rs`, `Follow::interrupted` and the fallback status read:

- The source catches Click's Abort as RuntimeError, producing an empty `Error:` message
  after the resume hint. The native implementation now returns `CliError::Failed` with
  an empty message at that boundary, preserving exit 1 and the exact stderr text.
- The final fallback request is outside the source polling interrupt handler. Its
  interruption now writes the leading newline and returns `CliError::Aborted` without
  a resume hint. It must not be moved under the ordinary polling interrupt handler.

**Rule:** Preserve observable quirks, outputs and failure ordering (rust-port section 1).
Both differences were caught by failing pinned Python fixture comparisons before correction.
No remote cancellation request is sent on either interrupt path.

## Should fix

None outstanding. The fixture test initially compared serialized sleep numbers, which
treated Python `1` and Rust `1.0` as different values. `job_follow_tests.rs`, `verify`, now
compares durations as floating-point numbers while retaining request order, sleep counts
and strict timeout checks. Production timing was unchanged.

**Rule:** Compare equivalent observable values at the same boundary (rust-port section 2).
Strict Clippy also corrected two nested conditional statements using supported let chains.

## Consider

- `Follow::remaining` applies the source terminal-state exemption to request deadlines,
  allowing remaining chunks and the final status record to drain after completion. A
  private transport boundary permits deterministic timeout/interrupt tests without
  exposing mocks or clock abstractions in the public library.
- Reqwest bounds the whole request rather than Python's socket read-idle interval.
  Native transport causes are retained; connection timeouts do not take the stream
  read-timeout retry branch. This difference is documented in `PORTING.md`.
- Native output replaces Rich's waiting spinner, and streamed JSON uses native compact
  whitespace and Unicode spelling. Comparisons preserve values, strings, line boundaries,
  exact error text, request timeouts and sleep sequences.
- Pedantic review leaves two `unused_async_trait_impl` advisories on immediate test fakes
  and one `assert_is_empty` advisory in the process test. The fakes deliberately implement
  asynchronous production interfaces; the direct empty-output assertion is readable.
  No lint suppression or artificial await was added. Strict default Clippy passes.

## What's already good

- Text chunks flush immediately, JSON chunks finish an open text line, and successful
  streamed output is not duplicated by fetching the final result.
- Failed streams retain partial chunks and print a JSON final record before the error
  where requested. Missing/null chunk lists use the source's polling/fallback behavior.
- The wait deadline includes HTTP elapsed time, bounds each new request and sleep, and
  produces a resumable command when exhausted. Real delayed HTTP verifies this boundary.
- Required response fields and duration conversions return typed failures, with no new
  unsafe code, runtime dependency or panic-based handling of external input.

## Verification

- The two new initial process tests failed while wait/stream commands were absent.
- Two unchanged upstream tests pass; 47 actual Python observer invocations in sixteen
  groups regenerate byte-identically. Sixteen native observer tests and three process
  tests cover command dispatch, validation before credentials and an in-flight deadline.
- The full Windows suite passes **776 tests**, zero failures and three ignored child
  entry points exercised by parent tests; the count includes two passing doctests.
  Locked build, formatting, strict all-target/all-feature Clippy and rustdoc all pass.
- Real Windows terminal checks interrupted both job wait and job stream. Each child
  returned exit 1 with the appropriate resume command and preserved empty `Error:` line.
  Requests used loopback mocks and synthetic credentials; no real Runpod resources were used.
- The previous job-control commit `54ea6e2` passed all four Linux/macOS/Windows/MSRV jobs.
  This commit's CI is checked separately. The pinned Python checkout remains unchanged.

Ignored logs are in `test-results/job-follow-*`; the terminal harness is
`test-results/job-follow-terminal.py`. Build artifacts are trimmed after verification.

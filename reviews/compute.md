# Rust review: compute selection and live catalog requests

Reviewed the new library module, Client/blocking methods, typed resolution error,
fixtures, and example using rust-port, rust-best-practices, and rust-review.
Edition 2024 and MSRV 1.94.1 remain unchanged. No dependency or unsafe code was added.
The implementation is approximately 400 nonblank, noncomment Rust lines for 217
nonblank Python lines, excluding tests and facade methods.

## Source and contract

Source: [`runpod/rp_cli/compute.py`](https://github.com/runpod/runpod-python/blob/ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5/runpod/rp_cli/compute.py),
at the existing SDK pin `ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5`.
All 17 symbols and 17 source test functions have explicit ledger mappings.
The Python checkout remains unchanged. This implements shared compute behavior;
the CLI command/parser/rendering layer remains unfinished work in the full goal.

## Must fix

None outstanding in this scope. Matching stages preserve precedence and catalog
order. Pool deduplication preserves first appearance, exclusion IDs retain catalog
order, region errors report known choices, and CPU resolution distinguishes unknown
from ambiguous. Numeric parsing uses checked conversions without panics or wraparound.
CUDA floors compare numeric pairs and retain the first spelling on ties.

## Should fix

None outstanding. Review changed GPU query options to owned strings, keeping the
public options bag easy to store and pass between tasks (Rust API Guidelines
C-COMMON-TRAITS and the ownership guidance). JSON records deliberately stay at the
existing SDK transport boundary, preserving fields unknown to this SDK. Private
typed views validate the fields needed for matching and borrow original records.
Public malformed-record failures use `InvalidResponse`; resolution failures retain
a distinct `ResolutionError` through `Error::Compute` (C-GOOD-ERR).

Strict Clippy identified a collapsible condition, which was simplified. Pedantic
review identified four test-only clarity issues (`map_unwrap_or` and
`default_trait_access`), which were corrected. No lint suppression was added.

## Consider

- Source globals become owned async clients plus the existing blocking facade.
  Blocking methods are tested on a native thread outside Tokio, consistent with
  the facade's explicit nested-runtime restriction.
- The defaultable public options bag is constructible with a struct literal,
  consistent with existing SDK option types. Product/cloud values remain strings
  because the source forwards them and the remote catalog may add values.
- Counts and version components are ASCII `u64`, rather than arbitrary Python
  integers or Unicode decimal digits. Native whitespace/case conversion follows
  Rust; Python's additional control-character whitespace is not emulated.
- Malformed records are validated eagerly in some paths where Python would
  short-circuit. Valid catalog data preserves source matching behavior and raw
  JSON values, including missing/null optional fields and future fields.
- Empty query behavior, unknown stock labels with rank zero, first-on-tie stock
  selection, duplicate exact CUDA versions, and empty-product query parameters
  are preserved quirks, not silently normalized away.

## What's already good

- Matching returns borrowed records and retains unknown catalog fields.
- API error causes/status and authentication stay in the shared transport.
- Tests compare complete query maps, not only expected key subsets.
- The fixtures execute actual pinned Python code without live Runpod traffic.

## Verification

- Test-first compilation failed on the missing compute API before implementation.
- The original Python tests passed 35 parameterized cases. The 111 captured
  differential cases regenerate byte-identically.
- All 21 native compute tests passed: 17 named source counterparts and four
  additional tests covering numeric limits, malformed JSON, transport errors,
  blocking calls, authentication, and preservation of unknown fields.
- The full Windows Rust 1.99.1 suite passed **614 tests**, zero failures, and two
  ignored child entrypoints exercised by their parent tests.
- Build, formatting, rustdoc, strict Clippy, and pedantic review ran. The example
  compiles under all-target checking; no live catalog request was made.
- The separate cache CI corrections passed Linux MSRV. Stable platform results
  belong to the corresponding CI runs; local Windows success alone does not prove
  Linux/macOS parity.

CI also exposed a pre-existing catalog cancellation-test race on macOS: a 20 ms
deadline could expire before the server received the first request, making its
two-request assertion unreliable. The test now acknowledges receipt, applies a
virtual-time deadline to the pending request, drops the cancelled future, and
verifies a fresh request succeeds. This retains the cancellation/lock-release
contract without depending on runner scheduling. Cache test fixtures now drop
their pending-sync owners before their temporary directories to prevent late
writers from recreating cleaned directories.

Logs remain in ignored `test-results/compute-*`. The compact build profile is
unchanged, and generated crate artifacts are cleaned after final validation.

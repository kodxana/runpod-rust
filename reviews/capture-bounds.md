# Rust review: capture policy and text bounds

Library review using rust-port, rust-best-practices, and rust-review. Scope: the capture-mode
enum, owned text buffer, and clipping helper. Edition 2024 and MSRV 1.94.1 remain. The
implementation has no outstanding correctness or soundness finding in this scope.

## Must fix

None outstanding. All slicing boundaries come from `str::char_indices`, preserving UTF-8
validity while matching Python character counts on valid Unicode. Clipping only calculates
`length - 2 * keep` after establishing `length > limit`, so the arithmetic cannot underflow
or overflow. Zero limits are handled before subtraction in buffer indexing.

## Should fix

None outstanding. An oversized write copies only its retained tail into the owned buffer;
it does not leave an allocation proportional to discarded input resident. Short clipped
strings borrow their input (`Cow<str>`), and getters borrow stored text (C-BORROWED).
Capture selection uses a closed enum instead of unvalidated strings, following the Python
adaptations in rust-port. No new dependency, global mutation, synchronization, or unsafe
operation was introduced.

## Consider

- `RingBuffer::new(0)` is unbounded, and `clip` limits zero and one retain the whole tail.
  These surprising Python slice quirks are documented and verified by differential cases;
  silently changing them would violate rust-port's observable-behavior requirement.
- Native unsigned limits and valid UTF-8 strings do not represent Python negative limits
  or lone surrogates. These source-language differences are recorded in PORTING.md.
- Stream interception, task-local capture, propagation to blocking hooks, and failure-payload
  integration remain pending. Tests for those behaviors are deliberately not marked ported.

## What's already good

- Tests were written first and confirmed the expected missing-module compilation failure.
- Twenty-six real Python fixtures cover chunked writes, Unicode, odd and zero limits,
  default-size boundaries, whitespace/case normalization, and invalid modes.
- Public helpers document character units and the inherited edge cases explicitly.
- The fixture generator runs the pinned Python implementation rather than duplicating it.

## Verification

Windows stable Rust 1.99.1: full suite **467 passed**, with two ignored child entrypoints
explicitly exercised by their parent tests. The final focused suite passes **seven tests**,
including all 26 differential cases; the original Python capture suite passes **22 tests**.
Build, formatting, Clippy with warnings denied, and rustdoc passed. Pedantic review also ran;
the new module's suggestions are optional `must_use` annotations on value-returning helpers.
Cross-platform and MSRV execution await this commit's CI, avoiding a second local build cache.

The compact dev/test profile disables debug information and incremental artifacts. Cargo's
compiler-artifact records confirm debug assertions and overflow checks remain enabled for
tests. The full verification reused `target/`; no separate MSRV directory was created.

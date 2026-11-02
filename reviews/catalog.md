# Rust review: native catalog command

Reviewed the new CLI application layer, its two executable entry points, rendering,
fixtures, and ledger mappings with rust-port, rust-best-practices, and rust-review.
The library exposes only `cli::run`; command configuration and display helpers stay
private. Edition 2024 and MSRV 1.94.1 remain unchanged. Clap handles argument parsing
and unicode-width measures table cells. No unsafe code or live Runpod traffic was added.
Formatting, strict Clippy, pedantic review, tests, build, and rustdoc were run.

## Source and contract

Source: [`runpod/rp_cli/catalog.py`](https://github.com/runpod/runpod-python/blob/ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5/runpod/rp_cli/catalog.py)
and the JSON/table subset of `rp_cli/output.py`, at the unchanged SDK pin.
All nine catalog symbols and four output helpers have explicit ledger mappings.
The root CLI as a whole, other commands, prompts, completion, and update integration
remain pending. The implementation uses the already-ported compute and HTTP clients.

## Must fix

None outstanding. Review corrected the VRAM comparison in `src/cli/catalog.rs`
to accept numeric memory values, including fractional JSON numbers, and to retain
negative/zero threshold behavior. The added Python fixtures cover the correction.
It also rejects non-object rows before adding derived fields, preventing externally
supplied JSON from reaching a panicking mutable index.

**Rules:** Preserve observable behavior (rust-port section 1); avoid reachable input
panics (rust-review correctness and soundness). Both corrections are implemented.

CUDA syntax is resolved before client construction in `Options::gpu_rows`; a bad
filter must not be hidden by a later configuration error. A process-level test uses
an invalid service URL to establish that ordering without making a request.

## Should fix

None outstanding. Pedantic review identified diagnostic-poor empty-output assertions
and an oversized fixture helper in `tests/cli_catalog.rs`. These now use equality
assertions and a separate `verify_table` helper (`clippy::assert_is_empty`,
`clippy::too_many_lines`). No lint suppression was added. Production CLI code had no
new pedantic diagnostics. Existing warnings elsewhere in the crate are outside this diff.

## Consider

- Native mode/product/cloud enums replace strings internally; Clap types do not
  enter the library API. The options object owns its strings and argument lists.
- `--count` and `--vcpu` use checked `u32`, while minimum VRAM uses `i32`. Python's
  arbitrary integers and Unicode numeric syntax are documented adaptations, following
  rust-port's instruction not to reproduce the source language runtime.
- Tables preserve headers, values, ordering, literal markup, and complete identifiers,
  with Unicode display widths. They use unwrapped native columns instead of Rich
  folding, colors, and renderable objects. JSON uses native numeric spellings and
  Unicode escapes; comparisons assert numeric values rather than float typography.
- Shared output accepts JSON values. Arbitrary Python objects, nested object `repr`,
  and `default=str` fallbacks are not reproduced. The helper mappings are therefore
  adaptations; their remaining interactive/prompt/completion functions stay pending.
- Scalar API fields needed for work are validated with `InvalidResponse`. This can
  reject malformed data earlier than Python or tolerate absent optional stock where
  Python would raise a lookup/type exception. Valid catalog data retains unknown fields.
- The CLI-only layer adds approximately 740 nonblank/noncomment lines excluding its
  unit tests; explicit typed parser declarations and Rust formatting account for much
  of the difference from the roughly 300-line source command and its shared rendering.

## What's already good

- Usage validation precedes network access and distinguishes explicitly supplied defaults.
- Complete HTTP query maps, authentication, request counts, error messages, and exit
  codes are checked against Python, as well as returned values and table cell order.
- Stable sorting and source price/stock quirks survive the port, including zero prices,
  unknown availability ranks, repeated scalar flags, and last product flag winning.
- Both installed executable names share one entry point; no subprocess Python runtime
  is required to execute the Rust command.

## Verification

- Test-first compilation failed because the `rp` binary did not yet exist.
- The unchanged Python catalog test file passes **69 parameterized cases**; this
  includes its completion test, whose Rust mapping intentionally remains pending.
  The two selected output-helper tests also pass.
- **120 Python command fixtures** regenerate byte-identically. The 24 native CLI
  tests include 21 source counterparts plus edge, parser/alias, and malformed-response
  coverage. Two library tests map the selected output helpers.
- Full Windows Rust 1.99.1 all-target testing passed **638 tests**, zero failures,
  and two ignored child entry points exercised by parent tests. Both doctests passed,
  giving **640 passed** across the full suite and doctests. After the final CUDA
  ordering correction and test refactoring, all 24 CLI tests passed again.
- Locked build, strict all-target/all-feature Clippy, formatting, and rustdoc passed.
  Pedantic output was reviewed. CI separately checks Linux MSRV and stable Linux,
  Windows, and macOS; local Windows results do not establish those platform results.

Ignored logs are under `test-results/catalog-*`. The compact Cargo profile remains
enabled, and package-generated build artifacts are cleaned after validation.

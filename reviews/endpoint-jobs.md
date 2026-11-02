# Rust review: endpoint job inspection and control

Application code, edition 2024, MSRV 1.94.1. This change adds job get/cancel/retry and
queue purge to the native CLI, with encoded data-plane routes, source-compatible output
choices and confirmation ordering. The public SDK API is unchanged. Build, tests, strict
Clippy, pedantic review and documentation checks pass as described below; no correctness
finding remains outstanding. Job wait/stream, endpoint run/logs and completions remain
explicitly pending in their own ledger entries.

## Must fix

None outstanding. `src/cli/endpoint/jobs.rs`, `execute`, asks for purge confirmation before
constructing the authenticated client. The process test removes API-key and home-path
variables and proves usage exit 2 occurs without any request. This avoids reading
credentials or sending a mutation before confirmation.

**Rule:** Preserve observable failure and side-effect ordering (rust-port section 1).

## Should fix

None outstanding. `tests/cli_endpoint_jobs.rs`, `verify`, initially narrowed fixture HTTP
statuses with an unchecked `as u16`. Review replaced it with
`u16::try_from(reply["status"].as_u64().unwrap()).unwrap()`, so malformed test data fails
explicitly rather than silently truncating. The final focused tests and strict lint checks
were rerun after this test-only correction.

**Rule:** Handle bounded integer conversion deliberately (`clippy::cast_possible_truncation`,
rust-port section 3). Pedantic review also corrected a missing statement semicolon.

## Consider

- `src/cli/endpoint/jobs.rs`, `request`, deliberately retains the SDK's existing disabled
  redirects and implicit retries. The source runner's ten-second request deadline is
  explicit. HTTP failure messages retain raw response text rather than using the
  control-plane JSON-detail extraction. Transport and JSON failures remain typed causes.
- `src/cli/endpoint/view.rs`, `job`, uses compact Serde JSON in a human-readable output
  cell. Tests compare decoded output values where the source uses different whitespace
  or Unicode escapes. Plain strings remain plain strings; raw JSON preserves unknown keys.
- `tests/cli_endpoint_jobs.rs`, `verify`, retains a `clippy::too_many_lines` advisory:
  one fixture is checked against its request and both output streams together. Splitting
  that test solely for the line limit would scatter the boundary comparisons. No lint
  suppression was added. Strict default Clippy passes; unrelated existing pedantic
  diagnostics are outside this change.

## What's already good

- A private enum expresses the four operations; no public helper framework or dependency
  was added. The implementation reuses native path encoding, field rendering and prompts.
- Three original tests run unchanged, and eight named native tests cover 31 captured
  invocations. Request assertions distinguish data-plane URLs from control-plane URLs.
- Cancel keeps the requested ID, retry can return a new ID, and purge distinguishes an
  absent removed count from null. Empty job IDs preserve the source's omitted path suffix.
- Errors, response JSON and successful mutation output use the existing incremental CLI
  writers. All tests use synthetic credentials and ephemeral loopback HTTP servers.

## Verification

- All eight new test groups failed before commands were implemented, then passed.
- The pinned upstream subset passes three test functions; 31 fixture invocations across
  eight groups regenerate byte-identically. The Python checkout remains unchanged.
- The full Windows suite passes **757 tests**, zero failures and three ignored child
  entry points exercised by parent tests, including two passing doctests. After the
  test-only review corrections, all eight job tests pass again.
- Locked build, rustdoc, formatting and strict all-target/all-feature Clippy pass.
  Pedantic checks were reviewed as described above.
- The previous endpoint-resource commit `3696f03` passed Linux, macOS, Windows and MSRV CI.
  This commit's matrix is tracked separately. No live Runpod resources were used.

Ignored validation logs are under `test-results/endpoint-jobs-*`. The compact development
profile remains enabled; package artifacts are cleaned after validation.

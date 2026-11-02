# Rust review: deployment CLI and Cargo worker builds

Reviewed the library discovery/build API and application CLI diff under Rust 2024 /
MSRV 1.94.1 using rust-port, rust-best-practices and rust-review. The command connects
existing artifact/deployment APIs with explicit native build selection and owned progress.
The full Windows suite and strict Clippy pass. No correctness findings remain outstanding.

## Must fix

None outstanding. `src/apps/discovery/worker_build.rs::compile` selects Cargo's executable
record by package identity, binary name and target kind after checking process success.
It cannot return a stale previous executable after failure, guesses no output path, and
never executes a deployment-target binary. Required features survive discovery/build;
`--locked` rejects dependency changes. Target/profile values are single argv entries.
Rule: preserve observable failures and explicit ownership (rust-port sections 1 and 3;
rust-best-practices sections 3 and 5).

`src/cli/deployment.rs::Options::read` rejects conflicting runtime flags before any app
discovery. `run_operation` uses existing native preflight and deployment sequencing.
Build-only requires no credentials. Each artifact owns its lifetime through upload or
explicit output; cancellation prevents later cloud calls. Cargo children use the existing
kill/reap path. `Streams::finish_frame` prevents the next app from erasing earlier output.

## Should fix

None outstanding. Pedantic review identified missing code backticks in target docs,
unseparated byte-count literals, test `map(...).unwrap_or_else(...)` chains, a negative
if/else and an empty-output assertion with poor failure diagnostics. These are corrected
in the changed files (`clippy::doc_markdown`, `unreadable_literal`, `map_unwrap_or`,
`if_not_else`, `assert_is_empty`). No warning suppression or dependency was added.

## Consider

- `deployment_progress.rs::bar` truncates a fraction clamped to 0..1 into 0..22 cells.
  This intentional display conversion cannot underflow the remaining-cell count. Display
  byte counters use floating-point MiB like Python; transport sizes remain integer bytes.
  `cast_precision_loss`, `cast_possible_truncation` and `cast_sign_loss` are not correctness
  problems on this boundary. No numeric enforcement uses the rendered values.
- `deployment.rs::Options::run_in` retains one readable command sequence; the pedantic
  line-count warning does not justify scattering its sequential side effects. The Python
  artifact integration test is similarly long because it creates real filesystem/process
  fixtures. Existing discovery line-count warnings predate this work.
- Rust replaces Rich styling/threaded rendering with native frames and plain text; the
  unfilled bar uses a thin glyph, and zero-total progress is defined. Empty plans render
  zero apps rather than raising Python's `max()` error, but the command calls this helper
  only for multiple apps. Cargo source paths always exist, unlike optional Python metadata.
- `--runtime python` needs actual Python source symbols/assets plus a build interpreter.
  It does not execute Rust closures on Python workers or introduce a Python SDK bridge.
  Existing package ignore semantics remain unchanged; mixed projects should ignore target/.
- Cross compilation requires caller-installed tools and compatible container libraries.
  The worker target is never run during building; host-discovered metadata must describe
  that target too. Build-only can package non-Linux workers for local checks, while native
  deployment separately requires a Linux x86_64 ELF header.
- Native coordinator tasks, other Flash commands and live dispatch remain pending. Local
  backend tests do not establish live Runpod compatibility. No cloud resources were created.
- A blocking artifact job already running at cancellation finishes. Direct child cleanup
  does not promise a portable process-group kill of every compiler descendant.

## Verification

- New build and CLI tests first failed for missing API/subcommand support. Focused checks
  cover nine discovery tests plus its ignored child entry point, nine CLI process tests,
  and ten deployment unit/integration tests. The child is exercised by its parent test.
- The complete locked all-feature Windows suite passes **1,096 tests**, zero failures,
  with eight ignored subprocess entry points exercised by their parents.
- Nine unchanged deployment CLI tests, six console tests and eleven discovery regressions
  pass in pinned Python. Nine command captures compare argument semantics, banners and
  stable errors. Paths and elapsed times are normalized; no source files were changed.
- Native process tests use real Cargo builds and local HTTP servers. They verify environment
  forwarding, sequential multi-app deployment, first-failure stopping, anonymous upload,
  archive bytes, build-only without authentication, and both CLI executable names.
- Automatic build tests verify required features/custom profiles, no execution of worker
  builds, stale-file failures and unchanged lockfiles. Python artifact tests use fake
  installer processes to check explicit version, exclusions and unsupported-version fallback.
- Windows terminal checks show animated phase replacement, durable first-app summaries,
  OSC 8 endpoint links and correct second-app output. An injected finalization failure
  exits 1 after one upload, without creating an endpoint or starting the second app.
- Locked build, fmt check, strict all-target/all-feature Clippy, rustdoc and focused tests
  after the review edits all pass. Pedantic output is reviewed only for this
  diff; unrelated pre-existing warnings are not silently classified as new problems.

Reproduce captures with `tools/deploy_cli_fixtures.py` using the pinned SDK/runtime on
`PYTHONPATH`; regenerate mappings with `tools/update_deploy_cli_parity.py`.
See [parity/deploy-cli.json](../parity/deploy-cli.json) for exact symbol/test mapping and
capture SHA-256. Fourteen API entries are adaptations. Fifteen source tests are mapped
(five ported, ten adapted), bringing coverage to 832 symbols and 943 source test functions.

Cargo contracts used here are documented in the official
[build command](https://doc.rust-lang.org/cargo/commands/cargo-build.html) and
[JSON message format](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages).
Package-ID selection is supported below this crate's MSRV; the implementation avoids
newer convenience target aliases.

## What's already good

- Public build options have documented defaults and typed errors with retained causes.
- Shared deployment code preserves manifests, mutation ordering and partial-failure behavior.
- Tests inspect archive bytes and subprocess results instead of mirroring implementation.
- Native adaptations and pending platform boundaries remain explicit in the parity ledger.

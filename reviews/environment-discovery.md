# Rust review: environments and compiled metadata discovery

Reviewed the native environment CLI, shared cleanup output, discovery library, process
guard, example and tests under Rust 2024 / MSRV 1.94.1. The implemented metadata contract
and environment commands have no outstanding correctness findings. Full live-handle
discovery remains required by the original goal and explicitly pending in the ledger.
Formatting, build, tests, strict Clippy, rustdoc and pedantic review were run.

## Must fix

None outstanding in the implemented scope. Validation fixed these issues before publication:

- `src/apps/app.rs:405`: an explicit routing snapshot could bypass a discovery child's
  process guard. The guard now takes precedence. A native child test uses a worker
  routing snapshot and verifies rejection before its local handler executes
  (rust-port section 1: preserve side-effect guards).
- `src/apps/discovery.rs:408`: interruption is prioritized when child completion and
  Ctrl-C become ready together. Native interrupted exit codes/signals also stop the
  scan, so a failed child cannot accidentally start later targets after interruption
  (rust-best-practices section 7: owned async lifetimes).
- `src/cli/environment.rs:76`: explicit names bypass even current-directory lookup,
  preserving the source's early return. The directory provider is lazy and tested.
- The environment fixture capture initially left a wrapped mock method installed after
  invocation, breaking an upstream mock assertion. Its patch now restores the original
  method before that assertion. All eight source tests pass unchanged.

## Should fix

None outstanding in the implemented scope.

## Consider

- `src/apps/discovery.rs:142`: Cargo owns module compilation and cache freshness. The
  API exports validated metadata, not live imported Rust handles. Binary entry paths
  identify sources; exact Python import identity, arbitrary module discovery and
  preserved-mtime cache invalidation remain pending. These are recorded gaps in the full
  goal, not silently treated as satisfied by process tests (rust-port sections 1–2).
- `src/apps/discovery.rs:142`: the linear orchestration retains a pedantic length
  advisory. A public-function `expect` was replaced with a typed target error; private
  pipe expectations describe invariants established immediately before spawning.
- `src/cli/environment_tests.rs:21`: four fake async methods retain unused-async
  advisories to keep recorded effects inside future polling. Explicit map construction
  replaced two default-trait advisories. No lint suppressions were added.
- `src/apps/discovery.rs:408`: explicit interruption waits for the direct child;
  dropping a future uses Tokio kill-on-drop. Arbitrary descendant processes launched by
  user code or build scripts are outside that direct-child guarantee. The API documents
  this limitation (rust-best-practices section 7).

## What's already good

- No shell command strings, mutable parent environment, shared global app registry,
  unsafe code or new dependency was introduced. Cargo target metadata is versioned;
  executable paths come from artifact messages, not guessed filenames.
- Each target uses a private export directory. A failed process or invalid declaration
  rejects the entire export, and a prior export cannot survive a fresh scan. Validation
  precedes publication of metadata; first-name deduplication retains deterministic order.
- SDK management helpers retain ordered partial failures and parent retention. The two
  destructive commands preserve their distinct prompts and final messages. A private
  generic cleanup bridge shares progress output with app deletion.
- Actual Cargo projects and both CLI aliases exercise the complete path. An SDK consumer
  additionally verifies real AppRegistry export and automatic name resolution, beyond the
  lightweight protocol fixtures. Local entrypoint output stays absent during discovery.
- Public discovery types have native errors and appropriate traits. Wrapped process/SDK
  errors preserve sources. No live Runpod resources or account credentials are used.

## Verification

- Eight unchanged Python environment tests pass; sixteen transport characterizations
  produce 24 deterministic fixtures, SHA256
  `AD127D882E50549ED8390DF6A9A67425E75B371937BB773F066FE26167CDDA65`.
- Six parameterized Python discovery cases pass, producing nine reference observations,
  SHA256 `ABD8BB8D4FD0ED1EE72F6D22E8C95E1DD5D34B496F71B21E9A89AE660B015729`.
  Their five source test functions and five discovery implementation entries remain
  pending; the observations are not added to Rust differential-fixture totals.
- Nine environment unit tests, three CLI process tests and six discovery tests pass.
  The full Windows suite passes **930 tests**, zero failures and four ignored child
  entry points exercised by parents. Locked build, formatting, strict all-target/all-feature
  Clippy and rustdoc pass. Focused discovery/CLI tests and strict Clippy were repeated
  after review fixes.
- A real SDK consumer built offline using the existing target directory, exported its
  registered app, resolved its name through the CLI and made the expected loopback
  environment query. A Windows PTY interruption rejected partial declarations, returned
  `Aborted!` / exit 1 and did not start the next binary.
- The source pin and tracked/index contents remain unchanged. Previous commit `3b4521c`
  passed Windows, Ubuntu and MSRV CI; its queued macOS job was cancelled. This milestone
  starts a fresh matrix after publication.

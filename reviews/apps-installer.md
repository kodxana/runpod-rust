# Rust review: external installer and environment builds

Library scope: BuildTools, pip/ensurepip/wheel commands, SDK source overlay, BuildOptions,
BuildResult, and build_environment. Edition 2024, MSRV 1.88. Reviewed with rust-port,
rust-best-practices, and rust-review. No blocking finding remains in this scope.

## Preserved contracts

- Linux wheel target flags, platform preference, SDK no-deps installation, runtime/API
  requirements, progress name parsing, last-line diagnostics, and sorted wheel selection.
- Missing pip bootstraps ensurepip. Empty requirements execute no subprocess. Package
  overrides bypass SDK overlay; custom images disable automatic GPU-package exclusions.
- Mixed CPU/GPU builds compute CPU dependency needs before pruning, preserving transitive
  extras and recording only necessary cold-start exclusions. SDK overlay preserves stale
  unrelated files while omitting source bytecode/cache entries.

## Review fixes

- stdout/stderr share a native anonymous pipe, avoiding deadlocks from sequential draining.
  Command's inherited writer handles are dropped immediately after spawn so EOF is visible.
- Progress callbacks have a named BuildProgress alias and retain their original errors.
  The synchronous child guard kills then reaps on early return/unwinding; it never waits for
  a running installer to finish normally during cleanup. Windows tools run without windows.
- BuildResult owns automatic scratch; explicit scratch survives success/failure. Debug on
  BuildOptions hides package override contents. Documentation names the external build-tool
  dependency rather than implying these optional builds never invoke Python.

## Native adaptations

- The explicit interpreter command replaces sys.executable. SDK source overlay takes an
  explicit Python package directory. With no source or package override, builds install
  the exact pinned upstream revision; there is no Python installation to introspect in Rust.
- Source/destination overlap is rejected; native fs::copy metadata differs from copy2.
  Build operations are synchronous and must run on a blocking executor in async workflows.
- Python/pip execute only as external build tools. SDK orchestration, manifests, resource
  APIs, dependency graph analysis, pruning, and archive construction execute in Rust.

## Verification

- All 66 upstream build tests pass on Windows, covering every test function in that file.
- Twenty Rust tests execute local shell/PowerShell stand-ins, including 256 KiB stderr,
  progress failure, bootstrap errors, wheel errors, exact CLI arguments, SDK override and
  pinned-source selection, automatic cleanup, and mixed CPU/GPU/custom-image builds.
- New tests pass on stable and MSRV 1.88. The full Windows suite passes 212 integration
  tests and one doctest. Build, fmt, Clippy with warnings denied, and docs pass. Unix process
  fixtures await this commit's remote CI. No package install or external download was run.

## Consider

- build_environment intentionally retains the source pipeline's visible phase order in a
  single function. Pedantic Clippy suggests splitting its 122 lines; preserving readable
  sequencing is preferable here. A format_collect advisory in the tiny test fixture builder
  is not material to SDK behavior. Other pre-existing pedantic warnings remain.
- Process cleanup is synchronous, including kill/reap on failure. No asynchronous task may
  call the installer directly and expect cooperative cancellation; deployment integration
  must manage its blocking work explicitly. There is no installer deadline in the source.

## What's already good

- Standard-library pipes avoid another process/pipe dependency and work on the declared MSRV.
- Real process tests verify output pressure and failure behavior beyond mocked call lists.
- Configuration and scratch ownership are explicit, with no global mutable SDK state.
- All upstream build functions and test functions now have concrete mappings in the ledger.

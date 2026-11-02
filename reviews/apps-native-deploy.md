# Rust review: native queue/API deployment

Reviewed the compiled-worker deployment path, shared upload pipeline, launcher and tests
under Rust 2024 / MSRV 1.94.1 using rust-port, rust-best-practices and rust-review.

## Must fix

None outstanding. Cold startup must not execute the archive's final worker path before
Rust extracts that same executable: on Linux this can fail with `ETXTBSY`. The launcher
extracts the fixed member to `runpod-native/bootstrap`, then executes it. Full extraction
writes `runpod-native/worker`, preserving the running inode. The Linux process test uses
the actual uploaded executable and resulting endpoint command to exercise this boundary.
Rule: preserve process and resource lifetime invariants (rust-best-practices sections 1, 5).

Tar also interprets a colon in an archive argument as a remote-host specification. The
first Windows/Git-sh cold-start tests exposed this with an absolute local path. The
launcher now opens the file through shell redirection and passes stdin to tar:

```sh
tar -xOzf - runpod-native/worker < "$ARTIFACT" > "$WORKER" || exit 1
```

Paths and worker arguments are quoted; application/resource names are not interpolated
into shell code. No dynamic command evaluation, unsafe Rust or new dependency is added.

## Should fix

None outstanding. Native task declarations are rejected before remote mutations because
the pinned SDK's coordinator task path does not establish a native command contract.
Queue/API deployment uses the existing endpoint `dockerArgs` field. Built-in and custom
images both receive the native command and one authoritative `RUNPOD_RUNTIME_KIND` value.
Rule: make unsupported runtime boundaries explicit rather than report false parity
(rust-port sections 1 through 4).

The original Python-compatible deployment retains its preparation/configuration behavior.
Both paths share the same lookup/create, upload, finalization, activation and endpoint
reconciliation implementation. Artifact ownership persists through upload; finalization
receives exactly the manifest packaged into the uploaded archive. Native preparation
owns its inputs on the blocking pool; cancellation prevents subsequent cloud calls.

## Consider

- ELF validation checks the Linux x86_64 header, including type, endianness and ABI. It is
  not a complete executable validator and cannot guarantee image/library compatibility.
  Inputs must remain stable while packaging. This limitation is in public API docs.
- Native deployment retains `pythonVersion: "3.12"` as the coordinator v1 image-family
  hint. No Python executable or installer runs in the native launcher. Standalone native
  packaging still omits this field; both modes use the same manifest builder.
- The existing built-in image is an OS/library carrier. Custom images require POSIX
  tools, compatible dynamic libraries and an entrypoint that accepts the generated
  command. Local mocks and processes do not establish live Runpod/container compatibility.
- Prelaunch failures use stderr/exit because no native responder can run before the
  binary exists. Once started, Rust retains its existing bootstrap error behavior.
- A cold container keeps one bootstrap copy plus the extracted worker. Warm startup
  executes the worker directly. Call one launcher per worker startup, as required by
  the underlying artifact preparation contract.
- Test shells use `sh` on Unix and Git's POSIX tools found relative to PATH on Windows;
  `RUNPOD_TEST_POSIX_SHELL` is an explicit test-only override.

## Verification

- New deployment tests first failed to compile because the native API/launcher were absent.
- Four native HTTP tests pass: uploaded/finalized manifest and native endpoint payloads,
  preflight rejection, secret validation and failure boundaries. Nine existing deployment
  tests and six existing native artifact tests also pass.
- Five real POSIX launcher tests pass locally through Git sh, including delayed artifact
  mounting, all selection paths, literal arguments and failures before worker execution.
- Seven unchanged source tests pass: four deployment pipeline, two manifest and one
  custom endpoint-input case. Source repositories remain at their pinned commits with
  no tracked/index changes. No cloud resources were created.
- Linux x86_64 CI additionally runs the uploaded compiled worker through the generated
  endpoint launcher and Rust bootstrap, then checks its HTTP response using a bundled asset.
  This test is gated by the deployment target and cannot execute on the Windows host.
- The full Windows suite passes **1,074 tests**, zero failures, with eight ignored child
  entry points exercised by parent tests. Locked build, formatting, strict all-target/
  all-feature Clippy and rustdoc pass. Focused native tests pass after final review edits.
- Pedantic review found two new minor warnings: backticks around `x86_64` in public docs
  and an empty-output assertion that should display bytes on failure. Both are fixed.
  The final scoped review retains only two pre-existing display-only integer-to-float
  warnings in the shared deployment code; artifact size enforcement uses integer bytes.
- Header read failures now preserve native I/O causes except truncated headers, which
  correctly report invalid target input. Debug output identifies native mode without
  exposing worker credentials. No new SDK inventory entries are credited.
- The separate Windows fixture-readiness correction (`c735405`) passed all four CI jobs
  (Windows, Linux, macOS and MSRV). This feature commit triggers its own full matrix.

## What's already good

The native path is explicit and leaves source-compatible behavior in shared, tested code.
Tests inspect real archive bytes and subprocess output. Native secrets, symbols and target
constraints fail before mutations; cloud failure ordering remains visible and documented.
The parity inventory does not claim new symbols for these already-mapped adaptations.

# Rust review: app management CLI

Reviewed the application code in `src/cli/flash.rs`, root registration, tests and parity
capture against Rust 2024 / MSRV 1.94.1. It adds native app inspection and deletion using
the existing SDK lifecycle. No outstanding correctness or API findings remain. Tests,
formatting, build, strict Clippy, rustdoc and a pedantic review were run.

## Must fix

None outstanding. During validation, the first REST failure capture had inconsistent
mocked error text and response JSON. The generator now uses Python's actual REST error
parser with a problem-detail response; the native wire comparison passes without changing
production transport behavior (rust-port sections 1–2: characterize the original).

## Should fix

None outstanding.

## Consider

- `src/cli/flash.rs:138`: cleanup progress is plain text emitted on the CLI task. The
  observer owns only a channel sender; draining queued messages before reporting the
  result preserves event order even when completion and the final event share a poll.
  Rich animation and console classes remain explicitly pending (rust-port section 4).
- `src/cli/flash.rs:104`: interruption drops the future and stops subsequent requests;
  already submitted remote mutations may still complete. There is no detached deletion
  task or implicit rollback. This matches the existing SDK cancellation contract
  (rust-best-practices section 7: owned async lifetimes).
- `src/cli/flash_tests.rs:23`: three pedantic unused-async advisories remain on fake
  management methods. Their recorded side effects occur when polled, matching native
  async methods. Replacing them with eagerly evaluated `ready` futures would change that
  timing. Test-only allocation, borrowing, unit-pattern and empty-assert advisories were
  fixed; no lint suppressions were added.

## What's already good

- The command delegates to the tested SDK management helpers. It preserves ordered
  best-effort cleanup, parent retention after child failures, lookup aborts and the
  source's false-confirmation quirk without duplicating lifecycle logic.
- Confirmation precedes credentials and network access. Both CLI aliases exercise real
  loopback transports, checking complete request bodies, encoded paths, headers, request
  counts, stdout, stderr and exit status.
- Rendering handles missing collections and Unicode build truncation; malformed metadata
  returns typed errors without reachable production unwraps or external-data indexing.
- No public API, dependency, unsafe code or live service operation was added. About
  224 nonblank Rust lines cover the roughly 90-line Python command group, including the
  explicit async/error boundary and progress bridge.

## Verification

- Seven unchanged upstream tests pass. Fifteen additional transport characterizations
  produce 22 byte-identical captures, SHA256
  `019C466C42ED0171E40A8AABBC575EF89CA0F7FE89BA424694E76ECE0E5063F4`.
  The pinned source checkout and index remain unchanged.
- Eight unit tests and two process tests pass. The complete Windows suite passes
  **912 tests**, zero failures and three ignored child entry points exercised by parents.
  Locked build, formatting, strict all-target/all-feature Clippy and rustdoc pass.
  Focused tests and strict Clippy were repeated after test-only lint fixes.
- Windows terminal checks: accepted confirmation makes ten verified requests and exits 0;
  declined confirmation makes none and exits 1; Ctrl-C during the first endpoint deletion
  stops after four requests and exits 1 with `Aborted!`. The loopback server observes the
  cancelled connection; no later deletion is submitted.
- The previous login commit passed Windows, Ubuntu and MSRV CI. Its queued macOS job was
  cancelled, so it is not described as fully green. This commit gets a fresh CI matrix.
- Only the app group and its three commands are mapped. The broader flash command group,
  local discovery, native deployment and remaining console work are still pending.

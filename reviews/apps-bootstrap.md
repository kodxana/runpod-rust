# Rust review: artifact bootstrap loading

Reviewed the library's artifact location/extraction, dependency paths, manifest reader,
phase error type and tests under Rust 2024 / MSRV 1.94.1. No outstanding correctness
findings remain in the implemented portable-artifact contract. Full worker bootstrap
orchestration and native deployment are still required by the original goal. Build,
tests, formatting, strict Clippy, rustdoc and a scoped pedantic review were run.

## Must fix

None outstanding. Validation fixed:

- `src/apps/bootstrap.rs:239`: tar's `unpack_in` silently accepts a regular-file entry
  named `.` without writing anything. The bootstrap could then mark extraction complete
  (and on Unix change root-directory permissions). Regular files now require a normal
  path component. The Python characterization failed natively before the fix and passes
  afterward. **Rule:** preserve filesystem side effects and failure behavior
  (rust-port sections 1–2).
- `src/apps/bootstrap.rs:267`: source data filtering makes every regular file owner
  writable. Windows extraction now explicitly clears the readonly attribute, permitting
  later duplicate entries to overwrite a mode-zero file as Python does. Unix uses
  explicit mode filtering and never uses `set_readonly(false)`.

## Should fix

None outstanding in this scope.

## Consider

- `src/apps/bootstrap.rs:130`: marker trust and partial-extraction semantics are retained.
  The source even trusts an embedded marker left by a failed extraction on the next
  start. This is recorded as a preserved source quirk, not silently repaired.
  **Rule:** preserve ambiguities/bugs and document them (rust-port section 1).
- `src/apps/bootstrap.rs:198`: portable native archives contain files and directories,
  as emitted by the SDK packager. Links/special entries and nonportable or escaping
  paths are rejected. Some Python 3.12+ data-filter inputs are consequently restricted;
  this difference is explicit, rather than presented as universal tar compatibility.
- `src/apps/bootstrap.rs:130`: location is intentionally blocking, matching source
  ownership and polling. Async callers must use a blocking executor; dropping their
  wait does not stop a running extraction. Concurrent extraction into one destination
  is outside the API's contract, as it is in the source.
- `src/apps/bootstrap.rs:271`: a statement-scoped expected Clippy lint documents the
  Windows readonly operation. The lint warns about Unix world-write permissions, but
  this statement is compiled only on Windows; Unix uses `PermissionsExt::from_mode`.
  **Rule:** narrowly justify a false-positive lint instead of broadly disabling it
  (rust-best-practices section 2). No new pedantic warnings remain in the changed files.

## What's already good

- Owned configuration replaces mutable module globals; phase errors retain native IO/
  JSON causes and preserve the source's structured error payload. No public panic path
  or unsafe code was introduced. Deadline checks avoid overflowing an Instant addition.
- Paths are validated before extraction, including an existing destination ancestor's
  resolved location. Entries remain ordered, so duplicate and partial-failure semantics
  are observable and characterized. Unix permissions remove privilege/write bits while
  retaining executability and owner access; directory modes are ignored.
- Compression uses safe interfaces to already-locked codecs. Cargo.lock gains only two
  direct dependency edges; no resolved package/version was added or upgraded.
- Tests compare unchanged source behavior and use actual SDK-generated archives, with
  binary bytes, project files and vendored files. No Python bridge is used in native code.
- Runtime coverage stays separate from SDK symbols/test counts. Resource selection,
  package setup, native launch and bootstrap error servers remain explicitly pending.

## Verification

- Ten unchanged source tests pass. The source pytest run uses a fresh ignored workspace
  basetemp because the existing system pytest temp root denies access; no source edits
  or changes to that system directory were needed.
- Thirty-three captured cases regenerate identically, SHA256
  `1B4B7D03DE2AE16BE9FE27962329A6D7223019E6F98F1A85A64C59E657419D02`.
  This includes raw/gzip/bzip2/xz, GNU/PAX long names, partial/warm behavior and malformed
  values. Native exceptions are compared by failure/phase, not interpreter-specific text.
- Sixteen Windows bootstrap tests pass. Two additional Unix-only tests cover symlink
  escape and file modes in CI; those platform-specific branches are not claimed as
  locally executed on Windows.
- Full Windows suite: **974 passed, zero failed, four ignored child entry points exercised
  by parents**. Locked build, fmt, strict all-target/all-feature Clippy and rustdoc pass;
  focused bootstrap tests and strict Clippy were repeated after review changes.
- Both source checkouts remain pinned and have no tracked/index modifications.

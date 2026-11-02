# Rust review: deployed resource loading and mounts

Reviewed the new library runtime module, application identity helper, mount diagnostic
changes, compiled example and tests under Rust 2024 / MSRV 1.94.1. The implemented
selection/startup contract has no outstanding correctness findings. Full deployment,
task HTTP serving and live dispatch remain part of the original goal. Formatting, build,
tests, strict Clippy, rustdoc and scoped pedantic review were run.

## Must fix

None outstanding. Source captures identified two mount-validation differences corrected
in `src/apps/volume.rs:248`: non-object records require their own diagnostic, and missing
or non-string paths must report path validation rather than missing kind/reference/ID.
**Rule:** preserve observable failure behavior (rust-port sections 1-2). The source-capture
test checks both cases and verifies that malformed JSON retains its native cause.

## Should fix

None outstanding. `src/apps/runtime.rs:39` reads only the eight relevant settings using
fallible Unicode conversion. It does not enumerate all environment values, retain
credentials, or risk the unrelated non-Unicode-variable panic of `std::env::vars`.
**Rule:** avoid reachable public panics and keep state owned (rust-best-practices sections
3 and 5). Non-Unicode worker settings are documented as absent.

## Consider

- `src/apps/runtime.rs:113`: queue/API first-match selection intentionally differs from
  strict task selection. Queue loading accepts a task handle; manifest owner/kind/qualified
  name do not constrain it. **Rule:** retain source quirks (rust-port section 1).
- `src/apps/runtime.rs:195`: native callbacks and explicit symbol metadata replace module
  reflection. A registry must expose only its module's declarations and finish registration
  before returning. Arbitrarily interleaved Python module attributes are represented by
  ordered app/resource registration, not by a Python-object emulation layer.
- `src/apps/runtime.rs:219`: alias detection compares application identity, not app names.
  Reusing one app is harmless; distinct same-named apps can be ambiguous. No user hook is
  invoked under a registration lock or before uniqueness is known. Task initialization
  inherits NativeFunction's documented sync-blocking and generator-collection adaptations.
- New pedantic findings were three test uses of inferred `Default::default`; they now use
  explicit `serde_json::Map::new`. No lint suppression or additional dependency was needed.

## What's already good

- Private owned context holds immutable settings and mounts; callers can run independent
  contexts without process environment mutation. Closures own the mounts they need.
- Task identity validation precedes manifest access and module loading. Attachment errors
  preserve phases; callback and initialization errors retain their typed native causes.
- Tests cover selection ordering, missing handles, wrong app/kind/qualified name, alias
  deduplication, ambiguity without initialization, and mount configuration before declarations.
- Real local queue transport verifies streamed chunks plus final aggregate. Real HTTP
  verifies readiness, initialized state and mounted paths. Async task init gates loading
  and leaves usable state for execution on the same runtime.

## Verification

- Twenty-one unchanged source cases (thirteen functions) pass against the pinned runtime.
  Two additional unchanged startup tests fail their POSIX path string expectations on
  Windows (`\\shared` / `\\data`); they are recorded as platform adaptations, not passes.
- Fifty-seven source captures regenerate deterministically with explicit LF output, SHA256
  `7C782AEB852C424CD6D953E34AF28D19CAB85A9AFC8554E8013BD3E7B59FD546`.
- Twenty new native tests pass. The complete Windows suite passes **994 tests**, with zero
  failures and four ignored child entry points exercised by parents. The focused tests
  and strict Clippy were repeated after the test-only pedantic cleanup.
- Locked build, fmt check, strict all-target/all-feature Clippy and rustdoc pass. Pedantic
  findings outside the changed code are pre-existing and outside this review.
- The compiled example was exercised against a local queue server: exact output includes
  the expected greeting and `/data` mount. A separate discovery-mode invocation exported
  matching symbol metadata without serving another job.
- Source checkouts remain pinned and have no tracked/index modifications. SDK inventory
  counts remain unchanged; this milestone's coverage belongs to the separate runtime ledger.

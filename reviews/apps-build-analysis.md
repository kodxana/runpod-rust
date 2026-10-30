# Rust review: dependency selection and pruning

Library scope: requirement collection/exclusion, installed wheel metadata, dependency
closure, and pruning. Edition 2024, MSRV 1.88. Reviewed with rust-port,
rust-best-practices, and rust-review. No blocking finding remains in this scope.
Dependency installation, wheel builds, SDK overlay, and orchestration remain pending.

## Preserved contracts

- First-named requirement wins, while the original requirement string is retained.
  Selection names and installed package canonicalization intentionally differ as upstream.
- Closures reprocess nodes when new extras arrive, terminate on cycles, evaluate each
  extra independently, and include the empty-extra context. Non-extra platform/version
  conditions are conservatively true. Missing/invalid metadata fails closed.
- Pruning distinguishes image-provided dependency trees from explicit user exclusions,
  retains shared files and namespace directories, and preserves unrelated packages.
  Missing RECORD falls back to the import-name directory only for the excluded package.

## Review fixes

- `src/apps/build.rs::normalized_record_path`: a failing regression showed that stripping
  a leading slash could reinterpret an absolute record as a relative deletion target.
  Reject rooted/drive records before normalization. This follows rust-port's requirement
  to test observable edge behavior and rust-review's correctness-first rule.
- Every file/tree deletion checks resolved containment within the canonical environment;
  directory deletion rejects symlinks. Relative pip staging paths remain supported.
  Unix tests cover outside file, directory, and dist-info links, plus broken local links.
- Native PEP 508/CSV libraries handle syntax instead of implementing Python parsers.
  Malformed/extreme syntax follows the native libraries and is documented in PORTING.md.

## Verification

- 73 captured Python cases compare results and exact remaining file/directory trees.
- All 34 matching upstream Python tests pass. Each of their 23 test functions has a
  named Rust counterpart, with parametrized cases supplied by the captured fixtures.
- 29 new Windows tests pass on stable and Rust 1.88. The full stable suite passes
  192 integration tests and one doctest. Build, fmt, Clippy with warnings denied, and
  documentation generation pass. Pedantic review reports no warnings in new files.
- The Unix containment test awaits this commit's remote CI. No real environment was
  pruned: deletion tests operate exclusively on newly created temporary wheel trees.

## Consider

- Like upstream, cleanup of dropped distribution directories ignores deletion errors;
  file removal and metadata reads propagate errors. Returned distribution names describe
  the selected removal set, not an audit that every directory deletion succeeded.
- Filesystem containment checks assume a stable local build tree, as upstream does;
  this is not an atomic filesystem sandbox against concurrent hostile mutation.

## What's already good

- Explicit options and ordered collections keep policy and observable output deterministic.
- File/dependency ownership stays native; no local Python is invoked for analysis.
- Known parse/IO errors retain structured causes, with conservative metadata fallbacks.
- Shared-file and transitive-extra tests exercise realistic wheel installation layouts.

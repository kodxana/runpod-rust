# Rust review: project artifact packaging

Library scope: project/dependency archive selection and artifact ownership, edition 2024,
MSRV 1.88. The rust-port and rust-best-practices skills guide the native implementation;
rust-review guides the checks. Complete dependency builds and deployment orchestration
remain pending. No blocking finding remains in the reviewed packaging implementation.

## Preserved contracts

- Sixteen Python fixtures compare ordered member paths, file bytes, and parsed manifests.
  They cover root/nested ignore precedence, negation, anchored patterns, escaped literals,
  pruned directories, protected credentials, Unicode, dependency namespaces, and output
  exclusion. The seven upstream packaging tests have explicit Rust test mappings.
- Symlinks never become source or dependency members, and symlinked ignore files cannot
  affect selection. Unix tests additionally preserve executable mode. Hard-linked files
  appear as independent regular members, and binary content/long filenames round-trip.
- Vendored dependencies bypass the source filters, preserving CA certificates and package
  data. Protected source exclusions remain effective even with user negations.

## Review fixes

- Automatic output storage is retained by Artifact until use completes. Explicit persist
  transfers deletion responsibility to the caller; dropping explicit-output artifacts does
  not delete caller files. This follows owned-resource cleanup in rust-best-practices.
- The exclusion-log test passed in isolation but missed tracing events when sharing a test
  binary with concurrent packagers. It now has a separate integration binary and subscriber;
  production filtering and parallel archive tests are unchanged.
- Removed a trailing-space filename from shared fixtures: Windows trims it while Unix
  retains it. Portable escaped-space patterns remain tested. Native filesystem semantics
  are documented rather than imitating another platform's filename restrictions.

## Consider

- `src/apps/package.rs` uses native tar/gzip metadata and UTF-8 JSON. Consumers must compare
  archive contents instead of compressed bytes. The Python packager also has varying file
  metadata and gzip timestamps, so byte reproducibility is not an upstream contract.
- IO errors preserve their original cause through Error::Io. More operation/path context
  could improve diagnostics consistently across the SDK in a later review.

## Verification

The full Windows stable suite passes 163 integration tests and one doctest. All twenty new
packaging tests also pass on Rust 1.88. Build, fmt, normal Clippy with warnings denied, and
documentation generation pass. The separate pedantic review has no packaging-file warnings;
existing advisory findings elsewhere remain. Unix symlink/mode tests require remote CI.
The matching Python subset passed six tests and failed only the symlink-creation test
because this Windows session lacks the required privilege.

## What's already good

- No Python process, unsafe code, global configuration, or mutable global archive state.
- Native gitignore/tar/gzip crates handle matching and serialization; file data is streamed.
- The implementation and tests distinguish source exclusions from dependency inclusion.
- Separate semantic fixtures and filesystem tests avoid relying only on self-round-trips.

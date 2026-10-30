# Rust review: artifact and deployment orchestration

Library scope: explicit secret/source metadata, build_artifact, deploy_app, temporary artifact
ownership, and size enforcement. Edition 2024, MSRV 1.88. Reviewed with rust-port,
rust-best-practices, and rust-review. No blocking finding remains in this scope. Native App
registration, discovery, and development workflows remain separate unfinished work.

## Preserved contracts

- Validate resource specs and explicit secret objects before building. Literal strings
  resembling secret templates are not inferred to be typed secret references.
- Vendor, manifest, package, size check, app/environment lookup/creation, upload, finalization,
  activation, and endpoint reconciliation execute in source order. Existing environments
  are reused, environment names reach nested worker calls, and endpoint-ready events sort
  by resource name. The uploaded manifest equals the finalized manifest byte-for-byte after
  JSON decoding. Resource result order remains the manifest's order.
- Failure does not roll back completed remote mutations. Failed finalization/activation
  prevents all endpoint provisioning. Size validation uses exact integer bytes, accepting
  the limit and rejecting one extra byte, while retaining the rounded upstream diagnostic.

## Review fixes and native adaptations

- ResourceDefinition stores explicit referenced_secrets and provides set_environment to
  render typed values without losing reference metadata. This avoids guessing from strings.
- Blocking build/package work owns its metadata and storage inside spawn_blocking. The
  async executor remains responsive; canceling the await prevents later cloud calls while
  the already running build completes locally. A blocking task panic retains its JoinError
  through Error::BackgroundTask. No extra raw thread or global mutable state is introduced.
- Automatic build and archive storage remains alive until its final consumer returns.
  Explicit output paths remain caller-owned. Module paths use explicit source metadata,
  with fallback for absent/inaccessible/outside-project paths rather than Python reflection.
- All observation callbacks follow EventSink's error suppression, including progress and
  endpoint-ready callbacks invoked directly by Python. Lower-level BuildTools still exposes
  fallible progress callbacks for callers that need observer failure to abort installation.

## Verification

- Four matching upstream deployment pipeline tests pass. Nine new Rust integration tests
  exercise real archive uploads against local HTTP services and real stand-in build processes.
- Tests check anonymous upload authentication, finalized manifest content, byte size, mutation
  order, existing environment reuse, worker env propagation, secret validation, failure
  boundaries, explicit output retention, source module paths, executor responsiveness, and
  cancellation after installer progress but before any cloud operation.
- Full Windows stable suite: 221 integration tests, one unit test, one doctest. New pipeline
  and size-boundary tests also pass on Rust 1.88. Build, fmt, normal Clippy, and docs pass.
  Cross-platform pipeline tests await this commit's CI. No cloud resources were touched.

## Consider

- Pedantic Clippy flags the 107-line orchestration function; keeping the ordered phases
  together remains readable. Size-to-f64 conversions affect display only, while the limit
  check is integer-based. Valid artifact sizes are exactly representable at this scale.
- Canceling a build does not forcibly interrupt its synchronous installer. Callers must
  keep build inputs stable until local build work finishes; this behavior is documented.

## What's already good

- HTTP tests inspect actual uploaded archive bytes instead of only asserting mock call names.
- Temporary storage and blocking task inputs are owned, preventing dangling paths or borrows.
- Source metadata, credentials, environment selection, and callback behavior have explicit APIs.
- Preflight failures are tested to cause neither local subprocesses nor remote mutations.

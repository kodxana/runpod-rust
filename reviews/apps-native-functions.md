# Rust review: native App functions and invocation routing

Library scope: App/AppRegistry, execution context, native queue/task registration, async
function handles, and submitted job ownership. Edition 2024, MSRV 1.88. Reviewed using
rust-port, rust-best-practices, and rust-review. No blocking finding remains in this scope.
API route registration, native binary packaging/runtime integration, discovery sessions,
and synchronous function handles are still pending and are not claimed as implemented.

## Preserved contracts

- Registration order, validation before mutation, duplicate rejection, default resource names,
  queue/task kind selection, worker precedence, and explicit development-target lookup.
- Worker-local execution only for the owning resource; sibling calls retain sentinel headers,
  environment overrides, named development endpoints, and successful lookup caching.
- Queue options stack without mutating the original handle. Tasks reject job options and ID
  reconnection. Spawn returns owned queue/task jobs; reconnect performs no queue submission.
- Discovery guards run before remote side effects, including worker-local shortcuts. Streaming
  checks task/generator eligibility and submits lazily. Startup hooks never run for local calls.
- Context identity, source/secret metadata, reserved schedule behavior, and generator ordering
  have explicit tests. New code does not mutate process environment or global registries.

## Review fixes

- Kept large destination/job variants behind private boxes instead of exposing allocator
  wrappers in public signatures (Clippy large_enum_variant; ownership/API guidelines).
- Native function callbacks execute outside registry locks. Replacing an initialization hook
  also releases the lock before dropping user captures, whose destructors may access the app.
- Sync bodies use the caller's Tokio blocking pool; a single-thread executor test proves it
  can release a waiting body. JoinError preserves panic causes. No raw threads are introduced.
- App/handle Debug output omits bodies, credential configuration, and per-job option values.
  Send + Sync are checked at compile time. Snapshot getters cannot mutate resource identities.

## Adaptations and limits

- Rust uses explicit owned registration and JSON argument bags instead of decorators,
  reflection, and Python argument binding. Async local calls collect generators; local_stream
  exposes chunks. Rust bodies provide their own defaults/argument validation.
- Handle clones share registered hook metadata; Python copies the current hook attribute
  when binding options. The Rust registration model is intentional and documented.
- In-progress synchronous bodies finish after cancellation. Async/streaming body panics use
  normal Rust panic semantics; sync panics return the original Tokio task error.
- Existing remote live/task workers still consume explicit Python source. Native Rust body
  execution is implemented locally; native binary deployment remains separate pending work.

## Verification

- Eighteen new native integration tests exercise actual local HTTP services and native handlers.
- Upstream App/context/dispatch subset: 65 passed, one Windows mount-path failure. This is
  the same PurePath/PosixPath platform issue already documented for volume-related tests.
- Formatting, normal Clippy, build, docs, and the full stable suite pass (239 integration,
  one unit, and two doctests). All eighteen native tests pass on Rust 1.88. Pedantic review found no further correctness
  issue; advisory explicit Map::default spelling in tests is left consistent with the suite.

## What's already good

- Credentials remain lazy: declaration and local execution work without a Runpod API key.
- Tests inspect routing headers, payloads, submission counts, streamed output, and cleanup.
- Owned registries and metadata snapshots avoid public lock guards and mutable globals.

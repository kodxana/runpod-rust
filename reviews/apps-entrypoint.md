# Rust review: local entrypoint registration

Library scope: an owned registry, async execution, and a synchronous runtime facade. Reviewed
with the Rust porting, best-practices, and review skills. No blocking finding remains. The
implementation delegates execution to NativeFunction and adds no global loop or thread state.

## Must fix

None. Registration does not execute code; returned and retained callables share captures.
Registry clearing is isolated. The blocking runner rejects nested runtimes before executing.

## Consider

- Owned registries replace Python module globals. Separate async and blocking runners replace
  its callable/awaitable detection and global loop bridge. Streaming functions collect their
  results into a native JSON array; Python's arbitrary return-object identity is not recreated.
- Blocking execution owns a runtime for one call. Work must await its child tasks, and async
  callers should use the native async runner. Synchronous bodies use the existing blocking pool.
- Discovery and CLI integration remain pending and are not credited by these unit contracts.

## Verification

Four tests check lazy/latest selection, empty/isolated registries, retained state after clear,
sync/async execution, error propagation, stream adaptation, and nested-runtime rejection.
Stable/MSRV and lint results are recorded in PORTING.md. No dependencies or unsafe code added.

## What's already good

The registry owns only native callables and exposes a borrowed latest entry. Captured state
stays redacted through NativeFunction's Debug implementation; there are no new locks or globals.

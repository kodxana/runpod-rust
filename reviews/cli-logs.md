# Rust review: shared pod and endpoint log commands

Application code, edition 2024, MSRV 1.94.1. The new commands reuse the existing SDK
SSE reader for snapshots and cursor-aware follow mode. The SDK public API, dependencies
and compact build profile are unchanged. Review found and fixed a missing reconnect path
for a server disconnecting before response headers. Required checks are recorded below.

## Must fix

### 1. Retry a disconnect before response headers (`src/cli/logs.rs:366`)

**Rule:** Preserve failure behavior and retry boundaries (rust-port sections 1 and 2;
rust-review correctness/error handling).

**Problem:** The initial classifier handled connect/timeouts and HTTP 429/5xx. Reqwest
reports an accepted connection disappearing before headers as a request error, so follow
mode returned an error instead of reconnecting. A real TCP regression reproduced the
failure after receiving a cursor from the first connection.

**Fix applied:**

```rust
matches!(error, Error::Http(error)
    if error.is_connect() || error.is_timeout() || error.is_request())
```

The regression now traverses a successful stream, a pre-header disconnect, HTTP 503,
and a successful resumed stream. All reconnect requests carry the original cursor;
cancellation closes the final socket. Snapshot mode still propagates transport failures,
and payload/decoder errors do not enter the retry path. No correctness finding remains.

## Should fix

None outstanding. Owned streams and discovery futures stay within the observer future;
dropping it cancels all work without joining detached tasks. A watch channel carries the
SDK cursor callback without holding a guard across an await. Endpoint and pod targets
use an enum, while the independent output/follow selections remain private options.

## Consider

- `src/cli/logs.rs:186`: stream ownership replaces Python task ownership. The extra outer
  stream carries worker completion separately from events, allowing source-compatible
  error propagation at the next worker-discovery tick. This is deliberately private.
- `src/cli/logs.rs:331`: RFC3339 parsing, string-shaped human log records, compact UTF-8
  JSON and native HTTP diagnostics are documented adaptations. Snapshot sorting still
  uses the original timestamp text, and an event's worker key overrides the prefix key.
- Pedantic review's production clone-assignment advisory was resolved with struct update
  syntax. Remaining test-only advisories concern a 102-line process fixture verifier,
  small string formatting allocations, an async fake trait implementation, and empty
  collection assertions. These do not affect correctness; no suppression was added.
  Strict default Clippy passes. Rule: proportionate testing and review (rust-port §4).

## What's already good

- Named counterparts cover all shared-log source tests and both command option routers.
  Virtual time prevents retry/discovery checks from depending on wall-clock timing.
- Real socket tests prove cancellation, idle timeout and the live snapshot cutoff close
  connections. A terminal check verifies the actual Ctrl-C handler exits successfully.
- Process fixtures check encoded IDs, filtering, duplicate IDs, padded worker labels,
  headers/query arguments, raw timestamp sorting, worker-key precedence and JSON output.
- HTTP 404 behavior distinguishes pod requests from disappearing endpoint workers;
  authentication errors propagate, and failed snapshots discard buffered output.

## Verification

- All 39 selected unchanged Python cases pass. Fourteen snapshot fixtures regenerate
  byte-identically; the pinned Python checkout remains unchanged.
- Nineteen native unit tests and three process tests cover this milestone, including
  the failing-then-passing pre-header disconnect regression.
- The final full Windows suite passes **818 tests**, zero failures and three ignored
  child entry points exercised by parent tests, including two passing doctests.
  Locked build, formatting, strict all-target/all-feature Clippy and rustdoc pass.
- A Windows PTY run of `endpoint logs --follow --json` emitted a worker event, refreshed
  discovery, and exited with child status 0 on Ctrl-C. The mock observed stream closure;
  PowerShell itself returned 1 after receiving Ctrl-C. All calls were loopback GETs.
- The preceding commit `cf114754b0ec1abddd2308b461e42e3010f09ad6` passed Windows,
  macOS, Ubuntu and MSRV CI. This commit's CI starts only after publication.

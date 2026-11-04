# Rust review: independent worker heartbeats

Scope: library changes in `src/serverless/heartbeat.rs`, queue/HTTP worker integration,
transport configuration and heartbeat tests. Edition 2024, MSRV 1.94.1. This ports the
pinned Python heartbeat to an explicitly started, owned native thread and Tokio runtime.
The HTTP client belongs to that runtime; no pooled connections cross runtime lifetimes.

## Must fix

No unresolved findings after verification. Review caught credential-bearing request URLs
in exhausted-retry errors; these are stripped while preserving the HTTP status. It also
caught cleanup errors masking the original worker failure; primary failures now survive.

## Should fix

No unresolved findings in scope.

## Consider

- `heartbeat.rs`, background startup: Rust best practices normally leave execution to
  library callers. Here callers explicitly opt into an owned background executor through
  `start_ping` or worker startup; this is needed to retain Python's independent heartbeat
  behavior. `ping_loop` remains available on the caller's executor. A native thread does
  not provide Python's process crash isolation.
- `HeartbeatOptions::pool_connections` uses reqwest's per-host idle-connection limit;
  Python also limits cached host pools. One heartbeat URL makes the difference largely
  incidental. Native TLS, proxies, default user-agent and rare malformed-response/redirect
  behavior remain reqwest behavior. This is not a complete requests/urllib3 emulation.
- Retry-After uses the HTTP date parser (including obsolete HTTP formats) and the captured
  urllib3 2.8.0 six-hour cap. Exotic non-HTTP date formats accepted by Python's email parser
  are not emulated. No new downloaded crate is needed: `httpdate` was already transitive.
- A cloneable bounded mirror replaces OS process-shared memory. Start suppression is
  per owned Heartbeat instance; normal worker shutdown drains jobs before stopping pings.
  Explicit low-level send errors remain inspectable, while periodic loops log and continue.
- Environment parsing floors milliseconds as Python does. Zero results fail at native
  construction. Explicit Rust durations retain subsecond precision for embedded workers.
- Pedantic Clippy suggests printing the API-key value when its empty-value test fails
  (`tests/heartbeat.rs`, `assert_is_empty`). The boolean assertion intentionally avoids
  putting credential values in failure output. There are no new library warnings.

## What's already good

- Requests, retries and sleeps are cancellable. Drop signals without blocking or panicking;
  explicit stop awaits resource cleanup, including on startup failure and cancellation.
- The mirror is read once per logical ping, preserving snapshots through retries. No user
  callback runs under its lock, and the heartbeat never locks the main tracker.
- Real HTTP source captures cover retries and accepted nonretryable errors; separate tests
  deliberately stall single-thread queue and HTTP runtimes and observe continuing pings.
- Default retry/pool reconstruction in the child is preserved and tested, including the
  surprising loss of custom parent options.

## Verification

The 15 unchanged upstream heartbeat tests pass. Native tests were written before the
implementation and initially failed for missing Heartbeat APIs. Nine captured source cases
are replayed against the native client. All 91 integration-test targets, unit tests and
doctests complete with **1,138 passed, 0 failed and 9 intentional skips**. The original
full-suite process was interrupted after 75 completed targets (960 passing tests); the
remaining 18 targets and doctests were run individually, with coverage checked against
every integration-test file. Source code was unchanged between these runs.

`cargo build --locked`, `cargo fmt --all -- --check`, strict all-target/all-feature Clippy,
`cargo doc --locked --no-deps`, and the scoped pedantic review pass. Local logs are under
`test-results/heartbeat-*`; `heartbeat-verification.json` records the aggregate and target
list. No live Runpod resources or paid workloads were used.

Timeout design uses separate connect and per-read limits, each twice the heartbeat interval,
matching requests' scalar timeout intent; see the [reqwest timeout documentation](https://docs.rs/reqwest/0.12/reqwest/struct.ClientBuilder.html#method.read_timeout).

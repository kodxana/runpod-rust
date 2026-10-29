# Rust review: Apps invocation and external resource clients

Scope: new library protocol, queue transport, targets, jobs, stubs, and blocking Apps
facades. Reviewed using rust-review and rust-best-practices against edition 2024 and
declared MSRV 1.88. Mechanical checks ran on Rust 1.99.1. No unsafe code was introduced.

## Must fix: resolved

### Endpoint IDs could alter load-balancer routing

Rule: validate external values before constructing trusted addresses (C-VALIDATE;
correctness and invariant preservation). Interpolating an arbitrary endpoint ID into
an authenticated URL allowed separators to alter its host/path. QueueClient now rejects
IDs outside ASCII letters, digits, hyphens, and underscores. A regression test checks
empty IDs and authority/path/query/fragment injection. Explicit service-root overrides
continue to validate HTTP(S) URLs without credentials, queries, or fragments.

### Debug exposed bound storage credentials and environment values

Rule: public diagnostics must preserve the client's credential-redaction invariant.
Derived Debug on Queue and Target printed S3 credentials, webhook URLs, and attached
resource environment values. Custom Debug implementations now omit those payloads;
JobOptions also redacts the sensitive option fields. Regression tests verify redaction.

## Should fix: resolved

### Zero HTTP timeout differed from upstream

Rule: preserve source behavior, including edge cases (rust-port). Aiohttp timeout zero
disables the HTTP deadline, whereas reqwest's zero duration expires immediately.
The shared client pool no longer applies a hidden default deadline: existing REST and
GraphQL methods still set their configured per-request timeout, and Apps omit the
timeout only when it is zero. Tests distinguish this behavior from zero job-wait time.

### Blocking adapter test used a runtime-entered thread

Rule: tests must model the documented public execution context. Tokio spawn_blocking
still enters a runtime, so the existing blocking-client construction guard correctly
rejected that fixture. The corrected test runs the client on an ordinary test thread
and hosts the mock service on a separately owned runtime. It verifies that jobs and
iterators remain usable after their creating client is dropped.

## Validation and remaining work

- Protocol fixtures compare 51 upstream helper/serialization cases. Python also loads
  all 14 values actually encoded by Rust; Unicode is read explicitly as UTF-8.
- Real TCP disconnects, HTTP timeouts, 404 propagation, 503 exhaustion, and invalid JSON
  exercise distinct error paths. Submission is not replayed on ambiguous failures.
- Source synchronization intentionally holds an async mutex across upload to coalesce
  callers. Cancellation tests show that its guard releases and success is not cached.
- Normal Clippy with warnings denied, formatting, full tests, build, and docs are the
  commit gates. The separate pedantic pass still reports style advisories (including
  must_use, long functions, and Python-compatible names); these are not all resolved.
- Live worker monitoring/log feeds and native Rust function deployment remain pending.
  Explicit module source and Serde values replace Python reflection/object execution;
  see PORTING.md for the boundary. No claim of full SDK parity is made yet.
- Declared MSRV and remote Linux/macOS CI remain final-audit tasks.

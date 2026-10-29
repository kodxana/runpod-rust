# Rust review: pod logs and live worker monitoring

Library scope: new log/monitor modules, live-target integration, and the stream client's
HTTP pool configuration. Edition 2024; declared MSRV 1.88. No unsafe code is used.
The rust-port, rust-best-practices, and rust-review skills guided the implementation.

## Must fix: resolved

### Socket timeout did not cover response headers

Rule: preserve transport timeout behavior and failure boundaries (rust-port).
The first stream implementation timed individual body reads but could wait one hour
for headers, while Python's socket timeout is sixty seconds. A regression test uses a
connected TCP server that withholds headers and advances Tokio's virtual clock. It
failed before the fix. A dedicated stream pool now applies reqwest's read timeout to
header and body reads; idle timeout still begins only after successful headers and
does not hide a connection/header failure. Snapshot timeout behavior is unchanged.

### Detached monitoring tasks could outlive a canceled invocation

Rule: resource ownership must handle cancellation and destruction (rust-best-practices).
Tokio JoinHandle drop alone detaches a task. The monitor instead owns an abort-on-drop
wrapper for every spawned task. Stop aborts all tasks before awaiting shutdown and
recovering snapshots. An actual open TCP log stream closes when its invocation is
canceled. Drop never blocks or starts network work; explicit stop recovers snapshots.

## Should fix: resolved

### Worker collection order affected recovered log order

Rule: preserve observable Python dictionary insertion order (rust-port).
Worker streams use an ordered Vec rather than a HashMap, and a two-worker test verifies
snapshot output follows assignment order. Deduplication still uses a HashSet because
its iteration order is never exposed.

## Validation and boundaries

- Thirty-three captured Python cases cover filtering, metric transitions, timestamp
  precision/offsets, and tail formatting, including zero and negative tail lengths.
- Local HTTP tests cover HAPI versus REST routing, pod versus worker sources, multiline
  frames, invalid/incomplete JSON, delivered cursors, UTF-8 split across TCP writes,
  startup delays, idle closure, retry after a fresh-worker 403, and snapshot failures.
- User callbacks execute outside monitor state locks. Short synchronous locks protect
  counts/deduplication; no lock guard crosses an await. Reconnect tasks and metrics polls
  use owned clients, with explicit cleanup and shared state rather than Python globals.
- Normal Clippy with warnings denied, formatting, build, docs, and tests are commit gates.
  A separate pedantic pass remains advisory, including existing must_use/long-function
  style warnings. Full tests passed on both stable and Rust 1.88 on Windows.
- Remote stable CI passed for the preceding commit; the new commit's remote run must
  be checked separately. The workflow now also tests Rust 1.88 on Linux.
- An explicitly stopped monitor recovers snapshots. Cancellation drops the owner and
  aborts I/O without async snapshot recovery. Timestamp parsing supports the documented
  native ISO forms rather than every exotic Python fromisoformat input.
- Task execution, deployment/discovery/dev workflows, management/CLI, and remaining
  worker utilities are still pending; this review does not claim complete SDK parity.

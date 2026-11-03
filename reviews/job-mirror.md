# Rust review: bounded worker job snapshots

Reviewed the library state/mirror diff with rust-port, rust-best-practices and rust-review
under Rust 2024 / MSRV 1.94.1. The bounded snapshot and attachment APIs are implemented;
independent heartbeat execution and retry integration remain pending. No new dependency
or unsafe code is used.

## Must fix

None outstanding. Tracker mutations and snapshot publication occur under the same tracker
lock. Publishing after releasing that lock could let a slower earlier update overwrite
the newest snapshot. The mirror never acquires the tracker lock, so the lock order has
no reverse path. No callback or await runs under either lock. Initial review found that
tracing a truncation while holding the tracker lock could deadlock a subscriber that
inspects job state. The new regression failed with that deadlock. Publication now returns
a diagnostic value; the caller releases both locks before invoking tracing. Direct mirror
writes follow the same rule. Byte publication still happens under the mutation lock.
Rule: protect related data together and avoid lock hazards (rust-best-practices section 7).

The mirror reserves a NUL byte, truncates bytes before looking for the last comma, and
does not repair an incomplete UTF-8 suffix. Invalid UTF-8 reads None, as in Python. A
capacity of zero or one cannot store visible text. These branches are replayed against
actual Python outputs, including overwrite after an invalid snapshot.
Rule: characterize and preserve observable edge behavior (rust-port sections 1 and 2).

## Should fix

None outstanding. Mirror poison is best effort rather than recovered unsafely or allowed
to panic into job handling. A fault-injection unit test poisons the mirror, then attaches
and mutates the tracker and confirms the tracker remains usable. Allocation failure follows
Rust's standard allocator policy, as documented; this does not emulate Python MemoryError.

## Consider

- Arc-owned memory replaces Python's multiprocessing shared array. It is deliberately
  documented as thread sharing, not cross-process IPC. A native independent heartbeat
  executor will consume this buffer; this commit does not claim that integration.
- Explicit tracker clones replace process-global singleton identity. Independent trackers
  and mirrors remain independent even in the same process or working directory.
- The pre-existing BTreeMap makes ID ordering deterministic; Python's set ordering is
  unspecified. Duplicate adds preserve original job data but still publish a snapshot.
- Mirror replacement/detachment leaves the previous buffer unchanged. A shared mirror
  explicitly attached to different trackers can be overwritten by either, like Python.
- Diagnostics use native tracing messages. Empty/invalid/poisoned reads return None;
  no snapshot parsing error is allowed to stop ordinary job handling.

## Verification

Tests were added first and failed for missing PingJobMirror and set_mirror APIs. Six
captured Python sequences cover default, small and zero capacities, comma boundaries,
embedded NUL, incomplete UTF-8 and subsequent overwrite. Named counterparts cover all
13 tests in test_integration_worker_state.py; singleton and cross-process behavior are
explicitly adapted. Additional tests cover duplicate/missing mutations, detachment,
concurrent publication and poisoned mirrors. A subprocess verifies that tracker/mirror
operations write nothing into their working directory.

The unchanged Python state, mirror, scaler-mirror and heartbeat baseline passes 49 cases
with one Windows fork skip. Only the 13 mirror integration functions receive new test
credit here; the heartbeat/scaler cases are baseline evidence, not ported-test claims.
The full Windows suite passes 1,113 tests, zero failures, with nine ignored subprocess
entry points exercised by parents. Subsequent logging-lock regression and focused
state/worker/scaling/runtime checks validate the final review correction. Build, formatting,
strict all-target/all-feature Clippy and rustdoc pass. All 37 focused checks after the
logging correction pass. Pedantic review added constructor must-use annotations and
tracker poison documentation, and corrected a macro semicolon. The only remaining
scoped warning is pre-existing Default::default syntax in WorkerJob metadata construction.

See [parity/job-mirror.json](../parity/job-mirror.json) for exact mapping and fixture hash.
Regenerate with tools/job_mirror_fixtures.py against the pinned Python SDK, then run
tools/update_job_mirror_parity.py. Neither source repository is modified.

## What's already good

- Fixed-capacity storage keeps heartbeat snapshot memory bounded.
- Source captures retain inconvenient byte-level quirks rather than guessing them.
- Lock ownership makes concurrent publication order explicit.
- The ledger separates completed snapshot APIs from pending heartbeat lifecycle work.

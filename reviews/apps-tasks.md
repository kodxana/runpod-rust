# Rust review: ephemeral task lifecycle and runtime selection

Library scope: images, custom-image launchers, task execution/jobs, and PodTarget.
Edition 2024; MSRV 1.88. Guided by rust-port, rust-best-practices, and rust-review.

## Correctness fixes

- ResourceSpec validation accepts whitespace around CUDA versions. A new Python fixture
  failed against the initial task payload builder; trimming the validated value before
  selecting allowed versions now matches the normalized Python spec.
- Cancellation during creation must not discard a late pod ID. The spawned deployment
  owns its execution state, records the ID, then signals completion. Cleanup waits on
  that signal before deletion. Tests abort creation and pending result waits and observe
  the eventual DELETE request.
- Async cleanup cannot run inside Rust Drop. A scoped guard starts an owned Tokio task;
  explicit cleanup waits up to thirty seconds without aborting that task on timeout.
  A virtual-clock test verifies cleanup remains active past that foreground deadline.
- Failed deletion retains the ID. Concurrent terminators serialize, transient failures
  retry, and 404 clears the ID. Execution failures take precedence over cleanup failures.

## API and ownership review

- Owned clients and short watch-state updates replace Python task globals. No state
  borrow crosses await; callbacks run outside state updates. Task tokens use OS randomness
  and are omitted from Debug. Runner HTTP uses a separate anonymous pool and only the
  per-task bearer token; readiness requests carry no credentials.
- Runtime hardware selection uses ComputeType rather than an unexplained public boolean.
  Interpreter selection is explicit, replacing local Python introspection. ResourceKind
  makes unknown runtime kinds unrepresentable after normal deserialization.
- Each TaskExecution is single-use. Clones observe the same deployment and deletion state.
  A TaskJob must be waited on or canceled; merely dropping an unused job does not delete
  its pod. The executor must remain alive while detached cancellation cleanup runs.
- Original runtime shell commands are fixture-checked exactly. Unix tests execute them
  with an isolated shell interpreter stub, checking installed/missing packages, overrides,
  and installation failure. They do not install packages or invoke local Python.

## Validation and limits

- Seventy-five captured Python cases cover image/version selection, launcher text, pod
  hardware and placement inputs, environment override order, CUDA floors, and result decoding.
- Local HTTP tests cover six-attempt 404 retry exhaustion, non-replayed other failures,
  timeout validation/acknowledgement, registry/storage integration, sibling placement,
  setup failure, canceled creation/waits, result caching, and deletion retry/idempotence.
- Upstream Python subset: 98 passed and five Windows-specific failures. Four require
  an unavailable POSIX sh. The sibling-volume test compares a Windows Path string with
  a POSIX endpoint mount path; the Rust equivalent passes using platform-independent paths.
- Normal Clippy with warnings denied, formatting, build, docs, stable tests and MSRV tests
  are commit gates. A separate pedantic review identified small API/documentation/style
  issues that were addressed here; unrelated pre-existing pedantic warnings remain advisory.
- No cloud pod was provisioned. Live runtime/image compatibility and the remaining
  function/test audit are still part of the overall port, not claims established by mocks.

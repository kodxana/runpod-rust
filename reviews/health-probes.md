# Rust review: native health probes

Library review using the Rust porting, best-practices, and review skills. Scope: GPU/system
probes, owned configuration, native binary packaging, eligibility helpers, error integration,
and tests. Fitness orchestration and GPU framework integration remain pending in the ledger.
Edition 2024 and MSRV 1.94.1 are unchanged.

## Must fix

No outstanding finding within the implemented probe layer. GPU allocation failures have a
separate typed variant and cannot be masked by a fallback that merely lists devices. Process
timeouts kill and await the child; dropped probe futures use Tokio's kill-on-drop ownership.
Tests observe canceled/timed-out child processes failing to produce their delayed output.
This follows rust-port's observable-behavior requirement and Rust API Guidelines C-GOOD-ERR.

## Should fix

No outstanding finding. Native subprocesses receive argument vectors without a shell.
Executable arguments and child environment are excluded from Debug; worker URLs are also
excluded. TCP probes use only host/port and send no credentials or job request. Fallback
errors retain their source. Linux binary extraction closes the writable descriptor before
execution, sets owner-only executable permissions, and retains the temporary path by ownership.

## Consider

- Configurable native commands allow deterministic process testing without Python or GPUs.
  The helper executable is compiled with the same Rust toolchain used for each test binary.
- The source ignores gpu_test exit status when stdout reports success, and its negative
  maximum-error setting is a Python slice endpoint. Both quirks are retained and tested.
- Pedantic cast warnings for memory/disk bytes describe deliberate binary64 unit conversion,
  matching Python float arithmetic. Timeout casts occur after sign validation/clamping.
  The version regex is a constant pattern; its expect cannot be triggered by user input.
  Other pedantic suggestions concern documentation, must-use annotations, or newline style.
- Unsupported binary platforms use explicit executable overrides or fallback. Native OS
  counters, URL parsing, and bounded integer domains are documented adaptations.
- No native GPU framework is initialized by these probes, and no new health checks are
  automatically run during worker startup yet. Remaining lifecycle tests stay pending.

## What's already good

- Captured Python fixtures verify parsers, host/port selection, and eligibility decisions.
- Local sockets verify that readiness checks send no HTTP payload or credentials.
- Controlled-clock tests check retry count, one shared time budget, and cancellation cleanup.
- Real native child processes exercise allocation failure, fallback, timeout, and cancellation.

## Verification

The upstream health suite passed 158 tests; two Unix-script fixture executions fail on Windows.
All 43 captured cases are replayed by native tests. Windows stable Rust 1.99.1 passes the full
417-test suite (409 integration, six unit, two doctests), and Rust 1.94.1 passes all 28 focused
unit/health tests. Build, formatting, Clippy with warnings denied, and rustdoc checks pass.
Linux packaged-binary ownership and Unix executable-permission tests await CI. No physical
GPU or billable Runpod resource was used.

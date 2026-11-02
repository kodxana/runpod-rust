# Rust review: endpoint resource commands and observation

Application code, edition 2024, MSRV 1.94.1, reusing the library's owned client and typed
configuration API. Scope covers nine endpoint resource commands, formatting, editing,
rollout/worker observation and incremental CLI output at the pinned source revision.
The overall implementation follows the Python command contracts. The remaining endpoint
data-plane/job/log and completion functions are explicitly pending. Full tests, strict
Clippy, build and documentation checks are run; details are recorded below.

## Must fix

None outstanding. Review corrected four observable differences before commit:

- `src/cli/endpoint/mod.rs`, `name_pattern`: native filesystem glob syntax initially
  rejected ordinary repeated stars such as `llama**`. A small adapter collapses adjacent
  stars and escapes unmatched opening brackets before native matching. Twelve actual
  Python name-filter fixtures cover these cases, negation, literal braces and ranges.
- `src/cli/endpoint/options.rs`, `Options::read`: an unreadable `--file` initially became
  an API-category exit 1. It now produces usage exit 2 during option processing, before
  credential/client construction or requests. The opened file is retained until use.
- `src/cli/streams.rs`, `frame_height`: clearing only logical lines left stale wrapped
  terminal rows. The frame height now counts physical rows using Unicode display width.
  A regression test includes full-width text, combining marks and an exact-width boundary.
- `src/cli/mod.rs`, endpoint dispatch: an omitted or empty ID in a noninteractive get/edit
  initially reported missing credentials first. Interaction eligibility is now checked
  before client construction. A subprocess regression removes credentials and home-path
  variables, also checking that unreadable files precede ID validation and an explicit ID
  still reaches credential validation before editor eligibility.

**Rule:** Preserve observable inputs, failure ordering and outputs (rust-port sections
1 and 2). Each correction was preceded by a failing native regression check.

The editor avoids the alternative backend's panic on empty commands and correctly treats
nonzero editor exit status as failure. It retains an owned temporary path, closes the
initial file before launching, and cleans it up on every return. It never executes a
shell. Parsed values, duration conversion and required response fields use typed errors;
no new unsafe code or panic-based input handling was introduced.

## Should fix

None outstanding. Commands use a shared incremental writer so successful update output
survives a later polling failure and edit patches remain visible when confirmation fails.
JSON progress uses stderr, with no hidden buffering until command completion. A regression
fixture applies an update and then fails release polling, proving both outputs survive.

**Rule:** Preserve side-effect/output ordering (rust-port section 1). Implemented at
`src/cli/streams.rs` and `src/cli/endpoint/mod.rs`, `Options::apply`.

The fixture harness initially compared integer query arguments with serialized URL
strings and recorded only endpoint selection's terminal predicate. Those observations
were corrected to compare transmitted query values and separately record confirmation's
predicate. Native behavior was retained; the source tests themselves were not changed.

**Rule:** Compare equivalent inputs at the same boundary (rust-port section 2).
Corrected in `tools/endpoint_cli_fixtures.py`.

## Consider

- Native CLI flags, bounded integers, JSON diagnostics and terminal rendering are the
  documented adaptations in `PORTING.md`. Optional configuration fields remain absent
  unless supplied; creation and update use the same tested library payload helpers.
- Observation timeouts preserve the source's successful-detach behavior and do not
  become HTTP deadlines. Ctrl-C cancels local waits/requests while leaving the remote
  resource intact. Signal registration failures remain typed errors.
- The public library API is unchanged except that the existing private command-word
  parser is now shared with the native editor. Editor preference and fallback selection
  are native; save detection and exit status are tested with an actual Rust subprocess.
- Strict Clippy passes. Pedantic review leaves advisory `too_many_lines` diagnostics for
  the nine-command dispatcher and two fixture harnesses, `struct_excessive_bools` for
  independent private CLI flags, and `unused_async_trait_impl` for the immediate fake
  clock. These are deliberate application/test shapes; no lint suppression was added.
  The nonempty-fixture assertion is retained despite the newer `assert_is_empty` advisory;
  an explicit cast to an empty array would add little useful failure information.
- Command implementations are split into option parsing, views and observation helpers,
  with shared output/editor code. The volume/catalog dispatch and tests remain in place.
  One small native dependency (`glob`) supplies name matching; no general CLI framework
  or source-language runtime emulation was added.

## What's already good

- Python-generated cases cover every new resource command, both JSON and human output,
  the request sequences, partial failures, delayed releases, and confirmation behavior.
- Runtime-only changes skip polling, baseline failures still permit updates, missing
  IDs avoid noninteractive list requests, and delete reads its name even with `--yes`.
- Editor retry retains invalid text, unsaved edits do nothing, and file handles are
  validated before any API operation. An actual child process verifies save/no-save/failure.
- Unknown JSON fields survive raw output and payload merges. Plain output preserves
  complete identifiers; live redraw accounts for terminal display width.

## Verification

- Test-first process execution failed all 33 initial command tests while `endpoint`
  was absent. Later failures exposed the glob, file-option, frame-height, and credential
  ordering differences.
- **75 parameterized upstream tests pass** unchanged. The generator records **116 actual
  Python command/helper/edge invocations** and reproduces them byte-for-byte.
- Forty process tests, nine interactive/rollout tests, the original worker-view
  counterpart, and native editor/transport/frame regressions cover the new implementation.
  The complete Windows suite passes **749 tests**, zero failures, and three ignored child
  entry points exercised by parent tests; the total includes two passing doctests.
- Locked build, formatting, strict all-target/all-feature Clippy and rustdoc pass.
  Pedantic diagnostics were reviewed as described above. The prior configuration commit
  passed Linux, macOS, Windows and MSRV CI; this commit's matrix is checked separately.
- A real Windows terminal applied an edited document after displaying the patch and
  receiving `y`, producing GET then PATCH. Ctrl-C detached rollout observation and ended
  JSON worker watch with child exit 0. The host PowerShell PTY reported interruption
  status 1 after Ctrl-C; the harness independently recorded the Rust child's exit code.
  All requests hit ephemeral loopback servers with synthetic credentials.

Ignored logs and the manual terminal harness are in `test-results/endpoint-*`. The compact
build profile remains enabled; package artifacts are cleaned after validation.

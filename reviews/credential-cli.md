# Rust review: registry and secret CLI

Reviewed the new private CLI module, command registration, hidden input and parity tests
as application code inside the SDK (edition 2024, MSRV 1.94.1). The implementation reuses
the tested Apps transports and keeps secrets out of displayed rows and derived debug
output. Two behavioral problems found during validation were fixed before publication.

## Must fix

No unresolved findings.

### 1. Hidden-input cancellation terminated the Windows process — fixed

**Location:** `src/cli/interaction.rs:57` and `hidden_line`.
**Rule:** preserve observable errors and test I/O boundaries (rust-port sections 1–2;
rust-best-practices section 11).

The initial dialoguer password prompt exited with Windows status `0xC000013A` on Ctrl-C.
The failing PTY check expected a clean exit 1. The existing console library's raw key API
delivers Ctrl-C as input, allowing this small fix without process-global signal handling:

```rust
let result = hidden_line(|| term.read_key_raw());
// In hidden_line:
Key::CtrlC | Key::Char('\u{4}') => return Err(CliError::Aborted),
```

The fixed PTY run prints `Aborted!`, exits 1 and sends zero requests. A second terminal
run verifies hidden entry with backspace reaches the API correctly. Unit tests cover
Unicode deletion, empty submission, interruption and no input consumed after cancellation.

### 2. Native parser rejected valid credential values — fixed

**Location:** `src/cli/credentials.rs:26`.
**Rule:** preserve source argument semantics (rust-port sections 1–2).

Source characterization showed that Click accepts values beginning with `--` and uses
the final repeated scalar option. The native fixture failed on the former. The add
command now declares those semantics directly:

```rust
Command::new("add").args_override_self(true);
Arg::new("password").long("password").allow_hyphen_values(true);
```

The same option setting applies to usernames, secret values and descriptions. Four
source captures cover these cases through both the private implementation and binaries.

## Should fix

No unresolved findings.

## Consider

- `src/cli/credentials.rs:144`: preserve exact first-name lookup and the source's success
  message for a false `secretDelete` response. These quirks are intentional and captured;
  changing either would alter Python behavior (rust-port section 1).
- `src/cli/credentials.rs` and `src/cli/interaction.rs`: native terminal input, literal
  unstyled output and typed diagnostics are documented adaptations. Scripts provide
  explicit fields and confirmation instead of piping answers to prompts. No attempt is
  made to reproduce Rich markup or Click's terminal runtime (rust-port section 4).
- Private operation variants carry only fields relevant to the selected command, and
  do not derive Debug because creation variants contain secret material. The code stays
  proportionate: roughly twice the nonblank source code, plus a small hidden-input loop.
- `src/cli/credential_tests.rs:10`, `tests/cli_credentials.rs:9`: pedantic review retains
  `too_many_lines` advisories for the fixture runners. Their linear request/output checks
  stay together for reviewability (rust-port section 4). String-assembly and empty-string
  assertion advisories were fixed; no lint suppressions were added.

## What's already good

- Forty-four captures execute the original Python assertions or characterized commands,
  then replay actual SDK methods to record REST/GraphQL requests. Fixtures regenerate
  byte-identically with SHA256 `9FBF0390AFACE581D70AD26CD594C91A975C5578A5D9CCFA8AFF681E7C629203`.
- Ten named test counterparts retain source traceability; thirty-one captured invocations
  also run through the native executables, checking full request sequences and output.
- Prompt cancellation and declined deletion prevent all requests; creation failures
  suppress success messages. No mutation is retried by the command layer.
- Sorting is stable; duplicate names, missing names, explicit empty values and metadata
  that unexpectedly contains a secret value are covered. Lists never print that value.
- No new dependency, unsafe code, global state or public SDK API was introduced.

## Verification

- Ten unchanged Python source tests pass, plus 34 source characterizations. The source
  remains pinned at `ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5` with no tracked changes.
- Thirteen focused credential tests and two hidden-input tests cover this milestone.
  Initial process tests failed before command registration; parser and terminal regressions
  also failed before their fixes.
- The full Windows suite passes **887 tests**, zero failures and three ignored child
  entry points exercised by parent tests. Locked build, formatting, strict all-target/
  all-feature Clippy and rustdoc pass. Pedantic review was run and triaged above.
- All traffic uses loopback servers and synthetic credentials. No live Runpod resources
  or user credentials were modified.
- Previous pod commit `3871f032fb0f77b51f14334d66c210ac8e0bb3a0` passed Windows, Ubuntu and
  MSRV CI; its macOS job was cancelled. This milestone triggers a new complete matrix.

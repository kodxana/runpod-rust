# Rust review: login CLI

Reviewed `cli::login`, command registration, source capture and native tests as application
code within the SDK (edition 2024, MSRV 1.94.1). All three authentication modes reuse
existing native SDK protocols and atomic credential persistence. No outstanding production
findings remain; formatting, strict Clippy, build, full tests and rustdoc pass.

## Must fix

None outstanding. Tests were written before registration and initially failed because
`login` did not exist. The fixture comparison then exposed Click's extra EOF prompt line;
the harness strips that rendering artifact while still requiring `Aborted`, exit 1,
no write, and unchanged credentials. It does not suppress a functional difference.

## Should fix

None outstanding.

## Consider

- `src/cli/login.rs:41`: preserve empty-key behavior. Presence conflicts with `--agent`,
  whereas a standalone empty key selects browser approval. Whitespace-only direct keys
  and empty profile names are also retained (rust-port section 1). Tests characterize
  these cases instead of silently tightening source validation.
- `src/cli/login.rs:94`: the private SDK/UI error boundary retains the original prompt
  error separately, so cancellation remains `CliError::Aborted` rather than becoming a
  generic SDK string error. No public error variant or SDK API was added (Effective Rust
  error handling; rust-best-practices sections 4–5).
- `src/cli/login.rs:207`: desktop launching is best effort, as with the ignored Click
  status. Native launchers receive the URL as an argument, the Windows helper has no
  console window, and a task reaps the child without closing the user's browser. Python's
  extra desktop/browser fallbacks are documented native adaptations (rust-port section 4).
- `src/cli/login_tests.rs:10` and `:18`: pedantic `unused_async` advisories remain for
  authentication fakes. Keeping callbacks inside the future defers their side effects
  until it is polled, matching the native implementations. `verify` retains its length
  advisory so one capture's auth/UI/persistence checks stay together. A boolean-to-integer
  test advisory was fixed. No suppressions were added.

## What's already good

- Typed modes prevent mixing direct keys with browser or email state. Options are
  validated without printing supplied keys; direct-key storage never initializes an
  authentication client or parses its URL.
- Anonymous clients exclude existing account headers. The email reader validates six
  ASCII digits locally, preserves the same attempt/verifier across API rejection, and
  never echoes the code. Success is printed only after the selected profile is written.
- Source fixtures verify exact auth/prompt/write calls, output, profile preservation,
  retries, malformed TOML and write errors. Their generator refuses paths outside its
  temporary workspace, so user credential files are never read or written by capture.
- Process and terminal tests exercise actual transports and storage. Cancelled email
  input sends no verification request; cancelled browser approval stops polling. Neither
  cancellation modifies the existing credential file.
- No dependency, unsafe code, public SDK API or live service operation was introduced.
  The port remains proportionate: about 222 nonblank Rust lines versus 67 Python lines,
  including explicit typed errors, platform launching and a private testable I/O boundary.

## Verification

- **18 unchanged upstream cases across 11 functions** execute. On Windows **17 pass**;
  the named-profile case passes every behavior assertion before its last Unix-only
  permission assertion fails (`0666` vs `0600`). The generator accepts only this exact
  failure and rejects any additional failure. The source checkout remains unchanged.
- That test is explicitly mapped as a platform adaptation. Native counterparts check
  Unix mode 0600 and profile contents; Windows uses native permission semantics.
- **11 characterizations** yield **29 deterministic captures**, SHA256
  `F516736853150BEBF4BAB891B8D0F7D9A71AF8C7720B296ECCA5365427EC7CE5`.
- **12 unit tests and 3 process tests** cover login. The full Windows suite passes
  **902 tests**, zero failures and three ignored child entry points exercised by parents.
  Locked build, formatting, strict all-target/all-feature Clippy and rustdoc pass.
- Windows PTY checks verify hidden invalid/rejected/accepted codes, the PKCE challenge,
  repeated verification with the same verifier, selected-profile storage, and Ctrl-C in
  both flows. All requests use loopback servers and synthetic credentials. The real OS
  browser launcher is not invoked during tests; its callback is checked with a fake.
- Previous commit `8e34bc7a8ed8e5c7475d6d05ce600e8c491bf1d4` passed Windows, macOS, Ubuntu
  and MSRV CI. This milestone triggers a fresh matrix after publication.

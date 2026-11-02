# Rust review: native pod resource commands

Application code, edition 2024, MSRV 1.94.1. The change adds pod resource commands and
reuses existing catalog, container, output, interaction and log transports. It preserves
request ordering, GPU selection, environment/mount edits, confirmation and restart
readiness. Two parity issues were found and fixed: optional log IDs and malformed update
state causing an unintended PATCH. Required checks and the full suite pass.

## Must fix

### 1. Resolve omitted log IDs through the pod picker (`src/cli/pod/mod.rs:94`)

**Rule:** Preserve public inputs and interaction behavior (rust-port sections 1 and 2).

**Problem:** The earlier log milestone required a pod ID. Python accepts an omitted ID,
opens a picker in a terminal, and reports `pass a POD_ID` in noninteractive mode.

**Fix applied:**

```rust
logs::command("pod-id").mut_arg("pod-id", |arg| arg.required(false))
```

The pod dispatcher now resolves the ID before constructing log options and passes the
same authenticated client to streaming. A loopback test checks the selected pod and
request sequence; a Windows PTY check selects a pod and reads its logs. Process tests
verify missing IDs fail with usage status before credentials.

### 2. Reject malformed current settings before PATCH (`src/cli/pod/config.rs:34`)

**Rule:** Preserve failure/side-effect ordering; invalid responses must not silently
become mutation inputs (rust-port sections 1 and 2; rust-review correctness).

**Problem:** Indexing a string, nonempty array or number as a JSON record produced an
empty environment map, allowing PATCH before the response-rendering error. Direct Python
characterization verifies these inputs stop after GET. A native regression initially
observed two requests where only one was expected.

**Fix applied:**

```rust
if current.is_some_and(|value| truthy(value) && !value.is_object()) {
    return Err(Error::InvalidResponse(
        "current pod settings must be an object".into(),
    ).into());
}
```

Source falsy current values retain their empty-record behavior. All three malformed
truthy responses now fail before PATCH. No correctness finding remains outstanding.

## Should fix

None outstanding. Hardware selection is an enum; optional flags retain explicit false
values, and oversized operation payloads are boxed privately. Owned async requests make
wait cancellation local. The public CLI future remains Send, verified at compile time.
Container field maps are moved into creation payloads instead of cloned.

## Consider

- `src/cli/pod/wait.rs:39`: the source's 600-second deadline is checked after each read,
  with readiness tested first. This intentionally does not impose a shorter per-request
  HTTP deadline. Restart needs both a new start timestamp and observed runtime reset;
  a recent uptime alone is insufficient.
- `src/cli/pod/config.rs:170`: keep the source's unique-GPU-match eligibility exception
  and stock/price/name ordering. Price notices use floating multiplication, like Python;
  the `u64` to `f64` pedantic advisory is intentional for display, not request quantities.
- Pedantic review retains line-count advisories for operation dispatch, parsing, GPU
  selection, fixed field rendering and fixture verifiers. These keep related source
  behavior together. Boolean, closure, assignment-semicolon and test narrowing advisories
  were fixed. No lint suppression or dependency was added. Rule: proportionate ports
  (rust-port section 4), rather than splitting code only to satisfy a line threshold.
- Plain terminal messages, native parser/transport diagnostics, bounded integers and
  owned credentials are documented adaptations. Rich animation, SSH connect/sync and
  completion remain pending in the full goal rather than being claimed by this module.

## What's already good

- Forty-eight named source counterparts check full API sequences, bodies, query arguments,
  notices, structured/field output, choice filtering and confirmation behavior.
- Seventy-one captured noninteractive invocations run through native binaries; tests also
  cover credential-independent usage checks, numeric bounds and both executable aliases.
- Mount paths and existing environment values survive edits; JSON output retains raw
  records while displayed-status filtering handles missing/zero container uptime.
- Restart tests distinguish unchanged timestamps, stale uptime and a genuine runtime
  reset. Declined termination and lookup failure prevent DELETE; `--yes` skips lookup.

## Verification

- **82 unchanged parameterized Python cases pass**, producing **89 invocations across
  48 test functions**. The generator also verifies three malformed-state failure paths.
  Fixtures regenerate byte-identically; the pinned Python checkout is unchanged.
- **51 unit tests and 3 process tests** cover the milestone. The malformed-state
  regression failed before its guard and passes after it.
- The full Windows suite passes **872 tests**, zero failures and three ignored child
  entry points exercised by parents, including two passing doctests. Locked build,
  formatting, strict all-target/all-feature Clippy and rustdoc pass.
- Windows PTY checks verify a prompted creation name reaches POST, omitted-ID logs use
  the picker, declined termination performs only GET, and Ctrl-C aborts a restart wait
  with child exit 1 after exactly one action POST. All traffic uses loopback mocks and
  synthetic credentials; no Runpod resource is created or modified.
- The preceding commit `42fe0647ccca603f474e76767909b029e29da5a2` passed Windows, macOS,
  Ubuntu and MSRV CI. This commit's CI is triggered after publication.

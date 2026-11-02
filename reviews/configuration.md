# Rust review: endpoint configuration and shared options

Library code, edition 2024, MSRV 1.94.1. This review covers the pinned Python
`rp_cli/options.py` and `rp_cli/endpoint_config.py` port, both client facades, and
extraction of private JSON comparison/pagination helpers. The public API has owned typed
options and returns payloads without mutating remote resources. Endpoint CLI parsing,
editing, rollout, logs and job execution remain pending, with no command mappings claimed.

## Must fix

None outstanding. Timeout conversion uses checked multiplication; nonfinite queue delays
fail before serialization. Malformed current records return typed errors. Invalid env
assignment errors do not include the supplied value. Shell quoting performs no execution,
expansion, or environment lookup. Existing source behavior is retained for omissions,
empty values, merging, CPU/GPU kind restrictions, and partial GPU edits.

## Should fix

None outstanding. Review identified a dependency from configuration patching into the
Apps development module for numeric JSON equality. The implementation now lives in
`src/json_value.rs`, shared privately by both callers without changing its behavior.

**Rule:** Keep module dependencies organized by responsibility (rust-best-practices,
section 9). Corrected at `src/configuration.rs`, `endpoint_patch`, and `src/apps/dev.rs`,
`changed_fields`, using `crate::json_value::equivalent`. The development session's
refresh comparison also uses the shared helper directly; the build caught its former
parent-module reference after import cleanup, and that call site was corrected.

The blocking integration test initially used Tokio's blocking pool, which retains an
entered runtime and correctly trips the blocking client's nested-runtime guard. It now
constructs and drops the client on a separate OS thread; the async test awaits that
thread while its loopback mock server runs.

**Rule:** Test the supported calling contract (rust-port section 2); do not relax a
runtime guard to accommodate an invalid test setup. Corrected at
`tests/configuration.rs`, `blocking_configuration_resolves_registry_and_preserves_merges`.

## Consider

- Integer quantities use checked `u64`; native enum variants replace spelling-sensitive
  option strings. `Display` and `FromStr` expose the CLI spellings without introducing
  a parallel builder hierarchy. The public methods support both existing client facades.
- Native JSON parsing rejects NaN/Infinity and uses Serde diagnostic text. Nested editable
  values intentionally retain the source's permissive validation rather than adding
  service-side range checks. These differences and quirks are documented in `PORTING.md`.
- A small private quoting parser retains `shlex.split` behavior with comments disabled.
  The available `shell-words` parser discards hash comments and line continuations and
  treats dollar/backtick escapes differently, so it cannot preserve these ordinary
  command inputs. No dependency or general shell interpreter was added.
- Shared pagination retains the CLI's existing cursor and retry semantics. Extracting it
  into `src/pagination.rs` avoids making the library configuration API depend on CLI
  orchestration. Existing CLI tests still exercise the same helper through a re-export.
- JSON numeric comparison retains the existing guarded float-to-integer conversion.
  Added Python fixtures cover integral floats and large integers that must remain unequal.
- The main configuration module is about 710 nonblank code lines excluding comments and
  attributes, against 324 in the two Python modules (about 2.2 times the source size).
  The blocking facade adds 37 lines; the shared comparison/pagination code was moved,
  rather than duplicated. This stays within the rust-port size guidance.

## What's already good

- Python-generated fixtures compare complete payloads, error messages, request order,
  paths, queries and request counts, with synthetic credentials and loopback servers.
- Environment and mount merges preserve unknown existing keys; GPU editing retains the
  source's paired pool/exclusion update and empty-string CUDA reset.
- Dedicated tests prove that overflowing/nonfinite values fail without network requests,
  malformed documents return structured errors, and the blocking facade performs lookup.
- Explicit pending mappings keep library helper coverage separate from CLI completion.

## Verification

- Test-first compilation failed while the configuration API and blocking methods were
  absent, before their respective implementations were added.
- Focused configuration tests pass 26 groups, including 102 pinned Python comparison
  cases. The unchanged upstream subset passes 30 parameterized cases. Fixture
  regeneration is byte-identical.
- The full Windows Rust 1.99.1 suite passes **696 tests**, zero failures, and two ignored
  child entry points exercised by parent tests. The total includes two passing doctests.
- Locked build, formatting, strict all-target/all-feature Clippy, and rustdoc pass.
  Pedantic review prompted documentation formatting and a pattern match in the editor
  validator, replacing a guarded `expect`. The final focused suite rechecks those
  cleanup changes; the new configuration, pagination and comparison modules have no
  remaining pedantic diagnostics. Existing unrelated advisory warnings are unchanged.
- Platform and MSRV verification use the new commit's CI; Windows success is not assumed
  to establish Linux/macOS behavior. The preceding volume commit passed all four jobs.

Ignored verification logs are in `test-results/configuration-*`. The compact development
profile remains enabled, and generated package artifacts are cleaned after validation.

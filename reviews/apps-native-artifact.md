# Rust review: native compiled-worker packaging

Reviewed the new library entry point and shared archive implementation under Rust 2024 /
MSRV 1.94.1. The native path packages an existing executable and assets for the implemented
bootstrap; it does not yet deploy that archive or select a compatible worker image.
Formatting, Clippy, tests and documentation are checked as recorded below.

## Must fix

None outstanding. Output aliasing the executable would truncate the input before it could
be archived. The preflight in `src/apps/package.rs:315` checks filesystem identity, including
hard links, before creating output. The tests assert that both worker bytes and existing
output survive rejected inputs. **Rule:** validate fallible input before destructive I/O
(rust-best-practices sections 4 and 11).

```rust
if let Some(output) = output
    && output.try_exists()?
    && same_file::is_same_file(executable, output)?
{
    return Err(Error::InvalidInput(
        "native artifact output must not overwrite its executable".into(),
    ));
}
```

## Should fix

None outstanding. The shared archive path at `src/apps/package.rs:337` keeps the original Python-compatible traversal,
ignore precedence, dependency namespace and artifact ownership. Native reservations are
enabled only when a compiled worker is supplied. The original archive tests remain the
regression gate for that refactor. **Rule:** preserve observable source behavior while
documenting native adaptations (rust-port sections 1-4).

## Consider

- `src/apps/native_artifact.rs:34`: the input executable is explicitly caller-selected.
  Packaging checks regular/nonempty file input but does not infer its target platform or
  dynamic dependencies. Native cloud upload and launch must validate their own requirements;
  a successful local worker test is not evidence of Linux/container compatibility.
- `src/apps/package.rs:298`: source files and the executable must remain stable during this
  blocking operation. Open file ownership keeps the worker handle alive, but does not
  snapshot a file being modified by another process. **Rule:** document side effects and
  ownership that signatures cannot express (rust-best-practices section 10).
- `src/apps/package.rs:191`: root `target/` and `runpod-native/` are reserved in native archives.
  Alternate build directories require project ignore rules. This prevents the ordinary
  Cargo cache and stale worker namespace from being accidentally shipped.
- Native manifests omit `pythonVersion` and `excludedPackages`. Resource records retain
  their existing shape. Coordinator support is not asserted by this packaging operation;
  the native deployment path remains unfinished.

The current [Runpod SDK runtime documentation](https://docs.runpod.io/sdk/runtime), checked
on 2026-11-02, describes Python/pip startup for custom images. Docker documents that an
exec-form entrypoint receives command overrides as arguments in its
[Dockerfile reference](https://docs.docker.com/reference/dockerfile/#understand-how-cmd-and-entrypoint-interact).
An image-provided native entrypoint is therefore a possible integration route, but that is
an inference requiring tests against the Runpod launch contract, especially for tasks.

## What's already good

- One shared source walker preserves credential exclusions and nested ignore precedence.
  The explicit worker is appended once with deterministic executable permissions.
- Storage ownership reuses `Artifact`; the new path adds no dependencies, unsafe code,
  global environment mutation, compiler invocation or remote operation.
- Tests inspect archive member bytes, permissions and order, then package and run a real
  compiled worker from the extracted tree and verify its HTTP response using a bundled asset.
- Validation rejects absent symbol metadata before writing an output that could never load.

## Verification

- The new tests first failed to compile because the native packaging API was absent.
  Six native tests now pass, including the extracted worker test; its ignored child entry
  point is invoked explicitly by its parent. Nineteen existing Windows package tests pass.
- Four existing source captures are also applied to the native path for credentials,
  nested ignore precedence, root anchoring and Unicode/member ordering.
- Eight unchanged Python manifest/packaging tests pass. One additional source test fails
  before packaging because this Windows account cannot create symbolic links (error 1314).
  The focused successful run disables the suite-wide coverage threshold; no source code
  or test was modified. Existing Unix Rust symlink tests remain enabled in CI.
- The full Windows suite passes **1,065 tests**, zero failures, with eight ignored child
  entry points exercised explicitly by parent tests. Locked build, formatting, strict
  all-target/all-feature Clippy and rustdoc pass.
- Pedantic review found one test-only `manual_string_new` warning at
  `tests/native_artifact.rs:197`; it now uses `String::new()`. The focused input-validation
  test passes after that cleanup, and the follow-up pedantic check has no warnings in
  the changed artifact/packager code or native tests. Unrelated existing warnings remain.
- Source pins and tracked/index cleanliness are unchanged. SDK inventory counts stay
  unchanged because these source functions were already mapped.

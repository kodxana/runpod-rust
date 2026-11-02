# Rust review: deployed bootstrap startup

Reviewed the deployed entry point, resource-specific system-package installer, shared
task validators and executable example under Rust 2024 / MSRV 1.94.1. This is library
code with an explicitly process-level entry point. Native packaging, platform launch,
live startup and Python-compatible repair remain part of the original port goal.

## Must fix

None outstanding. Task identity is checked before artifact filesystem work and task
manifest identity before installing packages. Preparation failures cannot invoke the
application loader. Existing direct-loading behavior is preserved by shared validators.

## Should fix

None outstanding. Review identified an API gap in the initial loader signature: mount
bindings alone did not expose the actual prebuilt/extracted directory to application
declarations. The loader now receives the owned context by reference:

```rust
F: FnOnce(&str, &DeployedContext) -> Result<AppRegistry> + Send
```

Handlers can clone paths and bindings without changing the process environment or current
directory. The regression tests first required the new signature, then verified bundled
asset access through all three worker kinds. **Rule:** make ownership and required context
explicit (rust-best-practices sections 1 and 3).

The bootstrap module documentation now distinguishes its blocking preparation primitives
from its async orchestration and failure responders. No new dependencies or unsafe code
were introduced. Scoped pedantic review found no new warnings.

## Consider

- `src/apps/bootstrap/system.rs`: cancellation kills the direct package-manager child.
  Descendants follow that program's lifecycle; this is not process-tree cancellation.
  The regression waits for the real child PID to disappear. **Rule:** specify async
  cancellation ownership and its limits (rust-best-practices sections 3 and 7).
- `src/apps/bootstrap/startup.rs`: location/extraction uses the blocking pool. Already-running
  filesystem work cannot be interrupted; cancellation prevents subsequent worker startup.
  Compiled Rust dependencies replace Python imports and pip repair in this native entry
  point. Python-compatible excluded-package repair remains explicitly pending.
- `src/apps/bootstrap/system.rs`: first-match resource selection, update-before-install,
  noninteractive child environment and the final 2,000 Unicode scalar values of stderr
  preserve source contracts. Native diagnostic quoting and typed malformed-declaration
  errors are documented adaptations. Commands receive literal arguments, without shell
  evaluation. **Rule:** preserve observable behavior while documenting language-specific
  adaptations (rust-port sections 1-4).

## What's already good

- Shared task identity/symbol validation prevents divergence between preparation and direct
  loading. Application declarations receive resolved paths without global mutation.
- Real queue/API/task child processes verify artifact reuse/extraction, assets, mounts,
  initialization, execution, task authentication and acknowledged deadline deletion.
- Early queue/API/task failure tests verify reporting without application registration,
  including system-phase failure with an empty executable search path.
- Package-manager tests use explicit fake executables. They verify selection, command order,
  literal metacharacters, errors and cancellation without installing host packages.

## Verification

- Nine unchanged source cases pass, mapped to seven source test functions in the separate
  runtime ledger. Twelve source system-package captures regenerate deterministically.
  SHA256: `05D1CE8020C14C4F50030446F9FC5490BB57DF402EE7BB6E238A790E99C5747A`.
- Eighteen new native tests pass: five preparation unit tests, eight installer tests and
  five startup process tests. The ignored child entry point is explicitly exercised by
  its parents. All sixteen existing runtime-loading tests also pass after helper extraction.
- The complete Windows suite passes **1,059 tests**, zero failures, with seven ignored child
  entry points exercised by parent tests. The API system-phase failure case was added to
  the existing matrix afterward; its focused test passes.
- The compiled example exports the expected app/resource/symbol metadata, then returns
  `{"output":{"greeting":"hello Rust","data":"/data"}}` through a local queue server.
  The initial smoke harness omitted the JSON response content type and later matched a
  result URL without its query string; correcting those harness issues required no SDK change.
- Locked default/examples builds, strict all-target/all-feature Clippy, formatting and
  rustdoc pass. The broader pedantic run retains unrelated pre-existing warnings; no
  warnings reference the changed startup, installer, runtime helpers, tests or example.
- Both source checkouts retain their pinned revisions with no tracked/index modifications.
  SDK inventory counts remain unchanged; this runtime coverage is accounted for separately.

## CI follow-up

Run 41523333260 exposed a POSIX fixture error on stable Linux and MSRV: the ungrouped
`&&`/`||` chain required the final install predicate even for update-failure mode. The
installer tests correctly rejected that false success. Grouping each mode/subcommand pair
fixes the fixture without changing the production installer or assertions. A local run
under Git's POSIX shell reproduced the wrong zero exit before the fix; all six combinations
of normal/update-failure/install-failure mode and update/install now return their expected
statuses, command log and stderr. Cross-platform CI is rerun by the correction commit.
# Windows CI fixture readiness correction

The Windows job for `0db913e` failed at the package-manager fixture's five-second
startup deadline, before cancellation was exercised. Other concurrent PowerShell
fixtures in that job needed more than five seconds to finish. Readiness now has a
separate 30-second bound; the five-second process-termination assertion is unchanged.
The task is aborted and awaited even when readiness times out, so a failed test
does not leave the installer running. All eight installer tests pass locally after
this correction; no production behavior or source parity assertion changed.

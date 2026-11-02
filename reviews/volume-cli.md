# Rust review: volume commands and terminal helpers

Reviewed the native CLI application layer using rust-port, rust-best-practices,
and rust-review. The six volume commands share the existing client, typed private
command state, JSON/field output, and an owned terminal adapter. The only public
entry point remains `cli::run`; no prompt-backend types enter the SDK API.
Edition 2024 and MSRV 1.94.1 are unchanged. Dialoguer adds native fuzzy selection
and confirmation with its editor/password features disabled. No unsafe code was added.

## Source and scope

Source: [`runpod/rp_cli/volume.py`](https://github.com/runpod/runpod-python/blob/ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5/runpod/rp_cli/volume.py),
`rp_cli/api.py`, and the fields/terminal subset of `rp_cli/output.py`, at the existing
SDK pin. The Python checkout remains unchanged. The ledger maps 12 volume symbols,
two API helpers, and four output helpers. Volume ID completion, general completers,
and shell integration remain pending, with their source tests explicitly unmapped.

## Must fix

None outstanding. Review caught a global-volume listing that built full detail fields
before truncating them for a table. That incorrectly required `createdByUserId`, which
the source list view never reads. The list now reads only ID, name, and creation date;
an additional Python fixture covers a record without creator details.

**Rule:** Preserve observable field access and valid inputs (rust-port section 1).
Corrected in `src/cli/volume.rs`, `Options::execute`'s list branch.

Growth syntax is validated before constructing the client. Existing size is read
before deciding whether the requested change would shrink, do nothing, or exceed
4096 GB. Addition is checked; invalid changes cannot reach PATCH. Declined or unavailable
confirmation cannot reach DELETE. Required JSON fields use fallible accessors.

## Should fix

None outstanding. The first HTTP authentication fixture raised a mocked arbitrary
`AuthenticationError("bad key")`, unlike Python's actual transport, which always uses
its fixed 401 message. The generator now invokes the pinned `_raise_for_error` helper
for error responses. The private error-wrapper test separately proves that a supplied
authentication message is preserved by the CLI conversion.

**Rule:** Compare equivalent inputs at equivalent boundaries (rust-port test fidelity).
Corrected in `tools/volume_cli_fixtures.py`; the adapted source-test mapping explains it.

## Consider

- Native Dialoguer rendering/key handling replaces Click/InquirerPy. Both terminal
  streams must be TTYs; labels are truncated by display width and remain separate from
  stable selection IDs. Prompt layouts are intentionally native, with behavioral
  choices/confirmation covered by fixtures and a real Windows terminal smoke check.
- Growth uses checked ASCII `u64`; bounded create/update sizes use `u32`. Unbounded
  Python integers and Unicode decimal digits are documented adaptations, following
  rust-port's instruction not to emulate the source runtime for exotic inputs.
- Malformed page/record fields produce typed errors instead of Python lookup/type
  exceptions. Valid records preserve unknown JSON fields and source request ordering.
- Shared CLI pagination deliberately retains the source's simple cursor loop and no
  added retry policy. It does not silently substitute the SDK pagination API, whose
  retry and repeated-cursor contracts differ from this CLI helper.
- Pedantic review reports `clippy::too_many_lines` for the declarative command tree
  (`src/cli/volume.rs:5`) and six-way command dispatcher (`src/cli/volume.rs:331`).
  The related declarations and branches remain together for clarity; no lint
  suppression was added. Strict default Clippy passes, and there are no other new
  pedantic diagnostics. This application code favors direct dispatch over extra
  abstraction (rust-best-practices section 1).
- The volume implementation is approximately twice the nonblank source command size;
  common terminal and pagination helpers are small private modules. No general-purpose
  CLI framework or Python object model was introduced.

## What's already good

- Tests compare all HTTP methods, paths, query parameters, bodies, request counts,
  output values, candidate order, confirmation messages, and exit codes.
- An exact data center skips catalog lookup; name-only updates skip size lookup;
  `--yes` skips the confirmation read. Those source distinctions remain observable.
- Same-size update is allowed while same-size expansion is rejected, preserving the
  source's deliberately different contracts.
- Terminal dependencies and test fakes stay behind one private interface. The real
  native backend was also exercised; tests do not merely assume that it can prompt.

## Verification

- Test-first process execution failed all 19 new tests because `volume` was absent.
- The unchanged upstream volume/API/output files pass **67 parameterized cases**,
  including completion tests that are still pending in the Rust ledger.
- **58 actual Python command fixtures** regenerate byte-identically. Nineteen process
  tests and three private interactive-command counterparts cover the volume cases;
  eight additional helper tests cover fields, prompts, pagination, and error conversion.
- Existing catalog tests pass after dispatch integration. The full Windows Rust 1.99.1
  suite passes **670 tests**, zero failures, and two ignored child entry points exercised
  by their parent tests. This count includes the two passing doctests.
- Locked build, formatting, strict all-target/all-feature Clippy, and rustdoc pass.
  Pedantic diagnostics were reviewed as described above. Platform/MSRV results are
  established by the new commit's CI rather than inferred from local Windows success.
- A real Windows pseudo-terminal smoke test declined deletion (GET only), accepted
  deletion (GET then DELETE), and selected a volume by typing `models` into the native
  fuzzy picker (list GET then detail GET). All requests hit an ephemeral loopback
  mock server with synthetic credentials; no Runpod resource was changed.

Ignored logs and the terminal smoke script are in `test-results/volume-*`.
The compact build profile stays enabled and package artifacts are cleaned after validation.

# Rust review: directory volume cache

Reviewed the library API, native archive/transfer implementation, fixtures, and
example using rust-port, rust-best-practices, and rust-review. Edition 2024 and
MSRV 1.94.1 are unchanged. No dependencies or unsafe code were added.

## Source and contract

The Python SDK remains pinned at `ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5`.
The reference is
[`rp_volume_cache.py`](https://github.com/runpod/runpod-python/blob/ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5/runpod/serverless/utils/rp_volume_cache.py)
and its source tests. The Python checkout is unchanged.

The port retains the manifest/archive/mirror layout, 256 KiB partition, two-second
mtime tolerance, prior-source metadata comparisons, deletion-triggered repacking,
pack-failure fallback, failed-copy omission, ranged hydration, and background sync.
All 48 module symbols have explicit native mappings; 84 source test functions are
mapped (64 ported and 20 adapted). Thirteen source tests remain pending. These
counts do not claim completion of the SDK or of every filesystem failure case.

## Must fix

None outstanding in this scope. Review fixed ASCII JSON escaping for Python's
locale-based reader, including surrogate pairs; pinned the archive descriptor
across header validation and extraction; and rejected cross-drive cache paths and
Windows drive-relative namespaces. Source/destination boundaries resolve symlinks
before checking directory membership. These checks do not claim protection from
concurrent adversarial filesystem mutation.

Sparse inputs produce regular archive entries. Hard-linked inputs produce link
entries, preserving the source's behavior of restoring only regular members.
Nonregular files are rejected before opening the big-file copy source, avoiding a
blocking FIFO read; small archive extraction skips special members. Unix data-filter
permissions and fractional PAX timestamps are retained.

Prepared range temporaries immediately have RAII ownership. File publication is
atomic, and the manifest is written last. Checked offsets avoid range overflow;
partial writes and interrupted reads are handled. Each range uses independent
handles and checks source size/mtime. Scoped workers stop accepting work on failure
or unwind, and empty tasks create no workers. Pending-sync locks are released
before waiting; background closures do not own their pending registry.

## Should fix

None outstanding. Strict Clippy and pedantic review informed naming, ownership,
documentation, and test cleanup. Dense preparation code was expanded for readability.
No lint suppression was introduced.

## Deliberate native adaptations and limits

- Owned configuration, native paths, and `NonZeroUsize` replace Python scalar/bytes
  dispatch and runtime worker-count validation. JSON interchange requires UTF-8
  paths; Python surrogate strings and non-UTF-8 native names are outside that domain.
- Native tar/PAX replaces subprocess selection and listing files. Archive byte
  identity is not promised. Independent seekable handles replace POSIX-only ranged
  I/O; Windows errors are not interpreted as POSIX errno values.
- Clones share an owned pending registry. Explicit `flush` and last-owner drop
  replace a process-global exit hook. A scope guard hydrates on entry and schedules
  sync on drop. Blocking operations belong outside async executor threads.
- Native panics unwind, and signals are not translated to Python exceptions.
  Forced process termination does not promise destructor cleanup. Timed-out syncs
  detach; callers should flush before exit.
- Per-file errors and metadata-copy failures remain best-effort. Operation-level
  errors propagate when configured. No cross-file transaction or writer lock is
  added, and orphaned big files are not pruned. Size/mtime identity deliberately
  excludes inode numbers and does not guarantee content integrity.
- No live FUSE/network volume, billable resource, or performance benchmark was
  used. Additional metadata-failure injection, interrupted waits, environment
  defaults, and path-type cases remain pending in the source-test ledger.

## Verification

- The unmodified Python cache tests on this Windows environment produced
  **99 passed, 18 failed, 1 skipped**. Failures include POSIX ranged-I/O/mode
  assumptions and Windows archive paths; the source suite is not reported as green.
- **154 differential cases** characterize actual Python comparison, range-order,
  and hard-link behavior. Regeneration is byte-identical.
- **89 Windows cache tests** pass: 60 internal tests and 29 public integration
  tests. Additional Unix tests cover symlink boundaries and FIFO behavior in CI.
- The full Windows Rust 1.99.1 suite passed **593 tests**, zero failures, and two
  ignored child entrypoints exercised by their parent tests. All 89 cache tests
  and the Python interoperability check passed again after the final transfer
  refinements.
- Python reads native manifest/archive/big-file output, and Rust restores a
  Python-written PAX archive and ranged-file contents, checking bytes and mtimes.
  Stable Linux, Windows, and macOS CI now run this interoperability check.
- Build, formatting, rustdoc, and strict all-target/all-feature Clippy pass.
  Pedantic review ran. No second local MSRV build cache was created.
- Previous commit `54bdb19` passed all four CI jobs: stable Linux/Windows/macOS
  and Linux MSRV. This change's platform results belong to its own CI run.

Logs remain in ignored `test-results/volume-cache-*`. Generated crate artifacts
are cleaned after validation within the verified workspace target directory;
dependency caches and logs remain. Incremental compilation stays disabled.

## Unix CI follow-up

The first cache commit's Linux jobs exposed a tar-crate special-file path issue:
its `append_path_with_name` implementation ignores the requested archive name for
FIFOs/devices, then rejects the absolute source path. That rejected the entire
small bucket. The correction reuses tar's platform metadata/header construction
in a temporary in-memory archive and writes the header with the relative member
name. No FIFO/device data is opened. Sockets are omitted, matching Python, so
their presence cannot discard regular members. Regression tests inspect FIFO
and device names/types and restore a regular member alongside a socket. Unix
execution is verified by the corrective commit's CI, not the Windows test run.

# Rust review: file downloads and ZIP extraction

Reviewed all five Python download functions and their native Rust equivalents using the
Rust porting, best-practices, and review skills. Stable/MSRV tests and normal Clippy pass;
full-suite and documentation verification are recorded in PORTING.md. No blocking finding
remains. The most important contracts are distinguishing policy errors from retryable HTTP
failures, preserving batch order, and ensuring local extraction does not create symlinks.

## Must fix

None outstanding. Review/tests corrected Windows returned paths with trailing dots, ZIP
overwrite order after duplicate-name coalescing, drive/extended-UNC sanitization, and buffered
file writes on an early read error. The latter now flush before returning the original error
unless flushing itself fails, corresponding to Python's file-context exit behavior.

## Should fix

None outstanding. Batch work uses unordered bounded execution and restores result order
after every item finishes. It does not accidentally delay new items behind the first slow
request or drop sibling operations when one returns an error. HTTP-only retries retain one
UUID per input; SSRF, random-source, and filesystem failures propagate immediately for that
item. Single-file status checks occur before file creation and extraction.

## Consider

- The SDK faithfully retains two upstream policies: ZIP expanded size has no limit, and
  job IDs are trusted filesystem paths, not confined identifiers. These are documented in
  the module, README, and PORTING.md. Changing them would be a separate behavior decision.
- ZIP filenames are sanitized using the source platform rules. Link metadata is ignored
  and its contents are written as an ordinary file. Extraction uses a newly created UUID
  directory and assumes no other process is racing to replace its components.
- The ZIP library coalesces raw duplicate names. Sorting by their last central-directory
  offsets preserves tested final-write precedence, including aliases such as `./name`.
  Repeated intermediate writes and partial output on conflicting entries can differ; these
  are explicit native adaptations rather than a claim of byte-for-byte filesystem traces.
- Local ZIP parsing/decompression runs on a blocking worker. A cancellation flag is checked
  between entries and reads, while HTTP futures and queued items are canceled immediately.
  No blocking cleanup is performed in Drop, and partial files remain caller-owned.
- Native HTTP framing rejects malformed Content-Length before the chunk-size helper runs.
  The upstream mocked-header cases are mapped to the helper fixtures. The helper accepts
  signed ASCII i128, not Python's unbounded integers or dynamic header types.
- Native user-agent, tracing diagnostics, owned environment snapshots, path normalization,
  and jitter RNG replace Python process-global and library-specific details. All such
  adaptations are recorded in the ledger. JSON metadata keeps the source dictionary names.

## Verification

- All nine upstream tests pass. Fifteen native integration tests and one new unit test cover
  ordered concurrency, missing inputs, exact retry counts, policy failures, partial writes,
  HTTP status precedence, cancellation, ZIP errors and CRCs, and filesystem-error propagation.
- Sixty-six captured Python cases cover helper boundaries, original/returned names and
  extensions, signed/Unicode URLs, and actual extracted files on Windows/POSIX path rules.
  The ZIP includes stored/DEFLATE/BZIP2/LZMA, ZIP64, duplicates, aliases, UNC/device roots,
  parent paths, Unicode, and symlink metadata. Fixtures use fixed timestamps.
- Tests use local servers and temporary directories. No Runpod resource, external download,
  Python SDK subprocess, unsafe Rust, cwd mutation, or environment mutation is required.

## What's already good

Pedantic review moved the reusable 64 KiB extraction buffer to the heap, avoiding a large
stack allocation and repeated per-member allocation. Remaining test-style suggestions are
advisory; normal Clippy warnings are denied.

Existing SSRF policy is reused without widening its error-catching boundary. Owned options
make tests deterministic without globals, and nonzero concurrency is enforced by the type.
Return values expose all original metadata, downloaded ZIPs are retained, and archive codecs
come from a maintained Rust library with a verified Rust 1.88 minimum version.

## Follow-up: POSIX fixture capture

CI run 41056779984 exposed a fixture-generation error: ZipInfo normalizes names at
archive-open time using the host os.sep. The generator opened the archive before switching
to simulated POSIX rules, turning a literal backslash into a slash. Move archive opening
inside the platform patch. The ZIP and native extraction code are unchanged. All 19 expected
POSIX files now match actual Linux Python extraction in WSL; Windows download tests pass.

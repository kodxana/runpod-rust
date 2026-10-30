# Rust review: native file and S3 uploads

Library review using the Rust porting, best-practices, and review skills. Scope: nine Python
upload functions, the native upload module, error integration, dependency/MSRV change, and
new tests. The remaining SDK port is still in progress.

## Must fix

No outstanding finding. Review corrected content-only local copies: std::fs::copy also copies
permissions, while Python shutil.copyfile does not. The native implementation checks same-file
identity before opening the destination, rejects Unix FIFOs, streams contents, and flushes
buffered output before returning read errors. A Unix test checks existing destination modes.

## Should fix

No outstanding finding. Credential/environment/ExtraArgs values are excluded from Debug.
Typed UploadError retains an underlying AWS error and HTTP status without printing endpoint,
key, credential, or response body in Display (C-GOOD-ERR). Unknown ExtraArgs are rejected;
operation-specific forwarding does not quietly discard supported metadata/checksum options.

Multipart cleanup uses owned state and an async abort; Drop does not block. Parts are canceled
before explicit failure cleanup and sorted before completion. File bodies reopen their exact
ranges on retries. Source bytes remain borrowed across a transfer; each memory part owns its
request bytes. No unsafe code, global environment/cwd mutation, or Python runtime is required.

## Consider

- Native AWS signing, default headers, retry classification, checksum framing, and transport
  internals replace boto3 behavior. Object keys, source metadata, transfer thresholds, retry
  count, and presign lifetime are verified. This is not a claim of identical HTTP bytes.
- A canceled create request may reach S3 without returning its UploadId. Cancellation cleanup
  is best effort once the ID is known, and requires the Tokio runtime to remain alive. Tests
  verify active-part cancellation sends abort and never completes the object.
- Inherited trusted-path and local month-based bucket behavior is retained and documented.
  A region-only DigitalOcean endpoint returns the source's surprising region string.
- Runtime Python dependency import failure becomes compile-time Cargo dependency resolution;
  that upstream test is explicitly adapted in the ledger rather than counted as a ported test.
- The current official AWS SDK requires Rust 1.94.1, which is declared in Cargo and CI. No
  application account credentials or cloud resources are needed to run tests.

## Verification

Ten upstream Python tests pass. The fixture tool captures 47 cases without network requests,
including every supported ExtraArgs key from the installed boto3 transfer implementation.
Eighteen Windows integration tests cover real local S3 requests and returned presigned URLs;
an additional Unix-only test covers copy permissions. All existing download tests also pass.
The complete Windows stable suite passed 376 tests. Final upload checks pass on stable and
Rust 1.94.1, with formatting, build, Clippy (warnings denied), and documentation checks passing.
Pedantic review's cast warnings refer to private range bounds derived from the slice length
and the checked 10,000-part ceiling; no truncation path is exposed by user input. Other
pedantic suggestions concern documentation sections and style. Details are in PORTING.md;
Unix-specific validation still awaits CI.

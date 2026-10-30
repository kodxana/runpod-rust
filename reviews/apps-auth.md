# Rust review: browser and email-code authentication

Native library scope: browser approval polling, isolated email-code transport, verifier and
challenge generation, retries, and sanitized login errors. Reviewed against pinned Python
and the Rust porting, best-practices, and review skills. Tests and Clippy were run; no blocking
finding remains. Credential persistence and interactive CLI presentation are separate work.

## Must fix

None. The implementation reuses an explicitly anonymous connection pool with redirects and
automatic retries disabled. Existing authorization, cookies, and netrc state cannot enter
the email exchange. The verifier is sent only to the encoded verification path, never the
creation request, URL, or diagnostic messages.

## Should fix

None. Tests use returned HTTP status and actual request data, including UUID version/variant,
verifier entropy format, SHA-256 challenge equality, body fields, headers, and request counts.
Callback cancellation and transport errors do not submit extra verification requests.

## Consider

- Browser expiry deliberately accepts offset-aware RFC 3339, rather than every Python ISO
  date spelling. Unsupported timestamps are ignored. Elapsed deadlines are monotonic and
  keys are native strings; these adaptations are explicit in the parity ledger.
- The asynchronous email-code reader prevents library-controlled blocking input. Caller
  errors propagate unchanged. The SDK returns keys without opening browsers or storing them.
- Media-type checks preserve the pinned aiohttp JSON contract; native JSON decoding is UTF-8.
  Transport and malformed-response errors intentionally suppress detailed bodies and URLs.
- Pedantic review prompted simpler option handling, moving the constant regex, and avoiding
  per-byte formatting allocations. The explicit retry `continue` and test-style suggestions
  remain advisory because the control flow is clearer as written.

## Verification

- All 39 upstream cases pass. Twenty Rust tests exercise local HTTP services and connection
  failures, including status matrices, redirect refusal, repeated code rejection, aborted
  prompts, malformed credential bodies, deadline precedence, and cancellation during polling.
- Stable/MSRV and complete-suite results are recorded in PORTING.md. No real login request,
  email, browser approval, or credential write was performed.

## What's already good

- Random verifier and UUIDv4 values use the OS random source; no cryptographic primitives
  are reimplemented. Each login is isolated; rejected codes keep the same proof and attempt.
- Public Debug output contains no verifier, code, or granted key. No unsafe code, threads,
  dependencies, or mutable process globals were added.

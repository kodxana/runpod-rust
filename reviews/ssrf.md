# Rust review: download security transport

Reviewed the native async SSRF transport against pinned Python, the Rust porting skill,
best-practices skill, and review skill. Tests, formatting, documentation, and Clippy were
run; no blocking finding remains. The key correctness properties are all-answer validation,
pinning without disabling TLS verification, and explicit ownership of response cleanup.

## Must fix

None outstanding. Review/tests caught and fixed an IPv6 exception transcription error,
duplicate Host headers, connection/header timeout misclassification, and delayed resource
release after a stream yielded an error. Transport objects are now dropped before error
delivery. A caller may retain the error-yielding stream without retaining its socket.

## Should fix

None outstanding. Local HTTP test helpers now consume complete request headers, avoiding
partial-read assumptions. Literal URLs are normalized and classified independently of an
injected resolver, so a resolver cannot validate one address while the client dials another.
Address literal constants are grouped by hextet for review, and error-returning APIs document
failure behavior. Process environment collection skips non-Unicode entries rather than panicking.

## Consider

- The pinned Python classifier considers deprecated IPv6 site-local addresses global. This
  inherited behavior is retained and documented; silently changing it would violate the port
  contract. The 498 fixtures cover range boundaries, exceptions, mapped addresses, scopes,
  seeded random addresses, configuration, and Host construction.
- Reqwest's [DNS overrides](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html#method.resolve)
  preserve the URL identity while replacing the dial address. Local TLS tests assert SNI and
  Host, reject a wrong certificate name, and reject an untrusted root. Trust can be extended
  explicitly, but verification cannot be disabled through this API.
- Reqwest starts the header timer before connection establishment. An atomic completion
  marker permits fallback on a timeout before connecting but never after connecting. No
  generic timeout retry is used. The conservative timing difference is documented.
- Automatic decompression removes encoding/length headers. Compressed responses enforce the
  decoded-body cap; they cannot use Python's raw compressed-length precheck. Native HTTP
  parser failures and chunk boundaries also differ. These are marked adaptations, not exact
  wire parity. Gzip expansion and unknown-length/chunked bodies are tested.
- Proxy configuration is owned and explicit. No OS proxy registry/PAC, account credentials,
  netrc, cookies, automatic redirect, Referer, or request retry state is imported. Opting into
  private URLs also permits a configured proxy; that deliberately forfeits pinning through
  the proxy. Callers' explicit headers follow redirects, as in Python.
- Async interfaces replace synchronous sessions/iterators. `SafeResponse` never exposes an
  uncapped reader. Caps fit u64 and cannot be loosened after fetching. Source error strings,
  Python monkey-patching, and teardown exceptions are represented by typed errors and owned
  infallible destructors. These distinctions are recorded in the parity ledger.
- The upstream metadata-redirect test is mapped to two native checks: repeated resolution
  for a redirect and immediate rejection of metadata IP URLs. No public endpoint is dialed
  to manufacture a local redirect test. Tests contain intentionally public fixture TLS keys.

## Verification

- Twenty-one Rust integration tests; all 26 upstream Python tests; 498 captured Python cases.
- Complete stable suite and Rust 1.88 results are recorded in PORTING.md. Only local servers
  and explicit test resolvers were used. No Runpod resources or external downloads were used.
- Normal Clippy warnings are denied. Pedantic review prompted literal grouping, explicit
  error docs, clearer variable names, and test-read fixes; remaining test-style suggestions
  are advisory. No unsafe code or mutable process-global configuration was added.

## What's already good

Policy errors cannot be mistaken for `Error::Http`, rejected mixed DNS results fail before
any connection, and redirects cannot reuse the previous connection's DNS decision. Owned
transports make normal return, rejection, cancellation, partial reads, and errors share the
same cleanup rules. Compression reuses maintained libraries instead of custom codecs.

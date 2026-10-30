# Public test TLS fixtures

These certificates and the intentionally public `PUBLIC_TEST_ONLY_key.der` serve only local
SDK integration tests. They have no connection to an account or deployment. Never use the
key for a real service. The server certificate identifies `download.test` and is signed by
the test-only CA. Validity is 2020–2045 so CI does not depend on the generation date.

Optional regeneration: `python tools/ssrf_tls_fixtures.py` with `cryptography` installed.
Normal Rust tests need neither Python nor openssl. Regeneration changes the random keys.

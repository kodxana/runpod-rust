# Porting status and behavior

Source: `runpod/runpod-python` commit `ba938cfa4e9486a2dc2c1d3264334a1f689c7ea5`.
The source inventory contains 1,135 classes/functions, of which 662 have public names,
and 1,659 test functions. These counts include internal implementation modules exposed
by Python; the complete port remains in progress.

## API mapping

| Python | Rust |
| --- | --- |
| `runpod.api_key`, base URL globals | owned `ClientConfig` |
| `api.rest.run_rest_request`, `run_rest_request_async` | `Client::run_rest_request`, `blocking::Client::run_rest_request` |
| `api.rest.run_rest_paginated_request_async` | `Client::run_rest_paginated_request` |
| `api.graphql.run_graphql_query`, `run_graphql_query_async` | `Client::run_graphql_query`, `blocking::Client::run_graphql_query` (anonymous mode pending) |
| All 17 `api.ctl_commands` functions | identically named `Client` methods; option bags replace long parameter lists |
| `endpoint.runner.Endpoint` | `Endpoint` (async Rust methods), `blocking::Endpoint` |
| `endpoint.runner.Job` | `Job`, `blocking::Job` |
| `endpoint.asyncio.Endpoint`, `Job` | `AsyncioEndpoint`, `AsyncioJob` |
| `endpoint.helpers.is_completed`, `FINAL_STATES` | `endpoint::is_completed`, `endpoint::FINAL_STATES` |
| AuthenticationError / QueryError | `Error::Authentication` / `Error::Query` |

The detailed symbol and test ledgers are [parity/api.json](parity/api.json) and
[parity/tests.json](parity/tests.json). Pending entries must not be interpreted as
implemented merely because a broad Rust module exists.

## Test mapping and reproduction

`tests/transport.rs` ports REST authentication, custom URL, empty response, problem
JSON, fallback errors, and pagination contracts from `tests/test_api/test_rest.py`.
`tests/endpoint.rs` ports the endpoint/job contracts in `tests/test_endpoint/test_runner.py`
and async-specific behavior in `test_asyncio_runner.py`, plus blocking streaming.

`tests/control.rs` replays 47 real Python wire fixtures in `tests/fixtures/control.json`.
Each fixture records its upstream test name where applicable, inputs, HTTP requests,
server replies, and returned output or error. They cover every ctl_commands operation,
template precedence, per-GPU host minima, storage, registry credentials, GPU exclusions,
CPU parsing, and rejected legacy parameters.

Regenerate with the upstream SDK installed in an isolated environment:

```sh
python tools/inventory.py ../runpod-python
python tools/control_fixtures.py
```

## Preserved quirks

- `Endpoint.run` wraps requests whose `input` field is absent or false-like, including
  empty objects. `AsyncioEndpoint.run` always wraps its argument.
- CANCELLED is terminal for status polling but absent from the Python stream terminal
  list. Streaming therefore retains that behavior; dropping the Rust stream cancels it.
- The synchronous-compatible job output uses zero timeout for a single immediate fetch.
  AsyncioJob instead times out immediately at zero unless a non-null output is cached.
- Null cached output causes another fetch; false/zero/empty-string outputs are cached.
- CPU instance memory is validated syntactically but not sent to REST v2.
- Unsupported legacy pod/resume parameters remain errors rather than being ignored.
- Template command `None` inherits, while an empty string explicitly clears it.

## Deliberate language/runtime differences

- Configuration is owned per client. Rust does not mutate module globals or execute
  fitness checks at import time. No Python interpreter is embedded or launched.
- API results remain JSON values to preserve unknown server fields. Options use typed
  structs. Timeouts use Duration; sizes/counts use bounded unsigned integers.
- Job wait deadlines use monotonic time including HTTP time, instead of Python's
  decrementing integer counter. Network errors are returned through Result.
- Transport errors for non-401 GraphQL HTTP failures are reported before JSON parsing.
  Malformed responses return errors rather than Python KeyError/IndexError.
- Redirects are disabled, and mutations are never implicitly replayed after ambiguous
  failures. Pagination alone retries transport failures, matching upstream.
- Environment scalar coercion preserves Python True/False/None. Complex nested values
  currently use JSON formatting rather than Python repr; this remains tracked work.
- Blocking clients own a Tokio runtime and must be constructed/used outside async tasks.

## Verification log

- Windows x86_64, Rust 1.99.1: initial transport/control tests and Clippy passed.
- Python 3.13.15: API/endpoint/error subset reported **157 passed**. The command's
  exit status was nonzero because upstream enforces 90% whole-repository coverage even
  for this subset (5% overall). The broader offline suite is being evaluated separately.
- Live Runpod deployment and GPU execution have not been performed.

This file will be updated as each remaining component and test is ported.

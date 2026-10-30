# Rust review: native API declarations and HTTP handles

Library scope: NativeApi/ApiHandle, shared function/API registration, resource snapshots,
and live-source attachment. Edition 2024, MSRV 1.88. Reviewed with the Rust porting, best
practices, and review skills. No blocking finding remains. Native HTTP hosting, request
binding, native binary deployment, and blocking registered handles remain pending.

## Preserved contracts

- Native API routes validate the same five methods and three reserved control paths.
  Duplicate verb/path pairs and multiple differently named initialization hooks fail.
  Route metadata sorts by handler name; replacing a named member replaces its route.
- Queue, task, and API resources share one duplicate-checking namespace and preserve app
  registration order. ASGI factory qualified-name metadata survives into resource specs.
- HTTP client methods do not call local native bodies, even in their own worker. Discovery
  guards precede network calls. Live calls share source synchronization; failure blocks the
  route call and leaves the next call able to retry. Typed secret metadata reaches deployment.

## Review fixes

- Unified registrations behind an internal resource enum; returned ResourceHandle snapshots
  retain the actual handle kind without exposing registry locks or type erasure.
- Target and event replacement now releases app locks before dropping captured user values.
  The regression unit test uses reentrant destructors and try_lock, so a failure produces an
  assertion instead of hanging. Hook replacement is covered by the same test.
- NativeApi Debug omits source and captured method state. ApiHandle/NativeApi remain Send/Sync.
  Initialization and body calls operate on cloned registrations after locks are released.
- Pedantic review's clone-assignment suggestion was applied. No public allocator or mutex
  types were added, and normal Clippy passes without lint suppression.

## Adaptations and limits

- Explicit route/hook methods replace marker decorators and class inspection. Closures own
  instance state. Rust method registration performs validation as declarations are added.
- NativeApi::asgi_factory is interoperability metadata only. Native initialize/call_handler
  reject such factories; Rust does not import or execute Python classes or ASGI applications.
- call_handler accepts explicit FunctionArgs. HTTP routing, body decoding, response binding,
  and serving are separate pending runtime work, not implied by registration support.
- App::resources now returns mixed ResourceHandle values. This is an unreleased SDK under
  construction; users of the preceding function-only milestone must match the enum.

## Verification

- Nine API integration tests plus the existing eighteen function tests pass with local HTTP
  services. The same twenty-seven tests pass on Rust 1.88. Fifteen Python marker tests pass.
- Full stable verification passes: 248 integration tests, two unit tests, and two doctests,
  plus build, formatting, normal Clippy, and docs. Both unit tests also pass on Rust 1.88.
  No real cloud deployment, Python factory execution, or native HTTP server was performed.

## What's already good

- Tests inspect actual HTTP requests and enforce one shared sync across all five verbs.
- Mixed registries retain ordering, resource kinds, and explicit secret/source metadata.
- Registration has no execution or credential side effects; local startup is explicit.

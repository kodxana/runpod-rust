# Rust review: Apps placement and volume provisioning

Reviewed the native library changes following commit 7885fa7 against the rust-review and
rust-best-practices skills. The crate uses edition 2024 and declares Rust 1.88 as its MSRV;
this review ran on Rust 1.99.1. Tests, formatting, normal Clippy with warnings denied,
and a separate pedantic Clippy pass were run. The main correctness finding was a
deserialization path bypassing validated volume constructors; it is fixed and tested.

## Must fix — resolved

### 1. Deserialization bypassed reference validation (`src/apps/volume.rs:38`, `src/apps/volume.rs:70`)

**Rule:** Validated types preserve invariants through every constructor (Rust API Guidelines C-VALIDATE).

**Problem:** Derived Deserialize could construct a blank reference even though `new` rejected it.
Such a reference could reach provisioning through a deserialized resource specification.

**Fix:** Both reference fields use a validating deserializer; the regression test first failed
and now rejects empty and whitespace-only references.

```rust
#[serde(deserialize_with = "deserialize_reference")]
reference: String,
```

## Should fix — resolved

### 2. Wrapped catalog failures lost their cause (`src/apps/resolution.rs:159`)

**Rule:** Errors retain underlying causes (Rust API Guidelines C-GOOD-ERR).

**Problem:** Converting a catalog failure into an InvalidInput string discarded source and status.

**Fix:** Error::Context preserves the boxed cause and delegates status_code. Tests verify the
actionable message and the presence of the source error.

```rust
Error::Context { message, source: Box::new(error) }
```

### 3. Retry test relied on wall-clock speed (`tests/apps_api.rs:338`)

**Rule:** Tests exercise observable behavior, not environmental timing (rust-best-practices, Tests).

**Problem:** A one-second elapsed-time assertion could fail under scheduler load.

**Fix:** The fixture closes its listener after a truncated response. The test checks that
the original transport failure is retained; a replay would replace it with a connection failure.

```rust
assert!(matches!(&error, Error::Http(error) if !error.is_connect()));
```

## Consider

- The advisory stock cache deliberately marks requests fetched before they complete,
  preserving upstream cancellation behavior. Reusing that same cache after cancellation
  can retain zero scores; use a fresh provisioning context for a new attempt.
- The pedantic pass reports style advisories such as must_use suggestions and long
  functions, including pre-existing modules. These are not all resolved in this milestone.
- The declared MSRV and remote Linux/macOS CI still require verification in the final audit.

## What's already good

- Nineteen Python differential fixtures cover placement ranking, product/count separation,
  storage constraints, and exact conflict messages.
- Resource mutations are not automatically replayed; create=False and unsupported placement
  fail before provisioning.
- Volume caches separate backend kinds, and mount bindings retain reference and ID lookup.
- Rust owns provisioning state and event callbacks; no interpreter or process globals are required.

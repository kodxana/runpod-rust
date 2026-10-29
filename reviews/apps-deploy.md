# Rust review: deployment manifests and endpoint reconciliation

Library scope: AppDefinition/SymbolReference data, manifest generation, deployed endpoint
payloads, and reconciliation. Edition 2024, MSRV 1.88. The rust-port, rust-best-practices,
and rust-review skills guide this port. Complete App registration and artifact construction
remain separate pending work; these functions do not claim to implement that pipeline.

## Preserved contracts

- Forty-three Python fixtures cover ordered resources, runtime symbols, schedule rejection,
  minimum resource-name length, image/version/channel selection, workers, scaling, CUDA,
  datacenter defaults, environment override precedence, and custom runtime launch commands.
- Resource specs validate before reconciliation sends requests. Queue/API resources are
  saved in sorted name order, existing IDs are reused, and obsolete endpoints are deleted
  only after every save succeeds. Failure leaves already completed changes in place.
- One VolumeResolver is shared across resource saves. A test observes one volume lookup,
  shared stock lookup results, and both endpoints pinned to the volume's datacenter.
- Exact GPU selections use the explicit client's live catalog. Registry IDs and model
  references are attached before save. Tasks only appear in manifests, not endpoint saves.

## Review fixes

- Construct the custom-image environment as a typed Vec before serializing the payload,
  avoiding a JSON downcast/expect and preserving the upstream duplicate runtime-kind key.
- Store validated existing endpoint names/IDs as borrowed string pairs in insertion order.
  This avoids a second JSON downcast when deleting. Duplicate names update the last ID
  without changing deletion order, matching Python dictionary behavior.
- DeploymentConfig Debug exposes only override key names and credential presence.
  Environment injection is explicit through owned config; no mutable global API key is used.

## Verification and boundaries

- Five Rust deployment tests pass on stable and MSRV, including all 43 differential cases.
  Six matching upstream manifest/endpoint configuration tests pass on Windows.
- Full stable tests, build, fmt, normal Clippy with warnings denied, and docs are gates.
  Pedantic review found documentation and invariant-downcast issues, fixed above.
- Runtime symbols are explicit metadata; Rust does not reflect on or import Python functions.
  App registration, project packaging, dependency vendoring, and full deployment orchestration
  remain in the parity ledger. No live endpoint was provisioned or deleted by these tests.

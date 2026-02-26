# Worker 1 Implementation: `sieve-types`

Worker 1 scope: crate `crates/sieve-types` only.

## Implemented

- Added rustdoc comments across externally used contracts in `crates/sieve-types/src/lib.rs`.
- Kept schema/type naming stable; no schema renames.
- Added serde JSON round-trip unit tests for:
  - `ApprovalRequestedEvent`
  - `ApprovalResolvedEvent`
  - `RuntimeEvent` (`PolicyEvaluated` variant)
  - `EndorseRequest` + `EndorseResponse`
  - `DeclassifyRequest` + `DeclassifyResponse`
- Added schema conformance tests validating serialized approval events against:
  - `schemas/approval-requested-event.schema.json`
  - `schemas/approval-resolved-event.schema.json`
- Added `Hash` derives on ID/sink newtypes (`RunId`, `ValueRef`, `ApprovalRequestId`, `SinkKey`) for downstream `HashMap` usage.
- Added dev-dependency `jsonschema` in `crates/sieve-types/Cargo.toml`.

## Verification

- `cargo check`: pass.
- `cargo test -p sieve-types`: pass.
- `cargo test` (workspace): pass.

## Surprises

- Initial workspace `cargo check` failed before dependency fetch/escalation due to sandbox/network restrictions and Cargo cache write path access.
- After full build path, runtime compile surfaced contract coupling: `ApprovalRequestId` used as `HashMap` key in `sieve-runtime`, requiring `Hash` derive in `sieve-types`.
- `jsonschema` initial pin resolved to older line (`0.37.x`); bumped to `0.42.x`.

## Remaining TODO

- Optional hardening: add schema tests for additional event payloads if more schemas are introduced.
- Optional CI guard: require schema-validation tests in the Worker 1 gate for any approval-event contract change.

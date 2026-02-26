# Worker 7: `sieve-runtime` Implementation Notes

You are reading implementation notes for Worker 7 (`crates/sieve-runtime`).

## Implemented

- In-process `ApprovalBus` implemented as `InProcessApprovalBus`.
  - One-shot request/resolve semantics via `tokio::sync::oneshot`.
  - Duplicate request-id guard.
  - Published approval-event capture for tests.
- Runtime JSONL event logger implemented as `JsonlRuntimeEventLog`.
  - Append-only writes.
  - Parent directory auto-create.
  - Serialized `RuntimeEvent` per line.
- Orchestration skeleton implemented as `RuntimeOrchestrator`.
  - Shell analyze -> summary merge -> policy precheck.
  - Policy evaluated event emission.
  - `deny_with_approval` flow wired.
  - Composed command path consolidated into one approval request.
  - Unknown/uncertain mode behavior wired:
    - `deny` => denied disposition
    - `accept` => quarantine disposition
    - `ask` => approval-first, then quarantine on `approve_once`
- Explicit tool approval lifecycle hooks implemented:
  - `request_endorse_approval`
  - `request_declassify_approval`
- Runtime value-state engine implemented (Chunk F):
  - in-memory `RuntimeValueState` keyed by `ValueRef`
  - `upsert_value_label`, `value_label`, `runtime_policy_context_for_control`
  - `orchestrate_shell` now includes `runtime_context` in `PrecheckInput`
  - `ShellRunRequest` now carries control refs + optional endorsement source
  - one-shot transition helpers:
    - `endorse_value_once`
    - `declassify_value_once`
- Deterministic event timing support via injectable `Clock`.

## Tests Added

- Approval roundtrip integration-style test.
- Composed-command consolidated approval test.
- Unknown `ask` approval-before-quarantine test.
- Uncertain `ask` approval-before-quarantine test.
- Endorse approval lifecycle tests:
  - approve/deny coverage
- Declassify approval lifecycle tests:
  - approve/deny coverage
- Runtime value-state tests:
  - policy precheck receives runtime context from value-state
  - endorse transition updates integrity label on approval
  - declassify transition tracks first/duplicate sink allowance
- Approval bus concurrency test:
  - parallel pending requests, out-of-order resolves, no cross-delivery.
- Runtime JSONL event log ordering test.
- Approval request/resolution event schema-shape stability tests.

## Surprises / Gotchas

- Initial unknown/uncertain `ask` implementation ran quarantine before approval.
  - Fixed to match MVP semantics: approval gates execution.
- Environment dependency drift blocked tests temporarily:
  - `futures-sink = ^0.3.32` unavailable in current index view.
  - Unblocked locally by downgrading lockfile `futures-channel` to `0.3.31`.
- Workspace had unrelated untracked/partial files during work.
  - Runtime scope commits kept focused to `crates/sieve-runtime`.

## Remaining TODO (Worker 7 Scope)

- Add explicit schema validation against JSON schema files (not just shape/serde checks):
  - `schemas/approval-requested-event.schema.json`
  - `schemas/approval-resolved-event.schema.json`
- Add integration test using real filesystem JSONL logger in orchestrator path (not logger unit only).
- Add docs on runtime event ordering contract:
  - expected sequence per flow (allow/deny/deny_with_approval/ask/accept).
- Decide lockfile strategy for workspace dependency resolution consistency across environments.

## Done Criteria Status

- `cargo test -p sieve-runtime`: passing.
- In-process approval roundtrip: passing.

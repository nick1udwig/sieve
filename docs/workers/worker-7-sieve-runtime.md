# Worker 7: `sieve-runtime`

You are Worker 7 for Sieve v3. Own only crate `crates/sieve-runtime`.

## Read First

- `/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md`
- `/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs`
- `/root/git/sieve-v3/crates/sieve-types/src/lib.rs`
- `/root/git/sieve-v3/schemas/approval-requested-event.schema.json`
- `/root/git/sieve-v3/schemas/approval-resolved-event.schema.json`

## Mission

Implement tokio orchestration and in-process approval flow using stable event payloads.

## Scope

- Implement in-process `ApprovalBus`.
- Implement event logger for runtime JSONL events.
- Coordinate precheck and decision path and one-shot approval semantics.
- Ensure composed commands generate one consolidated approval request.
- Wire endorse and declassify request lifecycle hooks (approval required).
- Support unknown and uncertain ask mode behavior in orchestration.

## Required Outputs

- Runnable orchestration skeleton with integration points for shell, summaries, policy, quarantine, llm.
- Tests for approval request and resolve flow and composed-command consolidation.
- Deterministic event ordering.

## Out Of Scope

- Telegram adapter UI details.
- LLM internals.

## Definition Of Done

- `cargo test -p sieve-runtime` passes.
- In-process approval roundtrip works in integration test.

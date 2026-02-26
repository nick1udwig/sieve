# Worker 1: `sieve-types`

You are Worker 1 for Sieve v3. Own only crate `crates/sieve-types`.

## Read First

- `/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md`
- `/root/git/sieve-v3/crates/sieve-types/src/lib.rs`
- `/root/git/sieve-v3/schemas/approval-requested-event.schema.json`
- `/root/git/sieve-v3/schemas/approval-resolved-event.schema.json`

## Mission

Stabilize and finalize shared contracts so all other workers can build on them safely.

## Scope

- Keep and extend the data model for capabilities, labels, decisions, approvals, quarantine reports, endorse and declassify, planner and q-llm I/O.
- Add missing types only if needed by other crate interfaces.
- Preserve MVP decisions: unknown and uncertain `ask|accept|deny`, consolidated composed-command approval, in-process events, indefinite log retention metadata.

## Required Outputs

- Strong serde compatibility.
- Unit tests for JSON round-trip on runtime events and approval payloads.
- Clear rustdoc comments on all externally used structs and enums.
- Do not rename schema names unless necessary.

## Out of Scope

- Business logic in other crates.
- Runtime orchestration.

## Definition Of Done

- `cargo check` passes.
- `cargo test -p sieve-types` passes.
- No breaking changes without explicit coordination note in commit message.

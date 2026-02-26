# Sieve v3 Interface Freeze v0

Date: 2026-02-26

This file freezes initial crate boundaries and worker ownership so implementation can proceed in parallel with low merge conflict risk.

## Product decisions locked

- LLM provider in MVP: OpenAI only.
- Architecture keeps provider abstraction for future multi-provider support.
- P-LLM and Q-LLM are independently configurable.
- Core enforcement must pass tests before Telegram integration starts.
- Approval flow transport in MVP: in-process events (stable JSON payload).
- Unknown and uncertain modes both support `ask | accept | deny`.
- Composed commands in ask mode use one consolidated approval request.
- Log retention: indefinite in MVP.

## Crate ownership

- `crates/sieve-types`: shared data contracts, IDs, event payloads, policy decision model.
- `crates/sieve-shell`: bash subset parse + composition extraction + unknown/uncertain classification.
- `crates/sieve-command-summaries`: per-command capability and sink summaries (seeded from Codex snapshot).
- `crates/sieve-policy`: TOML policy load + integrity/confidentiality evaluator.
- `crates/sieve-quarantine`: bwrap + no-net + strace runner and normalized trace artifacts.
- `crates/sieve-llm`: OpenAI-backed planner/quarantine model adapters under provider abstraction.
- `crates/sieve-runtime`: tokio orchestration, consolidated precheck, approval wait path, event logging.
- `crates/sieve-interface-telegram`: Telegram adapter (starts after core enforcement gate).

## Merge safety rules

- Shared contract changes (`crates/sieve-types` and `schemas/`) require explicit coordination before merge.
- Each worker edits only its owned crate unless a contract change is approved.
- Root workspace files (`Cargo.toml`, lockfile) are integrator-owned.
- Cross-crate integration happens after per-crate unit tests pass.

## Integration gate sequence

1. `sieve-types` + schemas stabilized.
2. `sieve-shell` + `sieve-command-summaries` produce precheck inputs.
3. `sieve-policy` returns `allow | deny_with_approval | deny`.
4. `sieve-runtime` drives approval flow and event log.
5. `sieve-quarantine` wired for unknown/uncertain accepted runs.
6. `sieve-llm` integrated with OpenAI P/Q models.
7. Core enforcement tests pass.
8. Telegram adapter integrates with runtime event bus.

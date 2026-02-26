# Sieve v3 MVP Completion Plan

Date: 2026-02-26

This plan is organized into independent chunks that can be executed in parallel by separate workers with minimal overlap.

## Working Agreement

- Integrator-only edits:
  - `/root/git/sieve-v3/Cargo.toml`
  - `/root/git/sieve-v3/Cargo.lock`
  - `/root/git/sieve-v3/schemas/`
- Worker ownership:
  - one worker owns one chunk at a time
  - each worker edits only the crate/files listed for that chunk
- Contract-first sequencing:
  - if `sieve-types` contracts change, merge that first
  - dependent chunks rebase before merge
- Integration guard:
  - behavior-changing chunks must include tests in owning crate
  - end-to-end assertions land in integration harness chunk

## Chunk A: Contract Freeze v1

- Primary owner: `sieve-types`
- Scope:
  - finalize shared contracts for:
    - tool contract validation errors
    - control-context integrity
    - sink permission context
    - endorse/declassify state transitions
- Files:
  - `/root/git/sieve-v3/crates/sieve-types/src/lib.rs`
  - `/root/git/sieve-v3/schemas/*` (integrator gate)
- Depends on:
  - none
- Done when:
  - contract docs/types stable
  - schema tests pass

## Chunk B: Rust Tool Contracts + Schema Emission

- Primary owner: new crate `sieve-tool-contracts`
- Scope:
  - define Rust typed arg contracts per tool
  - generate JSON schema from Rust types
  - expose validator API:
    - `validate(tool_name, args_json) -> Result<TypedCall, ContractError>`
- Files:
  - `/root/git/sieve-v3/crates/sieve-tool-contracts/*`
- Depends on:
  - A
- Done when:
  - strict validation for MVP tools is in place
  - generated schema artifacts produced from Rust types

## Chunk C: Planner Regeneration Pass

- Primary owner: `sieve-llm`
- Scope:
  - on tool arg validation failure:
    - emit compiler-like diagnostic
    - include tool call index and actionable fix
    - include line/column/range when recoverable
  - allow exactly one regeneration pass
- Files:
  - `/root/git/sieve-v3/crates/sieve-llm/src/openai.rs`
  - `/root/git/sieve-v3/crates/sieve-llm/src/wire.rs`
- Depends on:
  - A
  - B
- Done when:
  - failure -> regenerate once -> success/final fail flow is tested

## Chunk D: Shell Parser Security-Equivalent Parity

- Primary owner: `sieve-shell`
- Scope:
  - AST-based parsing with `tree-sitter-bash`
  - reuse Codex logic where it works
  - maintain security-equivalent Sieve behavior for classification
- Files:
  - `/root/git/sieve-v3/crates/sieve-shell/src/lib.rs`
- Depends on:
  - none
- Done when:
  - known/unknown/uncertain mapping validated on parity corpus

## Chunk E: Command Summary Expansion

- Primary owner: `sieve-command-summaries`
- Scope:
  - broaden mutating and sink command summaries
  - tighten unsupported-flag unknown routing
- Files:
  - `/root/git/sieve-v3/crates/sieve-command-summaries/src/lib.rs`
- Depends on:
  - D
- Done when:
  - summaries cover MVP-critical command classes with tests

## Chunk F: Runtime Value-State Engine

- Primary owner: `sieve-runtime`
- Scope:
  - implement runtime value/provenance/sink state handling
  - stop relying only on static config for trust/sink decisions
- Files:
  - `/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs`
- Depends on:
  - A
- Done when:
  - runtime passes value-state context into policy flow

## Chunk G: Policy Uses Runtime Context

- Primary owner: `sieve-policy`
- Scope:
  - consume runtime-provided integrity/sink context
  - preserve TOML policy semantics
- Files:
  - `/root/git/sieve-v3/crates/sieve-policy/src/lib.rs`
- Depends on:
  - A
  - F
- Done when:
  - policy checks run against runtime context, not static placeholders

## Chunk H: Quarantine Connect Coverage + Mandatory Report

- Primary owner: `sieve-quarantine`
- Scope:
  - expand connect-attempt extraction visibility
  - mandatory `report.json` whenever trace artifacts exist
  - no DNS-specific feature work
- Files:
  - `/root/git/sieve-v3/crates/sieve-quarantine/src/lib.rs`
- Depends on:
  - none
- Done when:
  - trace run emits `report.json`
  - connect attempts are normalized with endpoint metadata

## Chunk I: Runtime End-to-End Planner Loop

- Primary owner: `sieve-runtime`
- Scope:
  - invoke planner from orchestrator
  - validate tool calls using strict contracts
  - execute policy/quarantine/approval in full loop
- Files:
  - `/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs`
- Depends on:
  - B
  - C
  - F
  - G
- Done when:
  - end-to-end planner loop runs through policy and approval paths

## Chunk J: Telegram Adapter Finalization

- Primary owner: `sieve-interface-telegram`
- Scope:
  - align Telegram adapter with finalized runtime event flow
  - contract validation failures remain internal-only (logged, not user-visible)
- Files:
  - `/root/git/sieve-v3/crates/sieve-interface-telegram/src/adapter.rs`
  - `/root/git/sieve-v3/crates/sieve-interface-telegram/src/transport.rs`
- Depends on:
  - I
- Done when:
  - approval interaction works against finalized runtime loop

## Chunk K: End-to-End Security Harness

- Primary owner: new crate `sieve-e2e` or runtime integration tests
- Scope:
  - black-box security/behavior regression tests:
    - `rm -rf` -> deny_with_approval
    - `curl POST -d` sink enforcement
    - unknown/uncertain ask/accept/deny paths
    - endorse/declassify one-shot approval behavior
    - quarantine artifact/report generation
- Files:
  - `/root/git/sieve-v3/crates/sieve-e2e/*` or `/root/git/sieve-v3/crates/sieve-runtime/tests/*`
- Depends on:
  - A through J as needed
- Done when:
  - full MVP-critical behavior is guarded by end-to-end tests

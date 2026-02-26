# Worker 6 Implementation Notes: `sieve-llm`

Worker: 6  
Date: 2026-02-26  
Scope owner: `crates/sieve-llm`

## Mission Status

Implemented real OpenAI-backed planner and quarantine adapters under provider abstraction with independent planner/quarantine config.

## What Was Implemented

- Concrete models:
  - `OpenAiPlannerModel`
  - `OpenAiQuarantineModel`
- Config loading:
  - `SIEVE_PLANNER_*` + `SIEVE_QUARANTINE_*` model/provider/api-base
  - scoped API key fallback to `OPENAI_API_KEY`
- OpenAI client integration:
  - `POST /v1/chat/completions`
  - retry on transient status and transport timeout/connect errors
  - bounded status-body error reporting
- Structured outputs:
  - planner JSON schema for `PlannerTurnOutput`
  - quarantine JSON schema for `TypedValue`
- Typed extraction enforcement:
  - only `bool | int | float | enum`
  - enum registry + variant validation at decode boundary
- Planner boundary enforcement:
  - planner input shaped from trusted user message + constrained metadata
  - previous runtime events reduced to event kind tags, not free-text payloads
  - planner output tool names checked against `allowed_tools`
- Tests:
  - config parse + provider validation
  - planner input serialization boundary check
  - planner decode path
  - quarantine typed decode path + invalid enum rejection
  - env-gated live quarantine smoke test
  - env-gated live planner smoke test
- Real integration path:
  - env-gated live tests
  - runnable example: `crates/sieve-llm/examples/openai_live.rs`

## Files Added/Changed

- `crates/sieve-llm/src/lib.rs`
- `crates/sieve-llm/src/config.rs`
- `crates/sieve-llm/src/openai.rs`
- `crates/sieve-llm/src/wire.rs`
- `crates/sieve-llm/src/tests.rs`
- `crates/sieve-llm/examples/openai_live.rs`
- `crates/sieve-llm/README.md`

## Surprises

- Workspace `cargo fmt` currently blocked by unrelated missing file in another crate (`crates/sieve-interface-telegram/src/adapter.rs`), so formatting was run directly on `sieve-llm` files with `rustfmt`.
- Local registry/index availability did not include `futures 0.3.32` while lockfile referenced it; lockfile needed normalization to `0.3.31` for local resolution.

## Remaining TODOs (Within/Adjacent to Worker 6)

- Optional hardening: planner output schema currently allows arbitrary `args` object; can tighten per-tool once runtime tool arg contracts are fixed centrally.
- Optional coverage: add deterministic mocked transport tests for retry backoff behavior (current tests focus on boundary/decode + env-gated live paths).
- Optional docs: add planner-specific example alongside quarantine example if desired for operator onboarding.

## Verification

- `cargo test -p sieve-llm` passes.
- Live paths are gated by:
  - `SIEVE_RUN_OPENAI_LIVE=1`
  - `OPENAI_API_KEY` (or scoped planner/quarantine key vars)

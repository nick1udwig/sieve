# sieve-llm

OpenAI-backed planner + guidance + response/summary model adapters for Sieve v3 MVP.

## Config

Planner config env:
- `SIEVE_PLANNER_MODEL` (required)
- `SIEVE_PLANNER_PROVIDER` (optional; default `openai`)
- `SIEVE_PLANNER_API_BASE` (optional)
- `SIEVE_PLANNER_OPENAI_API_KEY` (optional; falls back to `OPENAI_API_KEY`)

Guidance config env:
- `SIEVE_GUIDANCE_MODEL` (optional; falls back to `SIEVE_PLANNER_MODEL`)
- `SIEVE_GUIDANCE_PROVIDER` (optional; default `openai`)
- `SIEVE_GUIDANCE_API_BASE` (optional)
- `SIEVE_GUIDANCE_OPENAI_API_KEY` (optional; falls back to `SIEVE_PLANNER_OPENAI_API_KEY` then `OPENAI_API_KEY`)

LLM exchange logging env:
- `SIEVE_LLM_EXCHANGE_LOG_PATH` (optional JSONL file path)
- default when unset: `$SIEVE_HOME/logs/llm-provider-exchanges.jsonl` (or `$HOME/.sieve/logs/...`)

## Guarantees

- Planner input boundary: only trusted user message + constrained metadata shape.
- Planner tool-call args validated against strict per-tool contracts (`bash|endorse|declassify`).
- On planner tool-arg contract failure, one regeneration pass is attempted with structured diagnostics.
- Q-LLM -> planner boundary: typed numeric guidance signals only (`PlannerGuidanceSignal` + `PlannerGuidanceFrame`).
- No free-form strings cross from guidance model into planner context.
- OpenAI wire logs persist exact request JSON payloads and raw response bodies per attempt.

## Live smoke test

Env-gated OpenAI call path test:
- `SIEVE_RUN_OPENAI_LIVE=1`
- `OPENAI_API_KEY=...` (or scoped guidance key)

Run:
- `cargo test -p sieve-llm openai_live_guidance_smoke_env_gated -- --nocapture`

Live example:
- `OPENAI_API_KEY=... cargo run -p sieve-llm --example openai_live`

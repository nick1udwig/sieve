# sieve-llm

OpenAI-backed planner + quarantine model adapters for Sieve v3 MVP.

## Config

Planner config env:
- `SIEVE_PLANNER_MODEL` (required)
- `SIEVE_PLANNER_PROVIDER` (optional; default `openai`)
- `SIEVE_PLANNER_API_BASE` (optional)
- `SIEVE_PLANNER_OPENAI_API_KEY` (optional; falls back to `OPENAI_API_KEY`)

Quarantine config env:
- `SIEVE_QUARANTINE_MODEL` (required)
- `SIEVE_QUARANTINE_PROVIDER` (optional; default `openai`)
- `SIEVE_QUARANTINE_API_BASE` (optional)
- `SIEVE_QUARANTINE_OPENAI_API_KEY` (optional; falls back to `OPENAI_API_KEY`)

## Guarantees

- Planner input boundary: only trusted user message + constrained metadata shape.
- Quarantine output boundary: typed only (`bool | int | float | enum`).
- Enum output validated against provided compile-time registry map.

## Live smoke test

Env-gated OpenAI call path test:
- `SIEVE_RUN_OPENAI_LIVE=1`
- `OPENAI_API_KEY=...` (or scoped quarantine key)

Run:
- `cargo test -p sieve-llm openai_live_quarantine_smoke_env_gated -- --nocapture`

Live example:
- `OPENAI_API_KEY=... cargo run -p sieve-llm --example openai_live`

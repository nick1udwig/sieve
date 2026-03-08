# sieve-llm

OpenAI-backed planner + guidance + response/summary model adapters for Sieve v3 MVP.

## Config

Planner config env:
- `SIEVE_PLANNER_MODEL` (required)
- `SIEVE_PLANNER_PROVIDER` (optional; `openai` or `openai_codex`; default `openai`)
- `SIEVE_PLANNER_API_BASE` (optional)
- `SIEVE_PLANNER_OPENAI_API_KEY` (optional; falls back to `OPENAI_API_KEY`)

Guidance config env:
- `SIEVE_GUIDANCE_MODEL` (optional; falls back to `SIEVE_PLANNER_MODEL`)
- `SIEVE_GUIDANCE_PROVIDER` (optional; `openai` or `openai_codex`; default `openai`)
- `SIEVE_GUIDANCE_API_BASE` (optional)
- `SIEVE_GUIDANCE_OPENAI_API_KEY` (optional; falls back to `SIEVE_PLANNER_OPENAI_API_KEY` then `OPENAI_API_KEY`)

Response/quarantine config env:
- `SIEVE_RESPONSE_PROVIDER` / `SIEVE_QUARANTINE_PROVIDER` (optional; `openai` or `openai_codex`)
- `SIEVE_RESPONSE_API_BASE` / `SIEVE_QUARANTINE_API_BASE` (optional)
- `SIEVE_RESPONSE_OPENAI_API_KEY` / `SIEVE_QUARANTINE_OPENAI_API_KEY` (OpenAI provider only)

Codex subscription auth env:
- `OPENAI_CODEX_ACCESS_TOKEN` + `OPENAI_CODEX_ACCOUNT_ID` for explicit token/account overrides
- `SIEVE_OPENAI_CODEX_AUTH_JSON_PATH` to override the auth file path
- default auth file path when unset: `$SIEVE_HOME/state/auth.json` (or `~/.sieve/state/auth.json`)
- native login flow: `cargo run -p sieve-app -- auth login openai-codex`
- `openai_codex` is intended for personal/subscription use, not production API workloads

LLM exchange logging env:
- `SIEVE_LLM_EXCHANGE_LOG_PATH` (optional JSONL file path)
- default when unset: `$SIEVE_HOME/logs/llm-provider-exchanges.jsonl` (or `$HOME/.sieve/logs/...`)

## Guarantees

- Planner input boundary: only trusted user message + constrained metadata shape.
- Planner tool-call args validated against strict per-tool contracts (`bash|endorse|declassify`).
- On planner tool-arg contract failure, one regeneration pass is attempted with structured diagnostics.
- Q-LLM -> planner boundary: typed numeric guidance signals only (`PlannerGuidanceSignal` + `PlannerGuidanceFrame`).
- Guidance input may inspect bounded raw untrusted artifact excerpts plus typed step observations; planner receives only safe typed guidance + browser-session summaries.
- Response writing and compose gating consume typed untrusted evidence records derived from raw refs; raw artifact text stays out of trusted response/planner paths.
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

# sieve-v3

A Rust-based general-purpose agent that is resistant to prompt injection *by design*.

Currently pre-Alpha.
Use at your own risk.

Inspired by:
- [Simon Willison](https://simonwillison.net/search/?q=prompt-injection)
- [CaMeL](https://arxiv.org/abs/2503.18813)
- [CaMeLs CUA](https://arxiv.org/abs/2601.09923)
- [FIDES](https://arxiv.org/abs/2505.23643)

## Run Integrated App

1. Copy `.env.example` to `.env` and set:
   - `OPENAI_API_KEY`
   - `SIEVE_PLANNER_MODEL`
   - `TELEGRAM_BOT_TOKEN`
   - `TELEGRAM_CHAT_ID`
   - optional: `SIEVE_POLICY_PATH` (defaults to `docs/policy/baseline-policy.toml`)
   - optional: `SIEVE_HOME` (defaults to `~/.sieve`)
   - optional: `SIEVE_MAX_CONCURRENT_TURNS` (defaults to `4`)
2. Start the app:

```bash
cargo run -p sieve-app -- "review workspace status"
```

Modes:
- Single command mode: pass a CLI prompt (`cargo run -p sieve-app -- "review workspace status"`).
- Long-running agent mode: omit CLI prompt. The app stays up, accepts prompts from stdin and
  Telegram chat, and executes turns concurrently (bounded by `SIEVE_MAX_CONCURRENT_TURNS`).

Approval responses in Telegram:
- `yes` or `y` (approve once), `no` or `n` (deny) when replying to the approval message.
- React `👍` (approve once) or `👎` (deny) on the approval message.
- Existing explicit commands still work: `/approve_once <request_id>` and `/deny <request_id>`.

Runtime JSONL logs now include both runtime events and conversation records, defaulting to
`$SIEVE_HOME/logs/runtime-events.jsonl` (same base dir as trace logs).

Baseline policy file: `docs/policy/baseline-policy.toml`.

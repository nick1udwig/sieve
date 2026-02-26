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
   - `SIEVE_POLICY_PATH`
2. Start the app:

```bash
cargo run -p sieve-app -- "review workspace status"
```

If no CLI prompt is passed, `sieve-app` reads prompts from stdin (one line per turn).

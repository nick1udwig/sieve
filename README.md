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

`sieve-app` auto-loads `.env` from the current working directory when present.

One-off smoke test (uses `.env` automatically):

```bash
cargo run -p sieve-app -- "Use bash to run exactly: pwd"
```

Expected result includes:
- current working directory printed by `pwd`
- `run-1 -> [Bash { command: "pwd", disposition: ExecuteMainline(...) }]`

Approval smoke scripts:

```bash
./scripts/smoke-approval-expected.sh
./scripts/smoke-requires-approval-expected.sh
```

These are live Telegram flows (not unit tests): each script runs `sieve-app`, waits for a real
`approval_requested` event, and verifies live gate behavior.
- `smoke-approval-expected.sh`: waits for you to approve in Telegram, then verifies execution.
- `smoke-requires-approval-expected.sh`: verifies command stays blocked pending approval.

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

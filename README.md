# sieve

A general-purpose agent that is resistant to prompt injection by design, in Rust.

Currently pre-Alpha.
Use at your own risk.

Inspired by:
- [Simon Willison](https://simonwillison.net/search/?q=prompt-injection)
- [CaMeL](https://arxiv.org/abs/2503.18813)
- [CaMeLs CUA](https://arxiv.org/abs/2601.09923)
- [FIDES](https://arxiv.org/abs/2505.23643)

## Dependencies

You need a Rust toolchain plus a `.env` copied from `.env.example`.
The minimum setup is `OPENAI_API_KEY`; Telegram usage also needs `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID`.
Optional host tools include `trash` for safer file deletion, `st` for voice-note I/O, `codex` for image OCR, and `sieve-lcm-cli` for LCM memory flows.

Full dependency, env, runtime, logging, Telegram, troubleshooting, and external CLI repo links live in [docs/running.md](docs/running.md#running).

## Running


Send a one-off request with:

```bash
cargo run -p sieve-app -- "review workspace status"
```

Start long-running mode with:

```bash
cargo run --release -p sieve-app
```

## Architecture

Sieve keeps the planner isolated from raw untrusted tool output.
The main workspace split is:
- `sieve-app` for the integrated entrypoint,
- `sieve-runtime` for orchestration and approvals,
- `sieve-policy` for capability checks,
- `sieve-quarantine` for sandboxed tracing, and
- supporting crates for summaries, LLM wiring, Telegram, schemas, and shared types.

Architecture notes and crate map: [docs/running.md](docs/running.md#architecture).
Deeper design docs: [docs/sieve-v3-mvp-spec-v1.3.md](docs/sieve-v3-mvp-spec-v1.3.md) and [docs/sieve-v3-mvp-security.md](docs/sieve-v3-mvp-security.md).

## Testing

Run the local suite with:

```bash
cargo test --workspace
```

Run the deterministic app harness with:

```bash
cargo test -p sieve-app e2e_fake_ -- --nocapture
```

Commands and coverage notes: [docs/running.md](docs/running.md#testing).

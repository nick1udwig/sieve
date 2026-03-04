# sieve-captrace

Standalone capability-definition generator for one CLI command.

Flow:
- collect argument variants (seed cases + optional planner LLM)
- when only default `<command> --help` is available, recursively parse help text for subcommands and synthesize subcommand exercise variants (`--help`, usage-tail args, representative flags)
- run each variant through quarantine tracing (`bwrap + strace`)
- emit Sieve-compatible `summary_outcome.summary` definitions per variant

Runtime behavior:
- auto-load `.env` from current working directory when present
- prefer Codex app-server for case generation when reachable
- fall back to OpenAI planner when app-server is not reachable

## Usage

```bash
cargo run -p sieve-captrace -- mkdir --seed-case 'mkdir -p {{TMP_DIR}}/logs' --output /tmp/mkdir-definition.json --rust-output /tmp/mkdir-generated.rs
```

Escalation flags:
- `--allow-local-network` (loopback-only intent)
- `--allow-full-network` (share host outbound network)
- `--allow-write <absolute path>` (repeatable writable bind mount)

### Placeholders

- `{{TMP_DIR}}`
- `{{IN_FILE}}`
- `{{IN_FILE_2}}`
- `{{OUT_FILE}}`
- `{{URL}}`
- `{{HEADER}}`
- `{{DATA}}`
- `{{KV}}`
- `{{ARG}}`

## LLM Mode

Enabled by default (disable with `--no-llm`).

Required env:
- `SIEVE_PLANNER_MODEL`
- `OPENAI_API_KEY` (or `SIEVE_PLANNER_OPENAI_API_KEY`)

Optional:
- `SIEVE_PLANNER_API_BASE`
- `SIEVE_CODEX_APP_SERVER_WS_URL` (default: `ws://127.0.0.1:4500`)
- `SIEVE_CODEX_MODEL` (default: `gpt-5.2-codex`)
- `SIEVE_CODEX_APP_SERVER_CONNECT_TIMEOUT_MS` (default: `500`)
- `SIEVE_CODEX_APP_SERVER_TURN_TIMEOUT_MS` (default: `30000`)

Output includes:
- JSON artifact (`GeneratedCommandDefinition`) with per-variant summaries
- per-subcommand aggregate report (`subcommand_reports`) with success/failure counts + capability union
- Rust snippet string (`rust_snippet`) suitable for adapting into `sieve-command-summaries`
- escalation guidance in `notes` when traces indicate blocked capabilities (also emitted on stderr as `next:`/`hint:` lines)
- normalized network capability scopes (`network=local|network=remote`) to avoid host-specific IP hardcoding

# Running Sieve

## Running

### Dependencies

1. Copy `.env.example` to `.env`.
2. Set the minimum env needed for your mode:
   - always: either `OPENAI_API_KEY` for `openai` provider, or `SIEVE_*_PROVIDER=openai_codex` plus `cargo run -p sieve-app -- auth login openai-codex` (or `OPENAI_CODEX_ACCESS_TOKEN` + `OPENAI_CODEX_ACCOUNT_ID`)
   - if `openai` is configured but no OpenAI API key is present, Sieve now auto-falls back to `openai_codex` when valid Codex auth is available
   - usually: `SIEVE_PLANNER_MODEL`
   - Telegram ingress: `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`
3. Add optional env as needed:
   - `SIEVE_TELEGRAM_ALLOWED_SENDER_USER_IDS` for Telegram sender allowlisting
   - `SIEVE_POLICY_PATH` to override the default policy file (`docs/policy/baseline-policy.toml`)
   - `SIEVE_HOME` to override the default state root (`~/.sieve`)
   - `SIEVE_HEARTBEAT_EVERY` to enable periodic main-session heartbeat wakeups (for example `15m`, `1h`, `1d`)
   - `SIEVE_HEARTBEAT_PROMPT` to override heartbeat instructions inline instead of reading `HEARTBEAT.md`
   - `SIEVE_MAX_CONCURRENT_TURNS` to cap long-running mode concurrency (default `4`)
   - `SIEVE_MAX_PLANNER_STEPS` to cap planner act/observe loops (default `3`)
   - `SIEVE_MAX_SUMMARY_CALLS_PER_TURN` to cap compose/evidence/gate summary calls per turn (default `12`)
   - `SIEVE_LCM_ENABLED`, `SIEVE_LCM_GLOBAL_SESSION_ID`, `SIEVE_LCM_TRUSTED_DB_PATH`, `SIEVE_LCM_UNTRUSTED_DB_PATH`, `SIEVE_LCM_CLI_BIN` for LCM memory
   - `SIEVE_LLM_EXCHANGE_LOG_PATH` for exact OpenAI request/response JSONL logging
   - `SIEVE_RESPONSE_MODEL`, `SIEVE_GUIDANCE_MODEL`, `SIEVE_QUARANTINE_MODEL` to split planner/response/guidance/quarantine models
   - `BRAVE_API_KEY` and `SIEVE_BRAVE_API_BASE` for bash/CLI-based Brave search commands
4. Install the repo-specific CLIs Sieve assumes when you use those paths:
   - `trash` for safer file deletion via the desktop trash
     - repo: [`andreafrancia/trash-cli`](https://github.com/andreafrancia/trash-cli)
     - used for: `trash FILE...`
     - optional: yes
     - Linux install: `sudo apt install trash-cli` (Debian/Ubuntu), `sudo pacman -S trash-cli` (Arch), `sudo dnf install trash-cli` (Fedora), or `pipx install trash-cli`
   - `bravesearch` for agent-friendly Brave web search flows
     - repo: [`nick1udwig/brave-search`](https://github.com/nick1udwig/brave-search)
     - used for: planner-discovery/search commands run through `bash`
     - optional: yes
   - `st` for Telegram voice-note STT/TTS
     - repo: [`nick1udwig/st`](https://github.com/nick1udwig/st)
     - used for: `st stt`, `st tts`
     - optional: yes
   - `codex` for Codex app-server OCR and delegated coding sessions
     - repo: [`openai/codex`](https://github.com/openai/codex)
     - used for: `codex app-server` behind `codex_exec` and `codex_session`
     - optional: yes
   - `sieve-lcm-cli` for LCM memory query/expand/ingest flows
     - repo: [`nick1udwig/sieve-lcm`](https://github.com/nick1udwig/sieve-lcm)
     - used for: `sieve-lcm-cli query`, `expand`, `ingest`
     - optional: yes, unless `SIEVE_LCM_ENABLED=1`
5. Install system packages as needed:
   - `ffmpeg` recommended for audio conversion and delivery paths

`.env.example` carries the fuller provider-level matrix (`SIEVE_*_PROVIDER`, `*_API_BASE`, scoped API keys, Codex auth-file overrides, runtime defaults, and Telegram polling config).
`sieve-app` auto-loads `.env` from the current working directory when present.
Codex auth defaults to `$SIEVE_HOME/state/auth.json`.
Codex session metadata persists in `$SIEVE_HOME/state/codex.db`.
`cargo run -p sieve-app -- auth path` prints the resolved auth file path.

### Start The Integrated App

Run one prompt end-to-end:

```bash
cargo run -p sieve-app -- run --prompt "review workspace status"
```

Start long-running mode (stdin + Telegram ingress, no initial prompt):

```bash
cargo run -p sieve-app -- run
```

One-off smoke:

```bash
cargo run -p sieve-app -- run --prompt "Use bash to run exactly: pwd"
```

Expected result:
- current working directory printed by `pwd`
- `run-1: ...` assistant reply from the response-writer phase

### Docker

The repo ships a multi-stage Debian image in [`Dockerfile`](../Dockerfile).
The runtime image is `debian:bookworm-slim` and includes `sieve-app`, `bubblewrap`, `strace`, `ffmpeg`, `trash`, `bravesearch`, `st`, `codex`, and `sieve-lcm-cli`.
The Nick CLI tools are downloaded from their latest GitHub releases at build time instead of being built from source.
Container defaults:
- workdir: `/workspace`
- `SIEVE_RUNTIME_CWD=/workspace`
- `SIEVE_HOME=/data/.sieve`
- `SIEVE_POLICY_PATH=/opt/sieve/docs/policy/baseline-policy.toml`

Build locally:

```bash
docker build -t sieve:local .
```

Run against the current checkout:

```bash
docker run --rm -it --security-opt seccomp=unconfined --env-file .env -v "$PWD:/workspace" -v sieve-data:/data sieve:local run --prompt "review workspace status"
```

Run long-lived mode:

```bash
docker run --rm -it --security-opt seccomp=unconfined --env-file .env -v "$PWD:/workspace" -v sieve-data:/data sieve:local
```

If `bubblewrap` quarantine fails under Docker, allow unprivileged user namespaces on the host or add the extra container privileges your runtime requires.
If a release asset name ever changes, override the matcher with `--build-arg BRAVE_SEARCH_ASSET_REGEX=...`, `ST_ASSET_REGEX=...`, or `SIEVE_LCM_ASSET_REGEX=...`.
`CODEX_NPM_SPEC` still controls the installed Codex npm package spec.

### Release Automation

`.github/workflows/release.yml` is currently manual-only via `workflow_dispatch`.
The old push-to-`master` trigger is commented out because the workflow is too slow for every merge right now.
For non-release commits it bumps the shared workspace patch version, regenerates both lockfiles, commits the bump back to `master`, and then builds and pushes `nick1udwig/sieve:<version>` plus `nick1udwig/sieve:latest` for `linux/amd64` and `linux/arm64`.
The workflow expects Docker Hub secrets `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN`.

### Modes

- Single command mode: use `run --prompt`, for example `cargo run -p sieve-app -- run --prompt "review workspace status"`.
- Long-running agent mode: use `run` with no prompt. The app stays up, accepts prompts from stdin and Telegram chat, and executes turns concurrently up to `SIEVE_MAX_CONCURRENT_TURNS`.
- Heartbeat and cron automation run only in long-running mode.

### Heartbeat And Cron

- Heartbeat wakes the durable `main` session on an interval or via `/heartbeat now`.
- Heartbeat instructions come from `HEARTBEAT.md` under `SIEVE_RUNTIME_CWD`, unless `SIEVE_HEARTBEAT_PROMPT` overrides them.
- If heartbeat decides nothing needs user-facing output, it replies internally with `HEARTBEAT_OK` and Sieve stays silent.
- Durable automation state lives at `$SIEVE_HOME/state/automation.json`.
- `main` cron queues a trusted system event into the durable main session, then heartbeat decides what to surface.
- `isolated` cron runs a separate synthetic turn under logical session key `cron:<job_id>` and does not share main-session conversation state.
- One-shot `at` jobs disable themselves after firing.
- One-shot relative `after` jobs resolve to a single future run time, then disable themselves after firing.
- Repeating `every` and `cron` jobs reschedule automatically.

Long-running mode command surface:

- `/heartbeat now`
- `/cron list`
- `/cron add main after 1m -- remind me to say hi`
- `/cron add main every 15m -- remind me to check deploys`
- `/cron add main at 2026-03-06T09:00:00Z -- remind me about standup`
- `/cron add isolated cron 0 9 * * 1-5 -- send build summary`
- `/cron pause cron-1`
- `/cron resume cron-1`
- `/cron rm cron-1`

### Telegram Approval Flow

- Reply `yes` or `y` to approve once, `no` or `n` to deny.
- React with a thumbs-up to approve once or a thumbs-down to deny on the approval message.
- Explicit commands still work: `/approve_once <request_id>` and `/deny <request_id>`.
- While a turn is processing, the bot emits Telegram `typing` and stops automatically on success, failure, or cancellation.
- `SIEVE_TELEGRAM_ALLOWED_SENDER_USER_IDS` restricts prompts and approvals to listed Telegram user IDs.
- Reaction approvals require Telegram `message_reaction` updates; Telegram also requires the bot to be an admin in group chats.

### Runtime Logs, Artifacts, And Policy

- Runtime JSONL logs are one canonical event stream at `$SIEVE_HOME/logs/runtime-events.jsonl`.
- Canonical events include runtime events, conversation turns, planner/controller decisions, and compose audit records.
- Canonical event records include `session_id`, unique `turn_id`, and per-session `turn_seq`.
- Canonical event records also include logical `turn_kind` and `logical_session_key` metadata for user, heartbeat, and cron turns.
- LLM provider wire logs default to `$SIEVE_HOME/logs/llm-provider-exchanges.jsonl` and contain exact request payloads plus raw response bodies per attempt.
- Planner turns run in an act/observe loop bounded by `SIEVE_MAX_PLANNER_STEPS`, with typed Q-LLM guidance deciding whether to continue tool actions or finalize.
- Planner never sees raw untrusted stdout/stderr strings.
- Compose-stage quarantine calls are capped per turn by `SIEVE_MAX_SUMMARY_CALLS_PER_TURN`.
- Mainline `bash` execution stores stdout/stderr as untrusted artifacts under `$SIEVE_HOME/artifacts/<turn_id>/`.
- Quarantine runs store traces under `$SIEVE_HOME/logs/traces/<turn_id>/`.
- Baseline policy file: `docs/policy/baseline-policy.toml`.

### LCM Memory Integration

LCM memory is dual-lane and tool-driven:

- ingestion is always on
- trusted lane receives trusted user prompts
- untrusted lane receives user and assistant turns
- planner retrieval is explicit via `bash` plus `sieve-lcm-cli query`
- `query --lane both` returns trusted excerpts plus opaque untrusted refs
- untrusted refs can be expanded later via `sieve-lcm-cli expand` in quarantined/Q-LLM flows
- global memory mode maps all turns to one configured session key via `SIEVE_LCM_GLOBAL_SESSION_ID`

### Search And Media Notes

Web search:

- `sieve-app` does not use a dedicated `brave_search` tool
- use normal `bash` commands, for example a local Brave CLI wrapper or `curl`
- keep policy capabilities aligned with the endpoint your CLI command talks to

Telegram voice notes:

- voice input is converted to text with `st stt <audio-file>`
- audio replies are synthesized with `st tts <text-file> --format opus --output <audio-file>`
- ensure `st providers` includes `openai`
- when audio synthesis or delivery fails, the app falls back to a text reply

Telegram images:

- photo/image input is converted to text through the Codex app-server `codex_exec` path with a read-only, no-network sandbox and a local image input
- this ingress OCR path is treated as trusted user-input provenance
- OCR done later inside planner `bash` tool flows remains untrusted tool output by default
- when OCR extraction fails, the app replies with a text error message

Modality contract:

- ingress modality is explicit (`text`, `audio`, `image`)
- response defaults to the same modality as ingress
- audio reply mode nudges the response writer toward speech-friendly phrasing before TTS
- explicit overrides currently supported:
  - `image -> text` (`not_supported`, no image-generation path yet)
  - `audio -> text` (`tool_failure`, when TTS synthesis or delivery fails)

### Telegram 409 Troubleshooting

If logs show `telegram poll failed ... error: 409`, another process is already calling `getUpdates` for the same bot token.

- stop other bot pollers (`sieve-app`, manual smoke runs, other bot clients)
- rerun after the competing poller exits
- quick check: `ps -ef | rg 'sieve-app|manual-smoke|getUpdates|telegram'`

## Architecture

Sieve is built around a strict trust boundary: user intent and local config can be trusted, but tool output is untrusted by default. Planner logic never receives raw untrusted stdout/stderr; quarantine and typed guidance sit between execution output and the planner loop.

### Core Workspace Pieces

- `crates/sieve-app`: integrated entrypoint, turn loop, response composition, ingress/media handling, logging, LCM integration
- `crates/sieve-runtime`: planner loop orchestration, approval flow, mainline execution, policy evaluation hooks, event logging
- `crates/sieve-policy`: capability and sink checks against the active TOML policy
- `crates/sieve-quarantine`: `bwrap + strace` quarantine runner and trace/report collection
- `crates/sieve-command-summaries`: command matching, capability inference, built-in summaries for filesystem, `curl`, `st`, Codex, and LCM flows
- `crates/sieve-llm`: provider integration plus typed planner/guidance/response wire contracts
- `crates/sieve-interface-telegram`: Telegram ingress/egress, approval UX, typing, media delivery
- `crates/sieve-shell`: shell parsing helpers
- `crates/sieve-tool-contracts`: planner tool-call schemas and validation
- `crates/sieve-types`: shared runtime, capability, event, modality, and tool types

### Capability Trace Definitions

`sieve-captrace` is the standalone helper crate for generating Sieve-compatible capability definitions:

- generates argument variants for one command (seeded plus optional LLM expansion)
- runs each variant in quarantine (`bwrap + strace`)
- emits a definition payload with `summary_outcome.summary.required_capabilities`
- auto-loads `.env` when present
- prefers the Codex app-server when reachable, then falls back to OpenAI planner

Run:

```bash
cargo run -p sieve-captrace -- mkdir --seed-case 'mkdir -p {{TMP_DIR}}/logs' --output /tmp/mkdir-definition.json --rust-output /tmp/mkdir-generated.rs
```

Optional:

- omit `--no-llm` to let the planner LLM propose additional cases
- set `SIEVE_PLANNER_MODEL` and either `OPENAI_API_KEY` (or `SIEVE_PLANNER_OPENAI_API_KEY`) for `openai`, or `SIEVE_PLANNER_PROVIDER=openai_codex` plus Codex auth via `cargo run -p sieve-app -- auth login openai-codex` for subscription mode
- app-server preference settings:
  - `SIEVE_CODEX_APP_SERVER_WS_URL` (default `ws://127.0.0.1:4500`)
  - `SIEVE_CODEX_MODEL` (default `gpt-5.2-codex`)
- generated JSON includes a `rust_snippet` field; `--rust-output` writes that snippet directly

### Design Docs

- MVP spec: [sieve-v3-mvp-spec-v1.3.md](sieve-v3-mvp-spec-v1.3.md)
- security design: [sieve-v3-mvp-security.md](sieve-v3-mvp-security.md)
- early architecture notes: [260225-sieve-v3.md](260225-sieve-v3.md)
- baseline policy: [policy/baseline-policy.toml](policy/baseline-policy.toml)

## Testing

### Core Test Suites

Before running tests, stop any long-running `sieve-app` process so Telegram polling and runtime state do not interfere.

Check for a running app:

```bash
ps -ef | rg 'sieve-app|target/release/sieve-app|cargo run -p sieve-app'
```

Run the main local suites with these exact commands:

```bash
cargo test -p sieve-app
cargo test -p sieve-runtime
cargo test -p sieve-policy
cargo test -p sieve-quarantine
cargo test -p sieve-command-summaries
cargo test --workspace
```

### Live OpenAI Runtime Tests

Runtime has env-gated live OpenAI planner integration tests that exercise full planner to runtime tool flows (`bash`, `endorse`, `declassify`), including approval handling.

```bash
SIEVE_RUN_OPENAI_LIVE=1 OPENAI_API_KEY=... cargo test -p sieve-runtime --test e2e_live_llm_openai -- --nocapture
```

Optional env overrides:

- `SIEVE_PLANNER_MODEL` (default `gpt-4o-mini`)
- `SIEVE_PLANNER_PROVIDER` (`openai` or `openai_codex`)
- `SIEVE_PLANNER_API_BASE`
- `SIEVE_PLANNER_OPENAI_API_KEY` (takes precedence over `OPENAI_API_KEY`)
- `OPENAI_CODEX_ACCESS_TOKEN` + `OPENAI_CODEX_ACCOUNT_ID`, or `SIEVE_OPENAI_CODEX_AUTH_JSON_PATH`

### App E2E Harness Tests

`sieve-app` includes an app-level E2E harness that can run with fake models for deterministic regression coverage or with real planner/response models for env-gated live smoke checks.

Deterministic regression coverage:

```bash
cargo test -p sieve-app e2e_fake_ -- --nocapture
```

Live app smoke suite:

```bash
SIEVE_RUN_OPENAI_LIVE=1 cargo test -p sieve-app live_e2e_ -- --nocapture
```

Live assertions cover more than "it replied":

- greeting/chat path stays tool-free
- web-search smoke emits the expected connect precheck capability
- policy allow path passes without approval
- successful smoke turns do not emit assistant-side error conversation records

When running search-heavy live scenarios via bash CLI, set `BRAVE_API_KEY` as needed.

### Live Telegram Approval Smokes

These are live Telegram flows, not unit tests:

```bash
./scripts/smoke-approval-expected.sh
./scripts/smoke-requires-approval-expected.sh
```

- `smoke-approval-expected.sh` waits for you to approve in Telegram, then verifies execution
- `smoke-requires-approval-expected.sh` verifies the command stays blocked pending approval

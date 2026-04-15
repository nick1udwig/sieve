# Codex App Server

Read when: touching Codex tool wiring, OCR ingress, approval passthrough, or Codex session persistence.

## Implemented

- `codex_exec` now maps to app-server `command/exec`.
- `codex_exec` takes argv `command`, sandbox mode, optional `cwd`, optional `writable_roots`, and optional `timeout_ms`.
- `codex_session` maps to app-server `thread/start` or `thread/resume` plus `turn/start`.
- `codex_session` persists session metadata in `$SIEVE_HOME/state/codex.db`.
- Saved session metadata includes `session_id`, `thread_id`, session name, cwd, sandbox, task summary, last result summary, status, and timestamps.
- Session names come from a short hyphenated instruction summary with chemist-name fallback when needed.
- Codex sandboxes are forced to `networkAccess: false` in both `readOnly` and `workspaceWrite` modes.
- Sieve planner receives trusted `CODEX_SESSIONS` summaries so it can choose resume vs start-new.
- Telegram image OCR now uses a transient Codex app-server turn with local image input instead of shelling out to `codex exec`.
- Command and file-change approvals from Codex sessions are passed through Sieve approval flow and Telegram UI with the session name prefixed.
- File-change approval prompts include best-effort diff preview text.
- Most Codex events are logged but not surfaced directly to the user.
- Telegram now keeps one editable status card per persistent Codex session and threads session-related assistant replies beneath that card.
- Codex approval prompts stay separate Telegram messages, but they reply to the session status card when one exists.
- Simple natural-language status questions about a saved Codex session are answered directly from trusted saved session metadata instead of going back through planner tool selection.
- Old `codex exec` command-summary/catalog handling was removed from `sieve-command-summaries`.
- `sieve-app` can now talk to a shared websocket app-server via `SIEVE_CODEX_APP_SERVER_WS_URL` instead of always spawning a fresh stdio child.
- The Docker image defaults that websocket URL to `ws://127.0.0.1:4500` and runs the shared server in tmux session `codex`.

## Tool Semantics

- Use `codex_exec` for one-off shell command execution inside Codex sandboxing.
- Use `codex_session` for Codex agent work such as coding, file edits, reviews, and multi-step repo tasks.
- Omit `session_id` on `codex_session` to start a fresh Codex thread.
- Provide `session_id` on `codex_session` to resume saved Codex work.
- Do not route Codex agent tasks through `bash`.
- Do not assume `codex_exec` can replace Codex agent turns.

## Current Limits

- No `turn/steer` support yet for in-flight Codex turns.
- When `SIEVE_CODEX_APP_SERVER_WS_URL` is unset, Sieve still spawns a fresh stdio app-server process per Codex request and relies on saved `thread_id` for continuity.
- Resume policy is still mostly prompt-driven from stored metadata.
- Open-loop working state currently only materializes automatically from zero-tool `continue_need_preference_or_constraint` turns.
- No Codex web search or network-enabled mode yet.
- No live env-gated e2e against a real Codex app-server binary yet.

## Likely Next Steps

- Add `turn/steer` and interrupt wiring for long-running Codex tasks.
- Add richer session-policy metadata so Sieve can decide resume vs fork vs start-new more reliably.
- Consider pooling multiple shared app-server connections if one websocket listener becomes a bottleneck.
- Add live integration tests behind env gates once a stable Codex test setup exists.

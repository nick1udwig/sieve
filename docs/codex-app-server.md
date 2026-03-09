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
- Old `codex exec` command-summary/catalog handling was removed from `sieve-command-summaries`.

## Tool Semantics

- Use `codex_exec` for one-off shell command execution inside Codex sandboxing.
- Use `codex_session` for Codex agent work such as coding, file edits, reviews, and multi-step repo tasks.
- Omit `session_id` on `codex_session` to start a fresh Codex thread.
- Provide `session_id` on `codex_session` to resume saved Codex work.
- Do not route Codex agent tasks through `bash`.
- Do not assume `codex_exec` can replace Codex agent turns.

## Current Limits

- No `turn/steer` support yet for in-flight Codex turns.
- No shared long-lived app-server child yet.
- Sieve spawns a fresh stdio app-server process per Codex request and relies on saved `thread_id` for continuity.
- Resume policy is still mostly prompt-driven from stored metadata.
- No Codex web search or network-enabled mode yet.
- No live env-gated e2e against a real Codex app-server binary yet.

## Likely Next Steps

- Add `turn/steer` and interrupt wiring for long-running Codex tasks.
- Add richer session-policy metadata so Sieve can decide resume vs fork vs start-new more reliably.
- Consider a shared app-server connection pool if process startup becomes a bottleneck.
- Add live integration tests behind env gates once a stable Codex test setup exists.

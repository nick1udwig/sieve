# Worker 8: `sieve-interface-telegram` Implementation Notes

You are reading implementation notes for Worker 8 (`crates/sieve-interface-telegram`).

## Implemented

- Telegram adapter implemented as `TelegramAdapter`.
  - Wraps `TelegramEventBridge`.
  - Maintains pending approval map by `request_id`.
  - Publishes runtime events to bridge.
  - Handles long-poll updates and command parsing.
  - Emits `ApprovalResolvedEvent` with `approve_once|deny`.
- Runtime event chat formatting implemented.
  - `approval_requested` includes:
    - full argv (composed segments)
    - inferred capabilities
    - blocked rule id
    - reason
  - `policy_evaluated` formatted to chat.
  - `quarantine_completed` formatted to chat.
- Telegram Bot API long-poll transport implemented as `TelegramBotApiLongPoll`.
  - `getUpdates` + `sendMessage`.
  - concrete transport via `curl` command execution.
  - typed Telegram response decoding and error mapping.
- Deterministic timing support via `Clock` trait + `SystemClock`.
- Manual smoke harness added:
  - `crates/sieve-interface-telegram/examples/manual-smoke.rs`
- Crate runbook added:
  - `crates/sieve-interface-telegram/README.md`

## Tests Added

- Message-to-approval mapping:
  - `/approve_once` -> `ApprovalAction::ApproveOnce`
  - `/deny` -> `ApprovalAction::Deny`
  - `approve` alias -> `ApprovalAction::ApproveOnce`
- Chat filtering:
  - ignores wrong chat id
  - unknown `request_id` reports error message to chat
- Runtime event formatting checks:
  - approval details
  - policy decision text
  - quarantine trace text
- Transport tests:
  - `getUpdates` URL + decode mapping
  - `sendMessage` POST payload shape
  - Telegram API error mapping

## Surprises / Gotchas

- `reqwest` path blocked in this environment:
  - transitive mismatch (`futures-sink = ^0.3.32` unavailable).
  - switched transport to `curl` executor to keep crate stable.
- Manual smoke initially blocked:
  - missing `TELEGRAM_BOT_TOKEN` / `TELEGRAM_CHAT_ID`.
- Network intermittently unavailable for crates index.
  - package tests run successfully with `--offline`.

## Remaining TODO (Worker 8 Scope)

- Run real Telegram manual smoke with valid env:
  - `TELEGRAM_BOT_TOKEN`
  - `TELEGRAM_CHAT_ID`
- Optional hardening:
  - add retries/backoff strategy in long-poll loop
  - add command help text for malformed approval commands
  - evaluate async transport path (remove shell `curl` dependency)

## Done Criteria Status

- `cargo test -p sieve-interface-telegram --offline`: passing.
- Manual approve/deny run against real Telegram chat: pending env secrets.

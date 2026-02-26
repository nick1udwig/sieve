# Worker 8: `sieve-interface-telegram`

You are Worker 8 for Sieve v3. Own only crate `crates/sieve-interface-telegram`.

Do not begin until core enforcement gate is declared green.

## Read First

- `/root/git/sieve-v3/docs/implementation-workers-v0.md`
- `/root/git/sieve-v3/crates/sieve-interface-telegram/src/lib.rs`
- `/root/git/sieve-v3/crates/sieve-types/src/lib.rs`
- `/root/git/sieve-v3/schemas/approval-requested-event.schema.json`
- `/root/git/sieve-v3/schemas/approval-resolved-event.schema.json`

## Mission

Build Telegram long-poll adapter that consumes runtime events and submits approval responses.

## Scope

- Implement adapter around `TelegramEventBridge`.
- Render consolidated approval request details:
  - Full argv.
  - Inferred capabilities.
  - Blocked rule ID.
  - Reason.
- Emit `approve_once` or `deny` responses mapped to `ApprovalResolvedEvent`.
- Keep payload contract unchanged from shared schemas.

## Required Outputs

- Long-polling loop and command handlers.
- Basic formatting for policy and quarantine events to chat.
- Tests for message-to-approval mapping.

## Out Of Scope

- Core policy logic.
- Changing shared event payload schema.

## Definition Of Done

- `cargo test -p sieve-interface-telegram` passes.
- Manual run can approve or deny a sample pending request.

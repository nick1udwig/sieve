# Telegram Full-Flow Harness Plan

Status: Implemented in `crates/sieve-app/src/main.rs` test harness (`AppE2eHarness::run_telegram_text_turn`) with deterministic and env-gated live coverage.

## Goal
Build a test harness that exercises the real `sieve-app` Telegram turn loop end-to-end without a human in the loop, including ingress via long-poll updates and egress via Telegram reply APIs.

## Why
Current live checks prove model/runtime basics, but still depend on manual Telegram interaction and do not fully validate the bot adapter path (`getUpdates` -> prompt enqueue -> turn execution -> `sendMessage`/typing events).

## Scope
- Mock Telegram long-polling transport.
- Inject synthetic incoming Telegram updates as if sent by user.
- Run full app turn flow against those updates.
- Capture and assert outbound bot actions/messages.
- Add env-gated live model scenarios for realistic QA loops.

## Out of Scope
- Real Telegram network integration (already covered by manual smoke).
- Audio/image full-flow in first phase (text-only first).

## Test Architecture
1. Add a deterministic mock Telegram API implementation in `sieve-interface-telegram`:
   - `getUpdates` returns queued synthetic updates.
   - `sendMessage`, `sendChatAction`, `sendVoice` calls are recorded.
2. Add a harness driver that wires:
   - mock Telegram API
   - real `TelegramAdapter` + `RuntimeBridge`
   - in-process channels already used by `sieve-app`
3. Provide helper APIs:
   - `push_user_text(chat_id, user_id, text)`
   - `drain_outbound_messages()`
   - `drain_outbound_chat_actions()`
4. Assert both:
   - runtime/event-log behavior
   - user-visible Telegram outbound content

## Test Matrix
### Deterministic (fake models)
- Greeting:
  - Input: `"Hi"`
  - Expect: one assistant reply, no policy/tool events, friendly direct tone.
- Weather request skeleton:
  - Input: `"weather in dublin ireland today"`
  - Expect: at least one tool/policy path used (via allowed toolset), one assistant reply with source URL.

### Env-gated live (real OpenAI)
- Case 1: `"Hi"`
  - Assert no tool action needed and response is direct conversational reply.
- Case 2: `"weather in dublin ireland today"`
  - Assert response includes a direct weather statement (not only link dump) and at least one plain URL.
- Case 3: `"weather in dublin ireland tomorrow"`
  - Assert response includes tomorrow-focused weather statement and at least one plain URL.

## Live Validation Strategy
To avoid brittle exact-temperature assertions:
- Validate structure/quality instead of exact numbers:
  - answers the asked timeframe (`today` vs `tomorrow`)
  - includes at least one concrete weather datum (temp/condition/precip/wind)
  - includes at least one plain source URL
  - avoids meta/third-person narration
- Add a Q-LLM grading pass for these criteria (PASS/REVISE) to reduce manual review.

## Implementation Phases
1. Telegram mock transport + outbound capture in `sieve-interface-telegram` tests.
2. App-level harness entrypoint using mocked Telegram transport.
3. Deterministic regression cases.
4. Env-gated live cases (`greeting`, `Dublin today`, `Dublin tomorrow`).
5. CI knobs/docs for running deterministic vs live harness suites.

## Acceptance Criteria
- A single test command can run full Telegram text flow locally without human action.
- Live env-gated suite can be run repeatedly to iterate on answer quality.
- Regressions like greeting misbehavior and non-answer weather responses are caught automatically.

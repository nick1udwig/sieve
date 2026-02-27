# sieve-interface-telegram

Telegram adapter for Sieve runtime events and approval resolution.

## Manual Smoke

Requirements:
- `curl` on `PATH`
- `TELEGRAM_BOT_TOKEN` env var
- `TELEGRAM_CHAT_ID` env var (numeric chat id)

### How to get `TELEGRAM_CHAT_ID`

1. Send a message to your bot in Telegram (or mention it in the target group).
2. Run:

```bash
curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getUpdates"
```

3. Read `result[].message.chat.id` from the JSON response.
   - Personal chat IDs are usually positive.
   - Group/supergroup chat IDs are usually negative.

Run:

```bash
cargo run -p sieve-interface-telegram --example manual-smoke
```

Behavior:
- Sends one sample `approval_requested` message to configured chat.
- Waits on long-poll updates.
- Map commands to approval result:
  - `/approve_once apr_manual_smoke` (or `/approve`)
  - `/deny apr_manual_smoke`
  - `yes`/`y` approve and `no`/`n` deny when replying to the approval message.
  - `👍` approve and `👎` deny when reacting to the approval message.
  - Reaction handling depends on Telegram `message_reaction` updates; this adapter requests them
    via `allowed_updates`, and Telegram requires bot admin permissions in group chats.

Config knobs:
- `TelegramAdapterConfig.chat_id`
- `TelegramAdapterConfig.poll_timeout_secs`

## Troubleshooting

`telegram ... 409` errors mean another process is consuming `getUpdates` for the same bot token.

Fix:
- stop other long-poll clients using that token
- rerun this adapter/app

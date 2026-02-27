// For full runtime + planner + Telegram wiring, run `cargo run -p sieve-app -- "<prompt>"`.
use sieve_interface_telegram::{
    SystemClock, TelegramAdapter, TelegramAdapterConfig, TelegramBotApiLongPoll,
    TelegramEventBridge,
};
use sieve_types::{
    Action, ApprovalRequestId, ApprovalRequestedEvent, ApprovalResolvedEvent, Capability,
    CommandSegment, Resource, RunId, RuntimeEvent,
};
use std::env;

struct LoggingBridge;

impl TelegramEventBridge for LoggingBridge {
    fn publish_runtime_event(&self, event: &RuntimeEvent) {
        println!("published runtime event: {event:?}");
    }

    fn submit_approval(&self, approval: ApprovalResolvedEvent) {
        println!("approval resolved: {approval:?}");
    }
}

fn main() {
    let bot_token = env::var("TELEGRAM_BOT_TOKEN")
        .expect("missing TELEGRAM_BOT_TOKEN (set Telegram bot token)");
    let chat_id = env::var("TELEGRAM_CHAT_ID")
        .expect("missing TELEGRAM_CHAT_ID (set target chat id)")
        .parse::<i64>()
        .expect("invalid TELEGRAM_CHAT_ID (must parse as i64)");

    let bridge = LoggingBridge;
    let poll = TelegramBotApiLongPoll::new(bot_token);
    let clock = SystemClock;
    let mut adapter = TelegramAdapter::new(
        TelegramAdapterConfig {
            chat_id,
            poll_timeout_secs: 20,
        },
        bridge,
        poll,
        clock,
    );

    let requested = ApprovalRequestedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("apr_manual_smoke".to_string()),
        run_id: RunId("run_manual_smoke".to_string()),
        command_segments: vec![CommandSegment {
            argv: vec!["rm".to_string(), "-rf".to_string(), "/tmp/demo".to_string()],
            operator_before: None,
        }],
        inferred_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "/tmp/demo".to_string(),
        }],
        blocked_rule_id: "deny-rm-rf".to_string(),
        reason: "manual-smoke".to_string(),
        created_at_ms: 0,
    };

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(requested))
        .expect("failed to publish approval request");

    println!("sent sample approval request to Telegram");
    println!(
        "approve: reply yes/y, react 👍, or /approve_once apr_manual_smoke; deny: reply no/n, react 👎, or /deny apr_manual_smoke"
    );

    loop {
        adapter.poll_once().expect("telegram poll failed");
    }
}

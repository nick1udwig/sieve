use super::support::{
    sample_approval_requested, test_config, FixedClock, StepClock, TestBridge, TestPoller,
};
use super::*;

#[test]
fn approval_message_suppresses_policy_and_quarantine_chatter() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(Vec::new());
    let clock = FixedClock { now: 0 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("approval requested event");
    adapter
        .publish_runtime_event(RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
            schema_version: 1,
            run_id: RunId("run_1".into()),
            decision: PolicyDecision {
                kind: PolicyDecisionKind::DenyWithApproval,
                reason: "blocked by policy".into(),
                blocked_rule_id: Some("deny-rm-rf".into()),
            },
            inferred_capabilities: Vec::new(),
            trace_path: None,
            created_at_ms: 111,
        }))
        .expect("policy evaluated event");
    adapter
        .publish_runtime_event(RuntimeEvent::QuarantineCompleted(
            QuarantineCompletedEvent {
                schema_version: 1,
                run_id: RunId("run_1".into()),
                report: QuarantineReport {
                    run_id: RunId("run_1".into()),
                    trace_path: "/tmp/trace".into(),
                    stdout_path: None,
                    stderr_path: None,
                    attempted_capabilities: Vec::new(),
                    exit_code: Some(1),
                },
                created_at_ms: 112,
            },
        ))
        .expect("quarantine completed event");

    let sent_messages = &adapter.poll.sent_messages;
    assert_eq!(sent_messages.len(), 1);
    assert!(sent_messages[0].1.contains("approval needed to run:"));
    assert!(sent_messages[0].1.contains("`rm -rf /tmp/scratch`"));
    assert!(sent_messages[0].1.contains("\nmutating command"));
    assert!(sent_messages[0].1.contains("reply yes/y or react"));
}

#[test]
fn assistant_message_event_is_forwarded_to_chat() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(Vec::new());
    let clock = FixedClock { now: 0 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
            schema_version: 1,
            run_id: RunId("run_1".into()),
            message: "hello from assistant".into(),
            reply_to_session_id: None,
            created_at_ms: 111,
        }))
        .expect("assistant message event");

    let sent_messages = &adapter.poll.sent_messages;
    assert_eq!(sent_messages.len(), 1);
    assert!(sent_messages[0].1.contains("hello from assistant"));
}

#[test]
fn codex_status_card_is_created_then_edited_in_place() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(Vec::new());
    let clock = StepClock::new(61_000, 61_000);
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::CodexSessionStatus(CodexSessionStatusEvent {
            schema_version: 1,
            run_id: RunId("run_codex".into()),
            session_id: "codex-session-1".into(),
            session_name: "you-are-starting".into(),
            cwd: Some("/root/git/modex".into()),
            status: CodexSessionLifecycleStatus::Running,
            started_at_ms: 1_000,
            updated_at_ms: 61_000,
            last_step: "working in /root/git/modex".into(),
            summary: None,
        }))
        .expect("publish codex status");
    assert_eq!(adapter.poll.sent_messages.len(), 1);
    assert!(adapter.poll.sent_messages[0].1.contains("status: running"));

    adapter.poll_once().expect("poll once");
    assert_eq!(adapter.poll.edited_messages.len(), 1);
    assert_eq!(adapter.poll.edited_messages[0].1, 1);
    assert!(adapter.poll.edited_messages[0].2.contains("elapsed: 2m 1s"));
}

#[test]
fn codex_approval_replies_to_status_card_message() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(Vec::new());
    let clock = FixedClock { now: 61_000 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::CodexSessionStatus(CodexSessionStatusEvent {
            schema_version: 1,
            run_id: RunId("run_codex".into()),
            session_id: "codex-session-1".into(),
            session_name: "you-are-starting".into(),
            cwd: Some("/root/git/modex".into()),
            status: CodexSessionLifecycleStatus::Running,
            started_at_ms: 1_000,
            updated_at_ms: 61_000,
            last_step: "working in /root/git/modex".into(),
            summary: None,
        }))
        .expect("publish codex status");
    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(ApprovalRequestedEvent {
            reply_to_session_id: Some("codex-session-1".into()),
            ..sample_approval_requested()
        }))
        .expect("publish approval");

    assert_eq!(adapter.poll.sent_messages.len(), 2);
    assert_eq!(adapter.poll.sent_messages[1].2, Some(1));
}

#[test]
fn codex_assistant_message_replies_to_status_card_message() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(Vec::new());
    let clock = FixedClock { now: 61_000 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::CodexSessionStatus(CodexSessionStatusEvent {
            schema_version: 1,
            run_id: RunId("run_codex".into()),
            session_id: "codex-session-1".into(),
            session_name: "you-are-starting".into(),
            cwd: Some("/root/git/modex".into()),
            status: CodexSessionLifecycleStatus::Completed,
            started_at_ms: 1_000,
            updated_at_ms: 61_000,
            last_step: "completed bounded increment".into(),
            summary: Some("build passed".into()),
        }))
        .expect("publish codex status");
    adapter
        .publish_runtime_event(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
            schema_version: 1,
            run_id: RunId("run_user".into()),
            message: "It’s done.".into(),
            reply_to_session_id: Some("codex-session-1".into()),
            created_at_ms: 61_001,
        }))
        .expect("assistant message event");

    assert_eq!(adapter.poll.sent_messages.len(), 2);
    assert_eq!(adapter.poll.sent_messages[1].2, Some(1));
    assert_eq!(adapter.poll.sent_messages[1].1, "It’s done.");
}

#[test]
fn non_approval_message_is_forwarded_as_prompt() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 9,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 507,
            reply_to_message_id: None,
            text: "show git status".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 7070 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter.poll_once().expect("poll once");
    let prompts = adapter
        .bridge
        .prompts
        .lock()
        .expect("prompts mutex poisoned")
        .clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].chat_id, 42);
    assert_eq!(prompts[0].text, "show git status");
    assert_eq!(prompts[0].modality, InteractionModality::Text);
    assert!(prompts[0].media_file_id.is_none());
}

#[test]
fn voice_marker_message_is_forwarded_as_audio_prompt() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 9_101,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 5_701,
            reply_to_message_id: None,
            text: format!("{TELEGRAM_VOICE_PROMPT_PREFIX}voice-file-1"),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 7_171 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter.poll_once().expect("poll once");
    let prompts = adapter
        .bridge
        .prompts
        .lock()
        .expect("prompts mutex poisoned")
        .clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].modality, InteractionModality::Audio);
    assert_eq!(prompts[0].media_file_id.as_deref(), Some("voice-file-1"));
    assert_eq!(prompts[0].text, "");
}

#[test]
fn image_marker_message_is_forwarded_as_image_prompt() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 9_102,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 5_702,
            reply_to_message_id: None,
            text: format!("{TELEGRAM_IMAGE_PROMPT_PREFIX}photo-file-1"),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 7_172 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter.poll_once().expect("poll once");
    let prompts = adapter
        .bridge
        .prompts
        .lock()
        .expect("prompts mutex poisoned")
        .clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].modality, InteractionModality::Image);
    assert_eq!(prompts[0].media_file_id.as_deref(), Some("photo-file-1"));
    assert_eq!(prompts[0].text, "");
}

#[test]
fn yes_without_pending_approval_is_forwarded_as_prompt() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 9_001,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 509,
            reply_to_message_id: None,
            text: "yes".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 7071 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter.poll_once().expect("poll once");
    let prompts = adapter
        .bridge
        .prompts
        .lock()
        .expect("prompts mutex poisoned")
        .clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].text, "yes");
}

use super::support::{
    sample_approval_requested, sample_approval_requested_with_id, test_config, FixedClock,
    TestBridge, TestPoller,
};
use super::*;

#[test]
fn poll_once_maps_approve_once_to_approval_resolved_event() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 9,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 501,
            reply_to_message_id: None,
            text: "/approve_once apr_1".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 777 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("publish runtime event");
    adapter.poll_once().expect("poll once");

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].request_id.0, "apr_1");
    assert_eq!(approvals[0].run_id.0, "run_1");
    assert_eq!(approvals[0].action, ApprovalAction::ApproveOnce);
    assert_eq!(approvals[0].created_at_ms, 777);
}

#[test]
fn poll_once_maps_deny_to_approval_resolved_event() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 11,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 502,
            reply_to_message_id: None,
            text: "/deny apr_1".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 888 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("publish runtime event");
    adapter.poll_once().expect("poll once");

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].action, ApprovalAction::Deny);
}

#[test]
fn approve_alias_is_mapped_to_approve_once() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 3,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 503,
            reply_to_message_id: None,
            text: "approve apr_1".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 1010 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("publish runtime event");
    adapter.poll_once().expect("poll once");

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].action, ApprovalAction::ApproveOnce);
}

#[test]
fn unknown_request_id_is_ignored_without_chat_noise() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 5,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 505,
            reply_to_message_id: None,
            text: "/deny apr_missing".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 3030 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("publish runtime event");
    adapter.poll_once().expect("poll once");

    let sent_messages = &adapter.poll.sent_messages;
    assert_eq!(sent_messages.len(), 1);

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert!(approvals.is_empty());
}

#[test]
fn yes_reply_to_request_message_approves_once() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 6,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 506,
            reply_to_message_id: Some(1),
            text: "yes".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 4040 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("publish runtime event");
    adapter.poll_once().expect("poll once");

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].action, ApprovalAction::ApproveOnce);
}

#[test]
fn thumbs_up_reaction_approves_once() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 7,
        message: None,
        message_reaction: Some(TelegramMessageReaction {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 1,
            emoji: vec!["👍".into()],
        }),
    }]]);
    let clock = FixedClock { now: 5050 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("publish runtime event");
    adapter.poll_once().expect("poll once");

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].action, ApprovalAction::ApproveOnce);
}

#[test]
fn thumbs_down_reaction_denies() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 8,
        message: None,
        message_reaction: Some(TelegramMessageReaction {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 1,
            emoji: vec!["👎".into()],
        }),
    }]]);
    let clock = FixedClock { now: 6060 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);

    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
        .expect("publish runtime event");
    adapter.poll_once().expect("poll once");

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].action, ApprovalAction::Deny);
}

#[test]
fn ambiguous_yes_without_reply_gets_help_message() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 10,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 508,
            reply_to_message_id: None,
            text: "y".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 8080 };
    let mut adapter = TelegramAdapter::new(test_config(None), bridge, poller, clock);
    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(
            sample_approval_requested_with_id("apr_1", "run_1"),
        ))
        .expect("publish runtime event");
    adapter
        .publish_runtime_event(RuntimeEvent::ApprovalRequested(
            sample_approval_requested_with_id("apr_2", "run_2"),
        ))
        .expect("publish runtime event");

    adapter.poll_once().expect("poll once");

    let approvals = adapter
        .bridge
        .approvals
        .lock()
        .expect("approvals mutex poisoned")
        .clone();
    assert!(approvals.is_empty());
    let sent_messages = &adapter.poll.sent_messages;
    let last = sent_messages.last().expect("help text");
    assert!(last.1.contains("approval target unclear"));
}

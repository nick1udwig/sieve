use super::support::{sample_approval_requested, test_config, FixedClock, TestBridge, TestPoller};
use super::*;

#[test]
fn ignores_messages_from_unconfigured_chat() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 4,
        message: Some(TelegramMessage {
            chat_id: 7,
            sender_user_id: Some(1001),
            message_id: 504,
            reply_to_message_id: None,
            text: "/deny apr_1".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 2020 };
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
    assert!(approvals.is_empty());
}

#[test]
fn allowlisted_sender_message_command_is_processed() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 4_101,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 5_101,
            reply_to_message_id: None,
            text: "/approve_once apr_1".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 2_021 };
    let mut adapter = TelegramAdapter::new(
        test_config(Some(BTreeSet::from([1001]))),
        bridge,
        poller,
        clock,
    );

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
fn non_allowlisted_sender_message_command_is_ignored() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 4_102,
        message: Some(TelegramMessage {
            chat_id: 42,
            sender_user_id: Some(2002),
            message_id: 5_102,
            reply_to_message_id: None,
            text: "/deny apr_1".into(),
        }),
        message_reaction: None,
    }]]);
    let clock = FixedClock { now: 2_022 };
    let mut adapter = TelegramAdapter::new(
        test_config(Some(BTreeSet::from([1001]))),
        bridge,
        poller,
        clock,
    );

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
    assert!(approvals.is_empty());
}

#[test]
fn allowlisted_sender_reaction_is_processed() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 8_101,
        message: None,
        message_reaction: Some(TelegramMessageReaction {
            chat_id: 42,
            sender_user_id: Some(1001),
            message_id: 1,
            emoji: vec!["👍".into()],
        }),
    }]]);
    let clock = FixedClock { now: 6_061 };
    let mut adapter = TelegramAdapter::new(
        test_config(Some(BTreeSet::from([1001]))),
        bridge,
        poller,
        clock,
    );

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
fn non_allowlisted_sender_reaction_is_ignored() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![vec![TelegramUpdate {
        update_id: 8_102,
        message: None,
        message_reaction: Some(TelegramMessageReaction {
            chat_id: 42,
            sender_user_id: Some(2002),
            message_id: 1,
            emoji: vec!["👎".into()],
        }),
    }]]);
    let clock = FixedClock { now: 6_062 };
    let mut adapter = TelegramAdapter::new(
        test_config(Some(BTreeSet::from([1001]))),
        bridge,
        poller,
        clock,
    );

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
    assert!(approvals.is_empty());
}

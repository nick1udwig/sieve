use super::support::{test_config, StepClock, TestBridge, TestPoller};
use super::*;

#[test]
fn typing_indicator_starts_and_stops_cleanly() {
    let bridge = TestBridge::new();
    let poller = TestPoller::new(vec![Vec::new(), Vec::new(), Vec::new()]);
    let mut adapter = TelegramAdapter::new(
        test_config(None),
        bridge,
        poller,
        StepClock::new(1_000, 5_000),
    );

    adapter.start_typing("run-1").expect("start typing");
    assert_eq!(adapter.poll.sent_chat_actions.len(), 1);
    assert_eq!(
        adapter.poll.sent_chat_actions[0],
        (42, "typing".to_string())
    );

    adapter.poll_once().expect("poll with typing");
    assert_eq!(adapter.poll.sent_chat_actions.len(), 2);
    adapter.stop_typing("run-1");
    adapter.poll_once().expect("poll after stop");
    assert_eq!(adapter.poll.sent_chat_actions.len(), 2);
}

use super::*;
#[tokio::test]
async fn live_e2e_greeting_stays_chat_only_env_gated() {
    let _guard = env_test_lock()
        .lock()
        .expect("live e2e env test lock poisoned");
    let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
        "bash".to_string(),
        "endorse".to_string(),
        "declassify".to_string(),
    ]) else {
        return;
    };

    harness
        .run_text_turn("Hi can you hear me?")
        .await
        .expect("live greeting should succeed");

    let events = harness.runtime_events();
    assert_eq!(count_approval_requested(&events), 0);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
        "greeting should remain chat-only with zero tool dispatches"
    );
    let assistant = assistant_messages(&events);
    assert_eq!(assistant.len(), 1);
    assert!(
        !assistant[0].trim().is_empty() && !assistant[0].starts_with("error:"),
        "live greeting must produce a non-error assistant reply"
    );
    let records = harness.jsonl_records();
    assert!(
        assistant_errors_from_conversation(&records).is_empty(),
        "live greeting must not produce assistant error conversation entries"
    );
}

#[tokio::test]
async fn live_telegram_full_flow_greeting_env_gated() {
    let _guard = env_test_lock()
        .lock()
        .expect("live e2e env test lock poisoned");
    let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
        "bash".to_string(),
        "endorse".to_string(),
        "declassify".to_string(),
    ]) else {
        return;
    };

    let flow = harness
        .run_telegram_text_turn("Hi how are you?")
        .await
        .expect("live telegram greeting should succeed");
    let message =
        latest_telegram_message(&flow).expect("live telegram greeting should send message");

    assert!(
        !message.trim().is_empty() && !message.starts_with("error:"),
        "live telegram greeting must produce a non-error assistant reply"
    );
    assert!(
        !obvious_meta_compose_pattern(message),
        "live telegram greeting reply should be direct, not third-person meta"
    );
    assert!(
        flow.sent_chat_actions
            .iter()
            .any(|(_, action)| action == "typing"),
        "live telegram greeting should emit typing action"
    );
}

#[tokio::test]
async fn live_telegram_full_flow_weather_today_env_gated() {
    let _guard = env_test_lock()
        .lock()
        .expect("live e2e env test lock poisoned");
    let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
        "bash".to_string(),
        "endorse".to_string(),
        "declassify".to_string(),
    ]) else {
        return;
    };

    let flow = harness
        .run_telegram_text_turn("weather in dublin ireland today")
        .await
        .expect("live telegram weather today should succeed");
    let message =
        latest_telegram_message(&flow).expect("live telegram weather today should send message");
    let lower = message.to_ascii_lowercase();

    assert!(
        !message.starts_with("error:") && !obvious_meta_compose_pattern(message),
        "live telegram weather today should produce direct non-error response"
    );
    assert!(
        message_contains_plain_url(message),
        "live telegram weather today response should include at least one plain URL"
    );
    assert!(
        message_has_weather_signal(message),
        "live telegram weather today response should include concrete weather signal"
    );
    assert!(
        lower.contains("today") || lower.contains("current") || lower.contains("now"),
        "live telegram weather today response should answer the requested timeframe"
    );
}

#[tokio::test]
async fn live_telegram_full_flow_weather_tomorrow_env_gated() {
    let _guard = env_test_lock()
        .lock()
        .expect("live e2e env test lock poisoned");
    let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
        "bash".to_string(),
        "endorse".to_string(),
        "declassify".to_string(),
    ]) else {
        return;
    };

    let flow = harness
        .run_telegram_text_turn("weather in dublin ireland tomorrow")
        .await
        .expect("live telegram weather tomorrow should succeed");
    let message =
        latest_telegram_message(&flow).expect("live telegram weather tomorrow should send message");
    let lower = message.to_ascii_lowercase();

    assert!(
        !message.starts_with("error:") && !obvious_meta_compose_pattern(message),
        "live telegram weather tomorrow should produce direct non-error response"
    );
    assert!(
        message_contains_plain_url(message),
        "live telegram weather tomorrow response should include at least one plain URL"
    );
    assert!(
        message_has_weather_signal(message),
        "live telegram weather tomorrow response should include concrete weather signal"
    );
    assert!(
        lower.contains("tomorrow"),
        "live telegram weather tomorrow response should answer the requested timeframe"
    );
}

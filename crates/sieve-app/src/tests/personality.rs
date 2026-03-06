use super::*;

fn temp_sieve_home(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sieve-app-personality-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    fs::create_dir_all(&path).expect("create temp sieve home");
    path
}

#[test]
fn resolve_turn_personality_applies_telegram_defaults() {
    let sieve_home = temp_sieve_home("telegram-defaults");
    let resolution = resolve_turn_personality(
        &sieve_home,
        PromptSource::Telegram,
        Some("42"),
        InteractionModality::Text,
        InteractionModality::Text,
        "hi there",
    )
    .expect("resolve personality");

    assert_eq!(
        resolution.delivery_context.channel,
        sieve_types::DeliveryChannel::Telegram
    );
    assert_eq!(
        resolution.delivery_context.destination.as_deref(),
        Some("42")
    );
    assert_eq!(
        resolution.resolved_personality.emoji_policy,
        sieve_types::EmojiPolicy::Light
    );
    assert_eq!(
        resolution.resolved_personality.verbosity,
        sieve_types::ResponseVerbosity::Concise
    );
    assert!(
        resolution
            .resolved_personality
            .channel_guidance
            .iter()
            .any(|line| line.contains("Telegram")),
        "telegram channel guidance should be present"
    );

    let _ = fs::remove_dir_all(sieve_home);
}

#[test]
fn resolve_turn_personality_persists_and_reuses_style_updates() {
    let sieve_home = temp_sieve_home("persist");
    let first = resolve_turn_personality(
        &sieve_home,
        PromptSource::Stdin,
        None,
        InteractionModality::Text,
        InteractionModality::Text,
        "please don't use emojis",
    )
    .expect("resolve personality");
    assert_eq!(
        first.acknowledgement.as_deref(),
        Some("I'll skip emojis from now on.")
    );
    assert_eq!(
        first.resolved_personality.emoji_policy,
        sieve_types::EmojiPolicy::Avoid
    );
    assert!(personality_state_path(&sieve_home).exists());

    let second = resolve_turn_personality(
        &sieve_home,
        PromptSource::Telegram,
        Some("42"),
        InteractionModality::Text,
        InteractionModality::Text,
        "hi again",
    )
    .expect("resolve personality");
    assert!(second.acknowledgement.is_none());
    assert_eq!(
        second.resolved_personality.emoji_policy,
        sieve_types::EmojiPolicy::Avoid
    );

    let _ = fs::remove_dir_all(sieve_home);
}

#[test]
fn resolve_turn_personality_keeps_turn_scoped_overrides_ephemeral() {
    let sieve_home = temp_sieve_home("ephemeral");
    let first = resolve_turn_personality(
        &sieve_home,
        PromptSource::Stdin,
        None,
        InteractionModality::Text,
        InteractionModality::Text,
        "for this reply, speak more tersely to conserve tokens: pretend we are communicating over telegraph",
    )
    .expect("resolve personality");
    assert_eq!(
        first.acknowledgement.as_deref(),
        Some("For this reply, I'll use terse, telegraph-style phrasing.")
    );
    assert_eq!(
        first.resolved_personality.verbosity,
        sieve_types::ResponseVerbosity::Telegraph
    );
    assert!(
        !personality_state_path(&sieve_home).exists(),
        "turn-scoped override should not persist"
    );

    let second = resolve_turn_personality(
        &sieve_home,
        PromptSource::Stdin,
        None,
        InteractionModality::Text,
        InteractionModality::Text,
        "hello",
    )
    .expect("resolve personality");
    assert_eq!(
        second.resolved_personality.verbosity,
        sieve_types::ResponseVerbosity::Concise
    );

    let _ = fs::remove_dir_all(sieve_home);
}

#[test]
fn resolve_turn_personality_allows_expressive_persona_requests() {
    let sieve_home = temp_sieve_home("expressive-persona");
    let resolution = resolve_turn_personality(
        &sieve_home,
        PromptSource::Telegram,
        Some("42"),
        InteractionModality::Text,
        InteractionModality::Text,
        "I want you to start using a lot of emojis (2+ per message) and start acting like a valley girl",
    )
    .expect("resolve personality");

    assert_eq!(
        resolution.acknowledgement.as_deref(),
        Some(
            "I'll lean into emojis, use an emoji-heavy chat style with multiple emojis in text replies, and adopt a valley girl persona from now on."
        )
    );
    assert_eq!(
        resolution.resolved_personality.emoji_policy,
        sieve_types::EmojiPolicy::Auto
    );
    assert!(
        resolution
            .resolved_personality
            .custom_instructions
            .iter()
            .any(|line| line.contains("emoji-heavy")),
        "emoji-heavy instruction should be preserved"
    );
    assert!(
        resolution
            .resolved_personality
            .custom_instructions
            .iter()
            .any(|line| line.contains("valley girl persona")),
        "persona instruction should be preserved"
    );
    assert!(personality_state_path(&sieve_home).exists());

    let _ = fs::remove_dir_all(sieve_home);
}

#[tokio::test]
async fn e2e_response_model_receives_delivery_context_and_personality() {
    let planner: Arc<dyn PlannerModel> =
        Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("chat only".to_string()),
            tool_calls: Vec::new(),
        })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response_impl = Arc::new(RecordingResponseModel::new("Hello there."));
    let response: Arc<dyn ResponseModel> = response_impl.clone();
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["bash".to_string()],
        E2E_POLICY_BASE,
    );

    harness
        .run_telegram_text_turn("Hi how are you?")
        .await
        .expect("telegram turn should succeed");

    let input = response_impl
        .last_input()
        .expect("response model should record input");
    assert_eq!(
        input.delivery_context.channel,
        sieve_types::DeliveryChannel::Telegram
    );
    assert_eq!(input.delivery_context.destination.as_deref(), Some("42"));
    assert_eq!(input.response_modality, InteractionModality::Text);
    assert_eq!(
        input.resolved_personality.emoji_policy,
        sieve_types::EmojiPolicy::Light
    );
    assert!(
        input
            .resolved_personality
            .channel_guidance
            .iter()
            .any(|line| line.contains("Telegram")),
        "telegram guidance should reach response model"
    );
}

#[tokio::test]
async fn e2e_style_only_persona_request_is_acknowledged_not_refused() {
    let planner: Arc<dyn PlannerModel> =
        Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("should not run".to_string()),
            tool_calls: Vec::new(),
        })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(RecordingResponseModel::new("unused"));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["bash".to_string()],
        E2E_POLICY_BASE,
    );

    let flow = harness
        .run_telegram_text_turn(
            "I want you to start using a lot of emojis (2+ per message) and start acting like a valley girl",
        )
        .await
        .expect("telegram turn should succeed");
    let message = latest_telegram_message(&flow).expect("telegram message should be sent");
    assert_eq!(
        message,
        "I'll lean into emojis, use an emoji-heavy chat style with multiple emojis in text replies, and adopt a valley girl persona from now on."
    );
}

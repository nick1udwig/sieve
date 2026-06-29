use crate::text_match::contains_any_word;
use sieve_llm::ResponseTurnInput;
use sieve_types::TrustedToolEffect;

fn message_or_reason_negates_scheduling(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let denies_action = lower.contains("i can't")
        || lower.contains("i cannot")
        || lower.contains("can't actually")
        || lower.contains("cannot actually")
        || lower.contains("unable to")
        || lower.contains("do not have")
        || lower.contains("don't have")
        || lower.contains("not available")
        || lower.contains("unsupported")
        || lower.contains("not actionable");
    let mentions_schedule = contains_any_word(
        &lower,
        &[
            "schedule",
            "scheduled",
            "scheduling",
            "cron",
            "reminder",
            "reminders",
            "message",
            "messages",
            "send",
            "sent",
            "tool",
            "tools",
        ],
    );
    denies_action && mentions_schedule
}

fn text_negates_effect(text: &str, effect: &TrustedToolEffect) -> bool {
    match effect {
        TrustedToolEffect::CronAdded { .. } => message_or_reason_negates_scheduling(text),
    }
}

pub(crate) fn message_negates_trusted_effects(
    message: &str,
    response_input: &ResponseTurnInput,
) -> bool {
    response_input
        .trusted_effects
        .iter()
        .any(|effect| text_negates_effect(message, effect))
}

pub(super) fn reason_negates_trusted_effects(
    reason: &str,
    response_input: &ResponseTurnInput,
) -> bool {
    response_input
        .trusted_effects
        .iter()
        .any(|effect| text_negates_effect(reason, effect))
}

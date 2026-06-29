use chrono::{TimeZone, Utc};
use sieve_llm::ResponseTurnInput;
use sieve_types::{AutomationDeliveryMode, TrustedToolEffect};

pub(super) fn trusted_effect_fallback_message(
    response_input: &ResponseTurnInput,
) -> Option<String> {
    response_input
        .trusted_effects
        .iter()
        .map(|effect| match effect {
            TrustedToolEffect::CronAdded {
                prompt,
                run_at_ms,
                delivery_mode,
                ..
            } => {
                let when = Utc
                    .timestamp_millis_opt(*run_at_ms as i64)
                    .single()
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| run_at_ms.to_string());
                let destination = match delivery_mode {
                    AutomationDeliveryMode::MainSessionMessage => "here",
                    AutomationDeliveryMode::IsolatedTurn => "in a separate turn",
                };
                format!(
                    "Scheduled. I'll send `{}` {} at {}.",
                    prompt.trim(),
                    destination,
                    when
                )
            }
        })
        .next()
}

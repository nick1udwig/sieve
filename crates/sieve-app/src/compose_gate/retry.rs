use super::effects::{message_negates_trusted_effects, reason_negates_trusted_effects};
use super::verdict::ComposeGateOutput;
#[cfg(test)]
use super::verdict::{parse_gate_verdict, GateVerdict};
use crate::response_style::{concise_style_diagnostic, obvious_meta_compose_pattern};
use sieve_llm::ResponseTurnInput;

#[cfg(test)]
pub(crate) fn compose_quality_requires_retry(
    composed_message: &str,
    quality_gate: Option<&str>,
) -> Option<String> {
    if obvious_meta_compose_pattern(composed_message) {
        return Some(
            "response used third-person meta narration; respond directly to user".to_string(),
        );
    }
    match parse_gate_verdict(quality_gate) {
        None | Some(GateVerdict::Pass) => None,
        Some(GateVerdict::Revise(reason)) => Some(reason),
    }
}

#[cfg(test)]
pub(crate) fn gate_requires_retry(gate: Option<&str>) -> Option<String> {
    match parse_gate_verdict(gate) {
        None | Some(GateVerdict::Pass) => None,
        Some(GateVerdict::Revise(reason)) => Some(reason),
    }
}

pub(crate) fn compose_gate_requires_retry(
    composed_message: &str,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    gate: Option<&ComposeGateOutput>,
) -> Option<String> {
    if obvious_meta_compose_pattern(composed_message) {
        return Some(
            "response used third-person meta narration; respond directly to user".to_string(),
        );
    }
    if let Some(diagnostic) = concise_style_diagnostic(composed_message, trusted_user_message) {
        return Some(diagnostic);
    }
    if message_negates_trusted_effects(composed_message, response_input) {
        return Some("response contradicted a trusted runtime side effect".to_string());
    }
    let gate = gate?;
    if gate.verdict.eq_ignore_ascii_case("PASS") {
        return None;
    }
    if reason_negates_trusted_effects(gate.reason.as_deref().unwrap_or_default(), response_input) {
        return None;
    }
    gate.reason
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some("requested revision".to_string()))
}

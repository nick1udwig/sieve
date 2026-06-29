use super::effects::reason_negates_trusted_effects;
use super::verdict::{continue_signal_from_code, ComposeGateOutput};
#[cfg(test)]
use super::verdict::{parse_gate_verdict, GateVerdict};
use crate::text_match::{contains_phrase_words, contains_word};
use crate::turn::{response_has_explicit_answer_candidate, response_has_trusted_effect};
use sieve_llm::ResponseTurnInput;
use sieve_types::PlannerGuidanceSignal;

fn mentions_denied_tool_reason(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    contains_word(&lower, "denied")
        || contains_word(&lower, "blocked")
        || contains_phrase_words(&lower, &["not", "allowed"])
        || contains_phrase_words(&lower, &["unknown", "command"])
        || contains_phrase_words(&lower, &["tool", "failure"])
}

fn followup_signal_from_reason(
    reason: &str,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    if reason_negates_trusted_effects(reason, response_input) {
        return None;
    }
    let has_tool_context = response_input
        .tool_outcomes
        .iter()
        .any(|outcome| !outcome.refs.is_empty() || outcome.failure_reason.is_some());
    if !has_tool_context {
        return None;
    }

    let lower = reason.to_ascii_lowercase();
    let has_explicit_answer_candidate = response_has_explicit_answer_candidate(response_input);
    if has_explicit_answer_candidate
        && (lower.contains("interstitial")
            || lower.contains("google sorry")
            || lower.contains("sorry page")
            || lower.contains("captcha")
            || lower.contains("login")
            || lower.contains("title-only")
            || lower.contains("page title")
            || lower.contains("page-level output")
            || lower.contains("higher quality"))
    {
        return None;
    }

    let is_style_only = lower.contains("third-person")
        || lower.contains("third person")
        || lower.contains("meta narration")
        || contains_word(&lower, "tone");
    if is_style_only {
        return None;
    }

    if lower.contains("current page")
        || lower.contains("already-open page")
        || lower.contains("title-only")
        || lower.contains("page title")
        || lower.contains("page-level output")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedCurrentPageInspection);
    }

    if lower.contains("captcha")
        || lower.contains("interstitial")
        || lower.contains("google sorry")
        || lower.contains("sorry page")
        || lower.contains("unusual traffic")
        || lower.contains("consent")
        || lower.contains("paywall")
        || lower.contains("login")
    {
        return Some(PlannerGuidanceSignal::ContinueEncounteredAccessInterstitial);
    }

    if lower.contains("reformulate")
        || lower.contains("command shape")
        || lower.contains("same target")
        || lower.contains("different command form")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedCommandReformulation);
    }

    let denied_command_present = response_input.tool_outcomes.iter().any(|outcome| {
        outcome
            .failure_reason
            .as_deref()
            .map(mentions_denied_tool_reason)
            .unwrap_or(false)
    });
    if denied_command_present || mentions_denied_tool_reason(&lower) {
        return Some(PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool);
    }

    if lower.contains("conflict")
        || lower.contains("contradict")
        || lower.contains("inconsistent")
        || lower.contains("disagree")
    {
        return Some(PlannerGuidanceSignal::ContinueResolveSourceConflict);
    }

    if contains_word(&lower, "stale")
        || contains_word(&lower, "outdated")
        || contains_word(&lower, "latest")
        || contains_word(&lower, "fresh")
        || contains_word(&lower, "refresh")
        || contains_word(&lower, "refreshed")
        || lower.contains("current as of")
        || lower.contains("time-bound")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence);
    }

    if lower.contains("no progress")
        || lower.contains("repeated")
        || lower.contains("same command")
        || lower.contains("no evidence gain")
    {
        return Some(PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction);
    }

    if lower.contains("asset")
        || lower.contains("favicon")
        || lower.contains("image url")
        || lower.contains("non-content url")
        || lower.contains("canonical content page")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl);
    }

    if lower.contains("extract url")
        || lower.contains("url extraction")
        || lower.contains("parse urls")
        || lower.contains("extract links")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedUrlExtraction);
    }

    if lower.contains("primary source")
        || lower.contains("primary-page")
        || lower.contains("primary content")
        || lower.contains("discovery/search snippets")
        || lower.contains("snippet-only")
        || lower.contains("insufficient source")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch);
    }

    if lower.contains("higher quality")
        || lower.contains("low quality source")
        || lower.contains("needs citation")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedHigherQualitySource);
    }

    if lower.contains("missing parameter")
        || lower.contains("need user input")
        || lower.contains("needs clarification")
        || lower.contains("please specify")
        || lower.contains("missing required")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedRequiredParameter);
    }

    if contains_word(&lower, "preference")
        || contains_word(&lower, "constraint")
        || contains_word(&lower, "format")
        || contains_word(&lower, "units")
        || contains_word(&lower, "locale")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint);
    }

    Some(PlannerGuidanceSignal::ContinueRefineApproach)
}

#[cfg(test)]
pub(crate) fn compose_quality_followup_signal(
    quality_gate: Option<&str>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let reason = match parse_gate_verdict(quality_gate) {
        None | Some(GateVerdict::Pass) => return None,
        Some(GateVerdict::Revise(reason)) => reason,
    };
    followup_signal_from_reason(&reason, response_input)
}

pub(crate) fn compose_gate_followup_signal(
    gate: Option<&ComposeGateOutput>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let gate = gate?;
    if reason_negates_trusted_effects(gate.reason.as_deref().unwrap_or_default(), response_input) {
        return None;
    }
    if let Some(signal) = gate.continue_code.and_then(continue_signal_from_code) {
        let has_explicit_answer_candidate = response_has_explicit_answer_candidate(response_input);
        if (has_explicit_answer_candidate || response_has_trusted_effect(response_input))
            && matches!(
                signal,
                PlannerGuidanceSignal::ContinueNeedEvidence
                    | PlannerGuidanceSignal::ContinueNeedHigherQualitySource
                    | PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch
                    | PlannerGuidanceSignal::ContinueNeedCurrentPageInspection
                    | PlannerGuidanceSignal::ContinueEncounteredAccessInterstitial
                    | PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction
            )
        {
            return None;
        }
        return Some(signal);
    }
    if gate.verdict.eq_ignore_ascii_case("PASS") {
        return None;
    }
    let reason = gate.reason.as_deref().unwrap_or("requested revision");
    followup_signal_from_reason(reason, response_input)
}

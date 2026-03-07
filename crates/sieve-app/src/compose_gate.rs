use crate::planner_progress::guidance_requests_continue;
use crate::response_style::{
    compact_single_line, concise_style_diagnostic, dedupe_preserve_order,
    obvious_meta_compose_pattern,
};
use crate::turn::response_has_explicit_answer_candidate;
use serde::{Deserialize, Serialize};
use sieve_llm::ResponseTurnInput;
use sieve_types::PlannerGuidanceSignal;

pub(crate) fn extract_trusted_evidence_lines(
    trusted_user_message: &str,
    planner_thoughts: Option<&str>,
) -> Vec<String> {
    let mut lines = vec![format!("[user] {trusted_user_message}")];
    if let Some(thoughts) = planner_thoughts {
        for line in thoughts.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("[user] ") {
                lines.push(trimmed.to_string());
            }
        }
    }
    dedupe_preserve_order(lines)
}

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

pub(crate) fn combine_gate_reasons(gates: &[Option<String>]) -> Option<String> {
    let mut combined = Vec::new();
    for gate in gates {
        if let Some(gate) = gate.as_deref() {
            let trimmed = gate.trim();
            if !trimmed.is_empty() {
                combined.push(trimmed.to_string());
            }
        }
    }
    if combined.is_empty() {
        None
    } else {
        Some(combined.join(" | "))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateVerdict {
    Pass,
    Revise(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ComposeGateOutput {
    pub(crate) verdict: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) continue_code: Option<u16>,
}

fn parse_gate_verdict(gate: Option<&str>) -> Option<GateVerdict> {
    let gate = gate.unwrap_or("").trim();
    if gate.is_empty() {
        return None;
    }
    let lower = gate.to_ascii_lowercase();
    if let Some(revise_idx) = lower.find("revise") {
        let reason = gate[revise_idx + "revise".len()..]
            .trim_start_matches(|ch: char| ch == ':' || ch == '-' || ch.is_whitespace())
            .trim();
        if reason.is_empty() {
            return Some(GateVerdict::Revise("requested revision".to_string()));
        }
        return Some(GateVerdict::Revise(reason.to_string()));
    }
    if lower.starts_with("pass") || (lower.contains("pass") && !lower.contains("revise")) {
        return Some(GateVerdict::Pass);
    }
    Some(GateVerdict::Revise(format!(
        "unstructured gate output: {}",
        compact_single_line(gate, 200)
    )))
}

fn followup_signal_from_reason(
    reason: &str,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
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
        || lower.contains("tone");
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
            .map(|reason| {
                let reason = reason.to_ascii_lowercase();
                reason.contains("denied")
                    || reason.contains("blocked")
                    || reason.contains("not allowed")
                    || reason.contains("unknown command")
            })
            .unwrap_or(false)
    });
    if denied_command_present
        || lower.contains("denied")
        || lower.contains("blocked")
        || lower.contains("not allowed")
        || lower.contains("unknown command")
        || lower.contains("tool failure")
    {
        return Some(PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool);
    }

    if lower.contains("conflict")
        || lower.contains("contradict")
        || lower.contains("inconsistent")
        || lower.contains("disagree")
    {
        return Some(PlannerGuidanceSignal::ContinueResolveSourceConflict);
    }

    if lower.contains("stale")
        || lower.contains("outdated")
        || lower.contains("latest")
        || lower.contains("fresh")
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

    if lower.contains("preference")
        || lower.contains("constraint")
        || lower.contains("format")
        || lower.contains("units")
        || lower.contains("locale")
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

fn continue_signal_from_code(code: u16) -> Option<PlannerGuidanceSignal> {
    PlannerGuidanceSignal::try_from(code)
        .ok()
        .filter(|signal| guidance_requests_continue(*signal))
}

pub(crate) fn parse_compose_gate_output(raw: Option<&str>) -> Option<ComposeGateOutput> {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<ComposeGateOutput>(raw) {
        let verdict = parsed.verdict.trim().to_ascii_uppercase();
        let reason = parsed.reason.map(|value| value.trim().to_string());
        if verdict == "PASS" {
            return Some(ComposeGateOutput {
                verdict,
                reason: None,
                continue_code: parsed
                    .continue_code
                    .and_then(continue_signal_from_code)
                    .map(|signal| signal.code()),
            });
        }
        return Some(ComposeGateOutput {
            verdict: "REVISE".to_string(),
            reason: reason
                .filter(|value| !value.is_empty())
                .or_else(|| Some("requested revision".to_string())),
            continue_code: parsed
                .continue_code
                .and_then(continue_signal_from_code)
                .map(|signal| signal.code()),
        });
    }
    match parse_gate_verdict(Some(raw)) {
        None => None,
        Some(GateVerdict::Pass) => Some(ComposeGateOutput {
            verdict: "PASS".to_string(),
            reason: None,
            continue_code: None,
        }),
        Some(GateVerdict::Revise(reason)) => Some(ComposeGateOutput {
            verdict: "REVISE".to_string(),
            reason: Some(reason),
            continue_code: None,
        }),
    }
}

pub(crate) fn compose_gate_requires_retry(
    composed_message: &str,
    trusted_user_message: &str,
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
    let gate = gate?;
    if gate.verdict.eq_ignore_ascii_case("PASS") {
        return None;
    }
    gate.reason
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some("requested revision".to_string()))
}

pub(crate) fn compose_gate_followup_signal(
    gate: Option<&ComposeGateOutput>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let gate = gate?;
    if let Some(signal) = gate.continue_code.and_then(continue_signal_from_code) {
        let has_explicit_answer_candidate = response_has_explicit_answer_candidate(response_input);
        if has_explicit_answer_candidate
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

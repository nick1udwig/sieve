use crate::planner_progress::guidance_requests_continue;
use crate::response_style::compact_single_line;
use crate::text_match::{is_word_char, starts_with_word, word_matches_at};
use serde::{Deserialize, Serialize};
use sieve_types::PlannerGuidanceSignal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GateVerdict {
    Pass,
    Revise(String),
}

const NEGATING_PASS_WORDS: &[&str] = &[
    "not", "no", "never", "cannot", "can't", "cant", "don't", "dont", "doesn't", "doesnt", "isn't",
    "isnt", "won't", "wont",
];

fn pass_word_is_negated(text: &str, pass_idx: usize) -> bool {
    let current_clause = text[..pass_idx]
        .rsplit(['\n', '.', ';'])
        .next()
        .unwrap_or_default();
    current_clause
        .split(|ch: char| !is_word_char(ch) && ch != '\'')
        .rev()
        .filter(|word| !word.is_empty())
        .take(4)
        .any(|word| NEGATING_PASS_WORDS.contains(&word))
}

fn revise_word_is_negated(text: &str, revise_idx: usize) -> bool {
    pass_word_is_negated(text, revise_idx) || text[..revise_idx].trim_end().ends_with("no need to")
}

fn is_pass_verdict_text(gate: &str) -> bool {
    let lower = gate.to_ascii_lowercase();
    if starts_with_word(&lower, "pass") {
        return true;
    }
    lower.match_indices("pass").any(|(idx, _)| {
        word_matches_at(&lower, idx, "pass")
            && !pass_word_is_negated(&lower, idx)
            && lower[..idx]
                .rsplit(['\n', '.', ';'])
                .next()
                .is_some_and(|prefix| prefix.contains("verdict"))
    })
}

fn unnegated_revise_idx(text: &str) -> Option<usize> {
    text.match_indices("revise")
        .map(|(idx, _)| idx)
        .find(|idx| word_matches_at(text, *idx, "revise") && !revise_word_is_negated(text, *idx))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ComposeGateOutput {
    pub(crate) verdict: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) continue_code: Option<u16>,
}

pub(super) fn parse_gate_verdict(gate: Option<&str>) -> Option<GateVerdict> {
    let gate = gate.unwrap_or("").trim();
    if gate.is_empty() {
        return None;
    }
    let lower = gate.to_ascii_lowercase();
    if let Some(revise_idx) = unnegated_revise_idx(&lower) {
        let reason = gate[revise_idx + "revise".len()..]
            .trim_start_matches(|ch: char| ch == ':' || ch == '-' || ch.is_whitespace())
            .trim();
        if reason.is_empty() {
            return Some(GateVerdict::Revise("requested revision".to_string()));
        }
        return Some(GateVerdict::Revise(reason.to_string()));
    }
    if is_pass_verdict_text(gate) {
        return Some(GateVerdict::Pass);
    }
    Some(GateVerdict::Revise(format!(
        "unstructured gate output: {}",
        compact_single_line(gate, 200)
    )))
}

pub(super) fn continue_signal_from_code(code: u16) -> Option<PlannerGuidanceSignal> {
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

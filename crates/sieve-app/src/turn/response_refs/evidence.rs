use super::summary::summarize_with_ref_id_counted;
use crate::json_util::to_json_value;
use crate::render_refs::RenderRef;
use crate::shell_words::{command_words_for_simple_argv, command_words_for_simple_exec};
use serde::Serialize;
use sieve_llm::{ResponseEvidenceRecord, ResponseToolOutcome, ResponseTurnInput, SummaryModel};
use sieve_shell::{BasicShellAnalyzer, ShellAnalyzer};
use sieve_types::{CommandKnowledge, RunId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize)]
struct ResponseEvidenceBatchRef {
    ref_id: String,
    kind: String,
    byte_count: u64,
    line_count: u64,
    content: String,
}

#[derive(Serialize)]
struct ResponseEvidenceBatchPayload<'a> {
    task: &'static str,
    trusted_user_message: &'a str,
    refs: Vec<ResponseEvidenceBatchRef>,
}

#[derive(Clone)]
struct EvidenceRefCandidate {
    ref_id: String,
    kind: String,
    byte_count: u64,
    line_count: u64,
    priority: u8,
    sequence: usize,
}

fn evidence_ref_priority(outcome: &ResponseToolOutcome) -> u8 {
    let attempted_command = outcome.attempted_command.as_deref().unwrap_or("");
    let is_dry_run = command_words_for_simple_exec(attempted_command)
        .into_iter()
        .any(|token| token == "--dry-run" || token.starts_with("--dry-run="));
    let is_schema = command_is_gws_schema_only(attempted_command);
    let is_live = !is_dry_run && !is_schema;
    let is_success = outcome.outcome.contains("exit_code=Some(0)") || outcome.outcome == "executed";

    match (is_live, is_success) {
        (true, true) => 3,
        (true, false) => 2,
        (false, true) => 1,
        (false, false) => 0,
    }
}

fn command_is_gws_schema_only(command: &str) -> bool {
    let Ok(analysis) = BasicShellAnalyzer.analyze_shell_lc_script(command) else {
        return false;
    };
    if analysis.knowledge != CommandKnowledge::Known || analysis.segments.len() != 1 {
        return false;
    }
    let mut tokens = command_words_for_simple_argv(&analysis.segments[0].argv).into_iter();
    matches!(
        (tokens.next(), tokens.next(), tokens.next(), tokens.next()),
        (Some("gws"), Some("schema"), Some(_), None)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_gws_schema_only_rejects_composed_shell_commands() {
        assert!(command_is_gws_schema_only(
            "sudo env GWS_CONFIG_DIR=/tmp/gws gws schema gmail.users.messages.list"
        ));
        assert!(!command_is_gws_schema_only(
            "gws schema gmail.users.messages.list | cat"
        ));
        assert!(!command_is_gws_schema_only(
            "gws schema gmail.users.messages.list; gws gmail users messages list"
        ));
        assert!(!command_is_gws_schema_only(
            "gws schema gmail.users.messages.list --unexpected"
        ));
    }
}

pub(crate) fn response_evidence_fingerprint(input: &ResponseTurnInput) -> String {
    let mut parts = Vec::new();
    for outcome in &input.tool_outcomes {
        parts.push(format!(
            "{}|{}|{}|{}",
            outcome.tool_name,
            outcome.outcome,
            outcome.attempted_command.as_deref().unwrap_or(""),
            outcome.failure_reason.as_deref().unwrap_or("")
        ));
        for metadata in &outcome.refs {
            parts.push(format!(
                "ref:{}:{}:{}",
                metadata.kind, metadata.byte_count, metadata.line_count
            ));
        }
    }
    if !input.extracted_evidence.is_empty() {
        let normalized_evidence: Vec<_> = input
            .extracted_evidence
            .iter()
            .cloned()
            .map(|mut record| {
                record.ref_id.clear();
                record
            })
            .collect();
        parts
            .push(serde_json::to_string(&normalized_evidence).unwrap_or_else(|_| "[]".to_string()));
    }
    if !input.trusted_effects.is_empty() {
        parts.push(
            serde_json::to_string(&input.trusted_effects).unwrap_or_else(|_| "[]".to_string()),
        );
    }
    parts.join("\n")
}

#[derive(Debug, serde::Deserialize)]
struct ResponseEvidenceBatchWire {
    #[serde(default)]
    records: Vec<ResponseEvidenceRecord>,
}

pub(crate) fn response_has_explicit_answer_candidate(input: &ResponseTurnInput) -> bool {
    input.extracted_evidence.iter().any(|record| {
        record
            .answer_candidate
            .as_ref()
            .map(|candidate| {
                candidate.support == "explicit_item" && !candidate.title.trim().is_empty()
            })
            .unwrap_or(false)
    })
}

pub(crate) fn response_has_trusted_effect(input: &ResponseTurnInput) -> bool {
    !input.trusted_effects.is_empty()
}

pub(crate) async fn build_response_evidence_records(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    trusted_user_message: &str,
    input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    summary_calls: &mut usize,
    summary_budget: usize,
) -> Vec<ResponseEvidenceRecord> {
    if *summary_calls >= summary_budget {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    let mut sequence = 0usize;
    for outcome in &input.tool_outcomes {
        let priority = evidence_ref_priority(outcome);
        for metadata in outcome
            .refs
            .iter()
            .filter(|metadata| metadata.byte_count > 0)
        {
            if !seen.insert(metadata.ref_id.clone()) {
                continue;
            }
            candidates.push(EvidenceRefCandidate {
                ref_id: metadata.ref_id.clone(),
                kind: metadata.kind.clone(),
                byte_count: metadata.byte_count,
                line_count: metadata.line_count,
                priority,
                sequence,
            });
            sequence = sequence.saturating_add(1);
        }
    }

    candidates.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| right.sequence.cmp(&left.sequence))
    });
    candidates.truncate(4);
    candidates.sort_by_key(|candidate| candidate.sequence);

    let mut refs = Vec::new();
    for candidate in candidates {
        let Some((content, _, _)) =
            crate::render_refs::resolve_ref_summary_input(&candidate.ref_id, render_refs).await
        else {
            continue;
        };
        refs.push(ResponseEvidenceBatchRef {
            ref_id: candidate.ref_id,
            kind: candidate.kind,
            byte_count: candidate.byte_count,
            line_count: candidate.line_count,
            content,
        });
    }

    if refs.is_empty() {
        return Vec::new();
    }

    let payload = to_json_value(
        ResponseEvidenceBatchPayload {
            task: "extract_response_evidence_batch",
            trusted_user_message,
            refs,
        },
        "response evidence batch payload",
    );
    let Some(raw) = summarize_with_ref_id_counted(
        summary_model,
        run_id,
        &format!("assistant-response-evidence:{}", run_id.0),
        &payload,
        summary_calls,
        summary_budget,
    )
    .await
    else {
        return Vec::new();
    };

    parse_response_evidence_batch(&raw)
}

fn parse_response_evidence_batch(raw: &str) -> Vec<ResponseEvidenceRecord> {
    let parsed: ResponseEvidenceBatchWire = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    parsed
        .records
        .into_iter()
        .filter_map(|record| {
            let summary = trimmed_non_empty(record.summary)?;
            Some(ResponseEvidenceRecord {
                ref_id: record.ref_id,
                summary,
                page_state: record.page_state.and_then(trimmed_non_empty),
                blockers: record
                    .blockers
                    .into_iter()
                    .filter_map(trimmed_non_empty)
                    .collect(),
                source_urls: record
                    .source_urls
                    .into_iter()
                    .filter_map(trimmed_non_empty)
                    .collect(),
                items: record
                    .items
                    .into_iter()
                    .filter_map(|item| {
                        let title = trimmed_non_empty(item.title)?;
                        Some(sieve_llm::ResponseEvidenceItem {
                            kind: trimmed_string(item.kind),
                            rank: item.rank,
                            title,
                            url: item.url.and_then(trimmed_non_empty),
                        })
                    })
                    .collect(),
                answer_candidate: record.answer_candidate.and_then(|candidate| {
                    let title = trimmed_non_empty(candidate.title)?;
                    Some(sieve_llm::ResponseAnswerCandidate {
                        target: trimmed_string(candidate.target),
                        item_kind: trimmed_string(candidate.item_kind),
                        title,
                        url: candidate.url.and_then(trimmed_non_empty),
                        support: trimmed_string(candidate.support),
                        rank: candidate.rank,
                    })
                }),
            })
        })
        .collect()
}

fn trimmed_string(value: String) -> String {
    value.trim().to_string()
}

fn trimmed_non_empty(value: String) -> Option<String> {
    let trimmed = trimmed_string(value);
    (!trimmed.is_empty()).then_some(trimmed)
}

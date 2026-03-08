use super::mainline::{count_newlines, mainline_artifact_kind_name};
use crate::render_refs::RenderRef;
use sieve_llm::{
    ResponseEvidenceRecord, ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput,
    SummaryModel, SummaryRequest,
};
use sieve_runtime::{PlannerRunResult, PlannerToolResult, RuntimeDisposition};
use sieve_types::{Integrity, InteractionModality, RunId, TrustedToolEffect};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub(crate) fn planner_allowed_tools_for_turn(
    configured_tools: &[String],
    has_known_value_refs: bool,
    automation_available: bool,
) -> Vec<String> {
    configured_tools
        .iter()
        .filter(|tool| {
            (has_known_value_refs || (tool.as_str() != "endorse" && tool.as_str() != "declassify"))
                && (automation_available || tool.as_str() != "automation")
        })
        .cloned()
        .collect()
}

pub(crate) fn build_response_turn_input(
    run_id: &RunId,
    trusted_user_message: &str,
    response_modality: InteractionModality,
    planner_result: &PlannerRunResult,
) -> (ResponseTurnInput, BTreeMap<String, RenderRef>) {
    let mut render_refs = BTreeMap::new();
    let mut tool_outcomes = Vec::with_capacity(planner_result.tool_results.len());
    let mut trusted_effects = Vec::new();
    for tool_result in &planner_result.tool_results {
        tool_outcomes.push(summarize_tool_result(tool_result, &mut render_refs));
        trusted_effects.extend(trusted_effects_for_tool_result(tool_result));
    }

    (
        ResponseTurnInput {
            run_id: run_id.clone(),
            trusted_user_message: trusted_user_message.to_string(),
            response_modality,
            planner_thoughts: planner_result.thoughts.clone(),
            tool_outcomes,
            trusted_effects,
            extracted_evidence: Vec::new(),
        },
        render_refs,
    )
}

pub(crate) fn requires_output_visibility(input: &ResponseTurnInput) -> bool {
    !non_empty_output_ref_ids(input).is_empty()
        && user_explicitly_requests_output_visibility(&input.trusted_user_message)
}

pub(crate) fn non_empty_output_ref_ids(input: &ResponseTurnInput) -> BTreeSet<String> {
    input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|ref_metadata| {
            output_ref_requires_visibility(&ref_metadata.kind) && ref_metadata.byte_count > 0
        })
        .map(|ref_metadata| ref_metadata.ref_id.clone())
        .collect()
}

pub(crate) fn response_has_visible_selected_output(
    input: &ResponseTurnInput,
    response: &sieve_llm::ResponseTurnOutput,
) -> bool {
    let output_ref_ids = non_empty_output_ref_ids(input);
    response.referenced_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id) && response.message.contains(&format!("[[ref:{ref_id}]]"))
    }) || response.summarized_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id)
            && response.message.contains(&format!("[[summary:{ref_id}]]"))
    })
}

pub(crate) fn format_integrity(integrity: Integrity) -> &'static str {
    match integrity {
        Integrity::Trusted => "trusted",
        Integrity::Untrusted => "untrusted",
    }
}

pub(crate) async fn summarize_with_ref_id_counted(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    ref_id: &str,
    payload: &serde_json::Value,
    summary_calls: &mut usize,
    budget_remaining: usize,
) -> Option<String> {
    if *summary_calls >= budget_remaining {
        return None;
    }
    *summary_calls = summary_calls.saturating_add(1);
    summarize_with_ref_id(summary_model, run_id, ref_id, payload).await
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

    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    for metadata in input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|metadata| metadata.byte_count > 0)
    {
        if refs.len() >= 4 || !seen.insert(metadata.ref_id.clone()) {
            continue;
        }
        let Some((content, _, _)) =
            crate::render_refs::resolve_ref_summary_input(&metadata.ref_id, render_refs).await
        else {
            continue;
        };
        refs.push(serde_json::json!({
            "ref_id": metadata.ref_id,
            "kind": metadata.kind,
            "byte_count": metadata.byte_count,
            "line_count": metadata.line_count,
            "content": content,
        }));
    }

    if refs.is_empty() {
        return Vec::new();
    }

    let payload = serde_json::json!({
        "task": "extract_response_evidence_batch",
        "trusted_user_message": trusted_user_message,
        "refs": refs,
    });
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
            let summary = record.summary.trim();
            if summary.is_empty() {
                return None;
            }
            Some(ResponseEvidenceRecord {
                ref_id: record.ref_id,
                summary: summary.to_string(),
                page_state: record.page_state.map(|value| value.trim().to_string()),
                blockers: record
                    .blockers
                    .into_iter()
                    .filter_map(|value| {
                        let trimmed = value.trim();
                        (!trimmed.is_empty()).then_some(trimmed.to_string())
                    })
                    .collect(),
                source_urls: record
                    .source_urls
                    .into_iter()
                    .filter_map(|value| {
                        let trimmed = value.trim();
                        (!trimmed.is_empty()).then_some(trimmed.to_string())
                    })
                    .collect(),
                items: record
                    .items
                    .into_iter()
                    .filter_map(|item| {
                        let title = item.title.trim();
                        if title.is_empty() {
                            return None;
                        }
                        Some(sieve_llm::ResponseEvidenceItem {
                            kind: item.kind.trim().to_string(),
                            rank: item.rank,
                            title: title.to_string(),
                            url: item
                                .url
                                .map(|value| value.trim().to_string())
                                .filter(|value| !value.is_empty()),
                        })
                    })
                    .collect(),
                answer_candidate: record.answer_candidate.and_then(|candidate| {
                    let title = candidate.title.trim();
                    if title.is_empty() {
                        return None;
                    }
                    Some(sieve_llm::ResponseAnswerCandidate {
                        target: candidate.target.trim().to_string(),
                        item_kind: candidate.item_kind.trim().to_string(),
                        title: title.to_string(),
                        url: candidate
                            .url
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty()),
                        support: candidate.support.trim().to_string(),
                        rank: candidate.rank,
                    })
                }),
            })
        })
        .collect()
}

fn user_explicitly_requests_output_visibility(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("output")
        || lower.contains("stdout")
        || lower.contains("stderr")
        || lower.contains("contents of")
        || lower.contains("content of")
        || lower.contains("show the result")
        || lower.contains("show me the result")
        || lower.contains("run exactly")
        || (lower.contains("what did") && lower.contains("return"))
}

fn output_ref_requires_visibility(kind: &str) -> bool {
    matches!(kind, "stdout" | "stderr")
}

fn summarize_tool_result(
    result: &PlannerToolResult,
    render_refs: &mut BTreeMap<String, RenderRef>,
) -> ResponseToolOutcome {
    match result {
        PlannerToolResult::Automation {
            request,
            message,
            effect: _,
            failure_reason,
        } => {
            let action = match request.action {
                sieve_types::AutomationAction::CronList => "listed cron jobs",
                sieve_types::AutomationAction::CronAdd => "scheduled cron job",
                sieve_types::AutomationAction::CronRemove => "removed cron job",
                sieve_types::AutomationAction::CronPause => "paused cron job",
                sieve_types::AutomationAction::CronResume => "resumed cron job",
            };
            ResponseToolOutcome {
                tool_name: "automation".to_string(),
                outcome: match message {
                    Some(message) => format!("automation {action}: {message}"),
                    None => format!("automation {action} failed"),
                },
                attempted_command: None,
                failure_reason: failure_reason.clone(),
                refs: Vec::new(),
            }
        }
        PlannerToolResult::Bash {
            disposition,
            command,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: format!("executed mainline (exit_code={:?})", report.exit_code),
                attempted_command: Some(command.clone()),
                failure_reason: None,
                refs: report
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        render_refs.insert(
                            artifact.ref_id.clone(),
                            RenderRef::Artifact {
                                path: PathBuf::from(&artifact.path),
                                byte_count: artifact.byte_count,
                                line_count: artifact.line_count,
                            },
                        );
                        ResponseRefMetadata {
                            ref_id: artifact.ref_id.clone(),
                            kind: mainline_artifact_kind_name(artifact.kind).to_string(),
                            byte_count: artifact.byte_count,
                            line_count: artifact.line_count,
                        }
                    })
                    .collect(),
            },
            RuntimeDisposition::ExecuteQuarantine(report) => {
                let trace_ref = format!("trace:{}", report.run_id.0);
                render_refs.insert(
                    trace_ref.clone(),
                    RenderRef::Literal {
                        value: report.trace_path.clone(),
                    },
                );
                ResponseToolOutcome {
                    tool_name: "bash".to_string(),
                    outcome: format!(
                        "executed in quarantine (exit_code={:?}, trace=[[ref:{}]])",
                        report.exit_code, trace_ref
                    ),
                    attempted_command: Some(command.clone()),
                    failure_reason: None,
                    refs: vec![ResponseRefMetadata {
                        ref_id: trace_ref,
                        kind: "trace_path".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    }],
                }
            }
            RuntimeDisposition::Denied { reason } => ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "denied".to_string(),
                attempted_command: Some(command.clone()),
                failure_reason: Some(reason.clone()),
                refs: Vec::new(),
            },
        },
        PlannerToolResult::Endorse {
            request,
            transition,
        } => {
            let value_ref_id = format!("value:{}", request.value_ref.0);
            render_refs.insert(
                value_ref_id.clone(),
                RenderRef::Literal {
                    value: request.value_ref.0.clone(),
                },
            );
            let outcome = match transition {
                Some(transition) => format!(
                    "endorse applied for [[ref:{}]] ({} -> {})",
                    value_ref_id,
                    format_integrity(transition.from_integrity),
                    format_integrity(transition.to_integrity),
                ),
                None => format!("endorse not applied for [[ref:{}]]", value_ref_id),
            };
            ResponseToolOutcome {
                tool_name: "endorse".to_string(),
                outcome,
                attempted_command: None,
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: value_ref_id,
                    kind: "value_ref".to_string(),
                    byte_count: 0,
                    line_count: 0,
                }],
            }
        }
        PlannerToolResult::Declassify {
            request,
            transition,
        } => {
            let value_ref_id = format!("value:{}", request.value_ref.0);
            let sink_ref_id = format!("sink:{}", request.sink.0);
            render_refs.insert(
                value_ref_id.clone(),
                RenderRef::Literal {
                    value: request.value_ref.0.clone(),
                },
            );
            render_refs.insert(
                sink_ref_id.clone(),
                RenderRef::Literal {
                    value: request.sink.0.clone(),
                },
            );
            let outcome = match transition {
                Some(transition) => format!(
                    "declassify applied for [[ref:{}]] -> [[ref:{}]] (already_allowed={})",
                    value_ref_id, sink_ref_id, transition.sink_was_already_allowed
                ),
                None => format!(
                    "declassify not applied for [[ref:{}]] -> [[ref:{}]]",
                    value_ref_id, sink_ref_id
                ),
            };
            ResponseToolOutcome {
                tool_name: "declassify".to_string(),
                outcome,
                attempted_command: None,
                failure_reason: None,
                refs: vec![
                    ResponseRefMetadata {
                        ref_id: value_ref_id,
                        kind: "value_ref".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    },
                    ResponseRefMetadata {
                        ref_id: sink_ref_id,
                        kind: "sink".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    },
                ],
            }
        }
    }
}

fn trusted_effects_for_tool_result(result: &PlannerToolResult) -> Vec<TrustedToolEffect> {
    match result {
        PlannerToolResult::Automation {
            effect: Some(effect),
            ..
        } => vec![effect.clone()],
        _ => Vec::new(),
    }
}

async fn summarize_with_ref_id(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    ref_id: &str,
    payload: &serde_json::Value,
) -> Option<String> {
    let content = payload.to_string();
    let request = SummaryRequest {
        run_id: run_id.clone(),
        ref_id: ref_id.to_string(),
        byte_count: content.len() as u64,
        line_count: count_newlines(content.as_bytes()),
        content,
    };
    match summary_model.summarize_ref(request).await {
        Ok(summary) => {
            let trimmed = summary.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(_) => None,
    }
}

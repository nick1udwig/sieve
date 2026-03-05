use super::mainline::{count_newlines, mainline_artifact_kind_name};
use crate::render_refs::RenderRef;
use sieve_llm::{
    ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput, SummaryModel, SummaryRequest,
};
use sieve_runtime::{PlannerRunResult, PlannerToolResult, RuntimeDisposition};
use sieve_types::{Integrity, InteractionModality, RunId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub(crate) fn planner_allowed_tools_for_turn(
    configured_tools: &[String],
    has_known_value_refs: bool,
) -> Vec<String> {
    if has_known_value_refs {
        return configured_tools.to_vec();
    }

    configured_tools
        .iter()
        .filter(|tool| tool.as_str() != "endorse" && tool.as_str() != "declassify")
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
    for tool_result in &planner_result.tool_results {
        tool_outcomes.push(summarize_tool_result(tool_result, &mut render_refs));
    }

    (
        ResponseTurnInput {
            run_id: run_id.clone(),
            trusted_user_message: trusted_user_message.to_string(),
            response_modality,
            planner_thoughts: planner_result.thoughts.clone(),
            tool_outcomes,
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

pub(super) fn response_evidence_fingerprint(input: &ResponseTurnInput) -> String {
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
                "ref:{}:{}:{}:{}",
                metadata.ref_id, metadata.kind, metadata.byte_count, metadata.line_count
            ));
        }
    }
    parts.join("\n")
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

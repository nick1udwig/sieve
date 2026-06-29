use super::super::mainline::mainline_artifact_kind_name;
use crate::render_refs::RenderRef;
use sieve_llm::{ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput};
use sieve_runtime::{PlannerRunResult, PlannerToolResult, RuntimeDisposition};
use sieve_tool_contracts::planner_exposed_tool_names;
use sieve_types::{Integrity, InteractionModality, RunId};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(crate) fn planner_allowed_tools_for_turn(
    configured_tools: &[String],
    has_known_value_refs: bool,
    automation_available: bool,
    codex_available: bool,
) -> Vec<String> {
    planner_exposed_tool_names(
        configured_tools,
        has_known_value_refs,
        automation_available,
        codex_available,
    )
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
        if let PlannerToolResult::Automation {
            effect: Some(effect),
            ..
        } = tool_result
        {
            trusted_effects.push(effect.clone());
        }
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

pub(crate) fn format_integrity(integrity: Integrity) -> &'static str {
    match integrity {
        Integrity::Trusted => "trusted",
        Integrity::Untrusted => "untrusted",
    }
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
        PlannerToolResult::CodexExec {
            request,
            result,
            failure_reason,
        } => {
            let outcome = match result {
                Some(result) => format!(
                    "codex exec exit {} (stdout {}B, stderr {}B)",
                    result.exit_code,
                    result.stdout.len(),
                    result.stderr.len()
                ),
                None => "codex exec failed".to_string(),
            };
            ResponseToolOutcome {
                tool_name: "codex_exec".to_string(),
                outcome,
                attempted_command: Some(request.command.join(" ")),
                failure_reason: failure_reason.clone(),
                refs: Vec::new(),
            }
        }
        PlannerToolResult::CodexSession {
            request,
            result,
            failure_reason,
        } => {
            let outcome = match result {
                Some(result) => format!(
                    "codex session {} [{}{}]: {}",
                    result.status.as_str(),
                    result.session_name,
                    result
                        .session_id
                        .as_ref()
                        .map(|value| format!(":{value}"))
                        .unwrap_or_default(),
                    result.summary
                ),
                None => format!(
                    "codex session failed{}",
                    request
                        .session_id
                        .as_ref()
                        .map(|value| format!(" ({value})"))
                        .unwrap_or_default()
                ),
            };
            ResponseToolOutcome {
                tool_name: "codex_session".to_string(),
                outcome,
                attempted_command: None,
                failure_reason: failure_reason.clone(),
                refs: Vec::new(),
            }
        }
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

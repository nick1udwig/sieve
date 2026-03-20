use crate::logging::{ConversationHistoryEntry, ConversationRole};
use crate::planner_products::PlannerIntermediateProductSummary;
use crate::planner_progress::summarize_redacted_tool_result;
use serde::Serialize;
use sieve_runtime::PlannerToolResult;
use sieve_types::{
    PlannerConversationMessage, PlannerConversationMessageKind, PlannerConversationRole,
    PlannerGuidanceFrame, PlannerGuidanceSignal,
};

#[derive(Serialize)]
struct PlannerTraceActionsPayload<'a> {
    step_index: usize,
    tool_calls: Vec<PlannerToolCallPayload<'a>>,
}

#[derive(Serialize)]
struct PlannerTraceObservationPayload {
    step_index: usize,
    tool_results: Vec<serde_json::Value>,
    intermediate_products: Vec<PlannerIntermediateProductSummary>,
    guidance: PlannerTraceGuidancePayload,
}

#[derive(Serialize)]
struct PlannerTraceGuidancePayload {
    code: u16,
    signal_name: Option<&'static str>,
    confidence_bps: u16,
    source_hit_index: Option<u16>,
    evidence_ref_index: Option<u16>,
}

#[derive(Serialize)]
struct PlannerToolCallPayload<'a> {
    tool: &'static str,
    args: PlannerToolArgsPayload<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum PlannerToolArgsPayload<'a> {
    Automation(&'a sieve_types::AutomationRequest),
    Bash(BashArgs<'a>),
    CodexExec(&'a sieve_types::CodexExecRequest),
    CodexSession(&'a sieve_types::CodexSessionRequest),
    Endorse(&'a sieve_types::EndorseRequest),
    Declassify(&'a sieve_types::DeclassifyRequest),
}

#[derive(Serialize)]
struct BashArgs<'a> {
    cmd: &'a str,
}

fn to_json_string<T: Serialize>(value: &T, context: &str) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|err| panic!("failed to serialize {context}: {err}"))
}

pub(crate) fn build_planner_conversation(
    history_messages: &[PlannerConversationMessage],
    policy_feedback: Option<&str>,
    memory_feedback: Option<&str>,
    planner_trace: &[PlannerConversationMessage],
) -> Vec<PlannerConversationMessage> {
    let mut conversation = Vec::new();
    if let Some(feedback) = policy_feedback.filter(|value| !value.trim().is_empty()) {
        conversation.push(redacted_user_message(format!(
            "TRUSTED_POLICY_FEEDBACK\n{feedback}"
        )));
    }
    if let Some(feedback) = memory_feedback.filter(|value| !value.trim().is_empty()) {
        conversation.push(redacted_user_message(format!(
            "TRUSTED_MEMORY_FEEDBACK\n{feedback}"
        )));
    }
    conversation.extend(history_messages.iter().cloned());
    conversation.extend(planner_trace.iter().cloned());
    conversation
}

pub(crate) fn planner_step_trace_messages(
    step_index: usize,
    step_results: &[PlannerToolResult],
    guidance: &PlannerGuidanceFrame,
    intermediate_products: &[PlannerIntermediateProductSummary],
) -> Vec<PlannerConversationMessage> {
    if step_results.is_empty() {
        return Vec::new();
    }

    vec![
        PlannerConversationMessage {
            role: PlannerConversationRole::Assistant,
            kind: PlannerConversationMessageKind::FullText,
            content: format!(
                "TRUSTED_PLANNER_ACTIONS\n{}",
                to_json_string(
                    &PlannerTraceActionsPayload {
                        step_index,
                        tool_calls: step_results.iter().map(planner_tool_call_payload).collect(),
                    },
                    "planner trace actions payload",
                )
            ),
        },
        redacted_user_message(format!(
            "TRUSTED_REDACTED_STEP_OBSERVATION\n{}",
            to_json_string(
                &PlannerTraceObservationPayload {
                    step_index,
                    tool_results: step_results
                        .iter()
                        .map(summarize_redacted_tool_result)
                        .collect(),
                    intermediate_products: intermediate_products.to_vec(),
                    guidance: PlannerTraceGuidancePayload {
                        code: guidance.code,
                        signal_name: PlannerGuidanceSignal::try_from(guidance.code)
                            .ok()
                            .map(PlannerGuidanceSignal::name),
                        confidence_bps: guidance.confidence_bps,
                        source_hit_index: guidance.source_hit_index,
                        evidence_ref_index: guidance.evidence_ref_index,
                    },
                },
                "planner trace observation payload",
            )
        )),
    ]
}

pub(crate) fn history_entry_to_planner_message(
    entry: &ConversationHistoryEntry,
) -> PlannerConversationMessage {
    PlannerConversationMessage {
        role: match entry.role {
            ConversationRole::User => PlannerConversationRole::User,
            ConversationRole::Assistant => PlannerConversationRole::Assistant,
        },
        kind: PlannerConversationMessageKind::FullText,
        content: entry.message.clone(),
    }
}

pub(crate) fn history_entries_to_planner_messages(
    history: &[ConversationHistoryEntry],
) -> Vec<PlannerConversationMessage> {
    history
        .iter()
        .map(history_entry_to_planner_message)
        .collect()
}

fn redacted_user_message(content: String) -> PlannerConversationMessage {
    PlannerConversationMessage {
        role: PlannerConversationRole::User,
        kind: PlannerConversationMessageKind::RedactedInfo,
        content,
    }
}

fn planner_tool_call_payload(result: &PlannerToolResult) -> PlannerToolCallPayload<'_> {
    match result {
        PlannerToolResult::Automation { request, .. } => PlannerToolCallPayload {
            tool: "automation",
            args: PlannerToolArgsPayload::Automation(request),
        },
        PlannerToolResult::Bash { command, .. } => PlannerToolCallPayload {
            tool: "bash",
            args: PlannerToolArgsPayload::Bash(BashArgs { cmd: command }),
        },
        PlannerToolResult::CodexExec { request, .. } => PlannerToolCallPayload {
            tool: "codex_exec",
            args: PlannerToolArgsPayload::CodexExec(request),
        },
        PlannerToolResult::CodexSession { request, .. } => PlannerToolCallPayload {
            tool: "codex_session",
            args: PlannerToolArgsPayload::CodexSession(request),
        },
        PlannerToolResult::Endorse { request, .. } => PlannerToolCallPayload {
            tool: "endorse",
            args: PlannerToolArgsPayload::Endorse(request),
        },
        PlannerToolResult::Declassify { request, .. } => PlannerToolCallPayload {
            tool: "declassify",
            args: PlannerToolArgsPayload::Declassify(request),
        },
    }
}

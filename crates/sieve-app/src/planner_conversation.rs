use crate::logging::{ConversationHistoryEntry, ConversationRole};
use crate::planner_progress::summarize_redacted_tool_result;
use crate::working_state::{format_open_loop_context_message, StoredOpenLoop};
use serde_json::json;
use sieve_runtime::PlannerToolResult;
use sieve_types::{
    PlannerConversationMessage, PlannerConversationMessageKind, PlannerConversationRole,
    PlannerGuidanceFrame, PlannerGuidanceSignal,
};

pub(crate) fn build_planner_conversation(
    history: &[ConversationHistoryEntry],
    policy_feedback: Option<&str>,
    memory_feedback: Option<&str>,
    open_loop: Option<&StoredOpenLoop>,
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
    if let Some(loop_record) = open_loop {
        conversation.push(redacted_user_message(format_open_loop_context_message(
            loop_record,
        )));
    }
    conversation.extend(history.iter().map(history_entry_to_planner_message));
    conversation.extend(planner_trace.iter().cloned());
    conversation
}

pub(crate) fn planner_step_trace_messages(
    step_index: usize,
    step_results: &[PlannerToolResult],
    guidance: &PlannerGuidanceFrame,
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
                json!({
                    "step_index": step_index,
                    "tool_calls": step_results.iter().map(planner_tool_call_json).collect::<Vec<_>>(),
                })
            ),
        },
        redacted_user_message(format!(
            "TRUSTED_REDACTED_STEP_OBSERVATION\n{}",
            json!({
                "step_index": step_index,
                "tool_results": step_results
                    .iter()
                    .map(summarize_redacted_tool_result)
                    .collect::<Vec<_>>(),
                "guidance": {
                    "code": guidance.code,
                    "signal_name": PlannerGuidanceSignal::try_from(guidance.code)
                        .ok()
                        .map(PlannerGuidanceSignal::name),
                    "confidence_bps": guidance.confidence_bps,
                    "source_hit_index": guidance.source_hit_index,
                    "evidence_ref_index": guidance.evidence_ref_index,
                }
            })
        )),
    ]
}

fn history_entry_to_planner_message(
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

fn redacted_user_message(content: String) -> PlannerConversationMessage {
    PlannerConversationMessage {
        role: PlannerConversationRole::User,
        kind: PlannerConversationMessageKind::RedactedInfo,
        content,
    }
}

fn planner_tool_call_json(result: &PlannerToolResult) -> serde_json::Value {
    match result {
        PlannerToolResult::Automation { request, .. } => {
            json!({"tool":"automation","args":request})
        }
        PlannerToolResult::Bash { command, .. } => {
            json!({"tool":"bash","args":{"cmd":command}})
        }
        PlannerToolResult::CodexExec { request, .. } => {
            json!({"tool":"codex_exec","args":request})
        }
        PlannerToolResult::CodexSession { request, .. } => {
            json!({"tool":"codex_session","args":request})
        }
        PlannerToolResult::Endorse { request, .. } => {
            json!({"tool":"endorse","args":request})
        }
        PlannerToolResult::Declassify { request, .. } => {
            json!({"tool":"declassify","args":request})
        }
    }
}

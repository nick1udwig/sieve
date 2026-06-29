use super::payloads::HeartbeatFinalizePayload;
use super::GeneratedAssistantMessage;
use crate::automation::{parse_heartbeat_planner_action, HeartbeatPlannerAction};
use crate::json_util::to_json_value;
use crate::logging::{append_turn_controller_event, FanoutRuntimeEventLog};
use sieve_types::RunId;

pub(super) struct HeartbeatFinalizeRequest<'a> {
    pub(super) event_log: &'a FanoutRuntimeEventLog,
    pub(super) run_id: &'a RunId,
    pub(super) thoughts: Option<&'a str>,
    pub(super) planner_steps_taken: usize,
    pub(super) planner_step_limit: usize,
    pub(super) planner_step_hard_limit: usize,
    pub(super) compose_followup_cycles: usize,
}

fn finalize_heartbeat_message(thoughts: Option<&str>) -> GeneratedAssistantMessage {
    match parse_heartbeat_planner_action(thoughts.unwrap_or_default()) {
        Some(HeartbeatPlannerAction::Noop) | None => GeneratedAssistantMessage::SuppressHeartbeat,
        Some(HeartbeatPlannerAction::Deliver { message }) => {
            let trimmed = message.trim();
            if trimmed.is_empty() {
                GeneratedAssistantMessage::SuppressHeartbeat
            } else {
                GeneratedAssistantMessage::Deliver {
                    message: trimmed.to_string(),
                    reply_to_session_id: None,
                }
            }
        }
    }
}

pub(super) async fn finalize_heartbeat_turn(
    request: HeartbeatFinalizeRequest<'_>,
) -> GeneratedAssistantMessage {
    let heartbeat_message = finalize_heartbeat_message(request.thoughts);
    append_turn_controller_event(
        request.event_log,
        request.run_id,
        "heartbeat_finalize",
        to_json_value(
            HeartbeatFinalizePayload {
                planner_steps_taken: request.planner_steps_taken,
                planner_step_limit: request.planner_step_limit,
                planner_step_hard_limit: request.planner_step_hard_limit,
                compose_followup_cycles: request.compose_followup_cycles,
                delivered: matches!(heartbeat_message, GeneratedAssistantMessage::Deliver { .. }),
            },
            "heartbeat finalize payload",
        ),
    )
    .await;
    heartbeat_message
}

use super::payloads::ComposeDecisionPayload;
use crate::compose::ComposeAssistantOutcome;
use crate::json_util::to_json_value;
use crate::logging::{append_turn_controller_event, FanoutRuntimeEventLog};
use sieve_types::{PlannerGuidanceFrame, PlannerGuidanceSignal, RunId};
use std::collections::BTreeSet;

pub(super) struct ComposeContinueRequest<'a> {
    pub(super) event_log: &'a FanoutRuntimeEventLog,
    pub(super) run_id: &'a RunId,
    pub(super) composed: &'a ComposeAssistantOutcome,
    pub(super) signal: PlannerGuidanceSignal,
    pub(super) evidence_fingerprint: &'a str,
    pub(super) compose_continue_fingerprints: &'a mut BTreeSet<String>,
    pub(super) planner_steps_taken: usize,
    pub(super) planner_step_limit: &'a mut usize,
    pub(super) planner_step_hard_limit: usize,
    pub(super) compose_followup_cycles: &'a mut usize,
    pub(super) max_compose_followup_cycles: usize,
    pub(super) summary_calls_used: usize,
    pub(super) summary_call_budget: usize,
}

pub(super) async fn handle_compose_continue(request: ComposeContinueRequest<'_>) -> bool {
    let mut can_continue = request.planner_steps_taken < request.planner_step_hard_limit
        && *request.compose_followup_cycles < request.max_compose_followup_cycles;
    let mut continue_block_reason: Option<&str> = None;
    if can_continue && request.summary_calls_used >= request.summary_call_budget {
        can_continue = false;
        continue_block_reason = Some("summary_budget_exhausted");
    }
    if can_continue
        && !request
            .compose_continue_fingerprints
            .insert(request.evidence_fingerprint.to_string())
    {
        can_continue = false;
        continue_block_reason = Some("no_new_evidence");
    }

    append_turn_controller_event(
        request.event_log,
        request.run_id,
        "compose_decision",
        to_json_value(
            ComposeDecisionPayload {
                planner_decision_code: request.signal.code(),
                quality_gate_len: request
                    .composed
                    .quality_gate
                    .as_deref()
                    .map(str::len)
                    .unwrap_or(0),
                planner_steps_taken: request.planner_steps_taken,
                planner_step_limit: *request.planner_step_limit,
                planner_step_hard_limit: request.planner_step_hard_limit,
                compose_followup_cycles: *request.compose_followup_cycles,
                should_continue: can_continue,
                continue_block_reason,
                summary_calls_used: request.summary_calls_used,
                summary_call_budget: request.summary_call_budget,
            },
            "compose decision payload",
        ),
    )
    .await;

    if can_continue {
        *request.compose_followup_cycles = (*request.compose_followup_cycles).saturating_add(1);
        *request.planner_step_limit = (*request.planner_step_limit)
            .saturating_add(1)
            .min(request.planner_step_hard_limit.max(1));
    }
    can_continue
}

pub(super) fn continue_guidance_frame(signal: PlannerGuidanceSignal) -> PlannerGuidanceFrame {
    PlannerGuidanceFrame {
        code: signal.code(),
        confidence_bps: 9_000,
        source_hit_index: None,
        evidence_ref_index: None,
    }
}

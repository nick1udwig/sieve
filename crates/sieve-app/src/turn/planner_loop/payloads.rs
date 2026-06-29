use serde::Serialize;

#[derive(Serialize)]
pub(super) struct PlannerRepeatGuardPayload {
    pub(super) step_number: usize,
    pub(super) planner_steps_taken: usize,
    pub(super) reason: &'static str,
    #[serde(rename = "continue")]
    pub(super) should_continue: bool,
    pub(super) next_signal_code: u16,
}

#[derive(Serialize)]
pub(super) struct StepCounterPayload {
    pub(super) step_number: usize,
    pub(super) planner_steps_taken: usize,
}

#[derive(Serialize)]
pub(super) struct PlannerGuidancePayload {
    pub(super) step_number: usize,
    pub(super) signal_code: u16,
    pub(super) effective_signal_code: u16,
    pub(super) override_reason: Option<String>,
    #[serde(rename = "continue")]
    pub(super) should_continue: bool,
    pub(super) step_tool_count: usize,
    pub(super) planner_steps_taken: usize,
    pub(super) planner_step_limit: usize,
    pub(super) planner_step_hard_limit: usize,
    pub(super) auto_extended_limit: bool,
    pub(super) consecutive_empty_steps: usize,
}

#[derive(Serialize)]
pub(super) struct HeartbeatFinalizePayload {
    pub(super) planner_steps_taken: usize,
    pub(super) planner_step_limit: usize,
    pub(super) planner_step_hard_limit: usize,
    pub(super) compose_followup_cycles: usize,
    pub(super) delivered: bool,
}

#[derive(Serialize)]
pub(super) struct TurnFinalizePayload {
    pub(super) planner_steps_taken: usize,
    pub(super) planner_step_limit: usize,
    pub(super) planner_step_hard_limit: usize,
    pub(super) compose_followup_cycles: usize,
    pub(super) quality_gate_len: usize,
    pub(super) summary_calls_used: usize,
    pub(super) summary_call_budget: usize,
}

#[derive(Serialize)]
pub(super) struct ComposeDecisionPayload {
    pub(super) planner_decision_code: u16,
    pub(super) quality_gate_len: usize,
    pub(super) planner_steps_taken: usize,
    pub(super) planner_step_limit: usize,
    pub(super) planner_step_hard_limit: usize,
    pub(super) compose_followup_cycles: usize,
    #[serde(rename = "continue")]
    pub(super) should_continue: bool,
    pub(super) continue_block_reason: Option<&'static str>,
    pub(super) summary_calls_used: usize,
    pub(super) summary_call_budget: usize,
}

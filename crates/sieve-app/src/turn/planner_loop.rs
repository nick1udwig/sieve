use super::response_refs::{
    build_response_evidence_records, build_response_turn_input, non_empty_output_ref_ids,
    requires_output_visibility, response_evidence_fingerprint,
    response_has_visible_selected_output,
};
use crate::compose::{compose_assistant_message, ComposeAssistantOutcome, ComposePlannerDecision};
use crate::config::{persist_runtime_approval_allowances, AppConfig};
use crate::logging::{
    append_turn_controller_event, now_ms, ConversationLogRecord, ConversationRole,
    FanoutRuntimeEventLog,
};
use crate::planner_feedback::{planner_memory_feedback, planner_policy_feedback};
use crate::planner_progress::{
    build_guidance_prompt, guidance_continue_decision, has_repeated_bash_outcome,
    progress_contract_override_signal,
};
use crate::render_refs::render_assistant_message;
use crate::response_style::strip_unexpanded_render_tokens;
use sieve_llm::{GuidanceModel, ResponseModel, SummaryModel};
use sieve_runtime::{
    EventLogError, PlannerRunRequest, PlannerRunResult, RuntimeEventLog, RuntimeOrchestrator,
};
use sieve_types::{
    AssistantMessageEvent, InteractionModality, PlannerGuidanceFrame, PlannerGuidanceInput,
    PlannerGuidanceSignal, RunId, RuntimeEvent,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn emit_assistant_error_message(
    event_log: &FanoutRuntimeEventLog,
    run_id: &RunId,
    error_message: String,
) -> Result<(), EventLogError> {
    event_log
        .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
            schema_version: 1,
            run_id: run_id.clone(),
            message: error_message.clone(),
            created_at_ms: now_ms(),
        }))
        .await?;
    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            error_message,
            now_ms(),
        ))
        .await
}

pub(super) async fn generate_assistant_message(
    runtime: &RuntimeOrchestrator,
    guidance_model: &dyn GuidanceModel,
    response_model: &dyn ResponseModel,
    summary_model: &dyn SummaryModel,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_id: &RunId,
    trusted_user_message: &str,
    response_modality: InteractionModality,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut aggregated_result = PlannerRunResult {
        thoughts: None,
        tool_results: Vec::new(),
    };
    let mut planner_guidance: Option<PlannerGuidanceFrame> = None;
    let mut consecutive_empty_steps = 0usize;
    let mut planner_steps_taken = 0usize;
    let mut compose_followup_cycles = 0usize;
    let mut summary_calls_used = 0usize;
    let mut compose_continue_fingerprints = BTreeSet::new();
    let mut compose_evidence_cache = BTreeMap::new();
    let max_compose_followup_cycles = cfg.max_planner_steps.max(1);
    let planner_step_hard_limit = cfg
        .max_planner_steps
        .saturating_add(max_compose_followup_cycles);
    let mut planner_step_limit = cfg.max_planner_steps.max(1);
    let planner_user_message = trusted_user_message.to_string();

    loop {
        while planner_steps_taken < planner_step_limit {
            let step_number = planner_steps_taken + 1;
            let policy_feedback = planner_policy_feedback(&aggregated_result.tool_results);
            let memory_feedback = planner_memory_feedback(&aggregated_result.tool_results).await;
            let planner_turn_user_message = match (policy_feedback, memory_feedback) {
                (Some(policy), Some(memory)) => {
                    format!("{planner_user_message}\n\n{policy}\n\n{memory}")
                }
                (Some(policy), None) => format!("{planner_user_message}\n\n{policy}"),
                (None, Some(memory)) => format!("{planner_user_message}\n\n{memory}"),
                (None, None) => planner_user_message.clone(),
            };
            let has_known_value_refs = runtime.has_known_value_refs()?;
            let allowed_tools_for_turn = super::planner_allowed_tools_for_turn(
                &cfg.allowed_tools,
                has_known_value_refs,
                runtime.has_automation_tool(),
            );
            let browser_sessions = runtime.planner_browser_sessions()?;
            let step_result = match runtime
                .orchestrate_planner_turn(PlannerRunRequest {
                    run_id: run_id.clone(),
                    cwd: cfg.runtime_cwd.clone(),
                    user_message: planner_turn_user_message,
                    allowed_tools: allowed_tools_for_turn,
                    allowed_net_connect_scopes: cfg.allowed_net_connect_scopes.clone(),
                    browser_sessions,
                    previous_events: event_log.snapshot(),
                    guidance: planner_guidance.clone(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: cfg.unknown_mode,
                    uncertain_mode: cfg.uncertain_mode,
                })
                .await
            {
                Ok(result) => result,
                Err(err) => {
                    if let Err(log_err) =
                        emit_assistant_error_message(event_log, run_id, format!("error: {err}"))
                            .await
                    {
                        eprintln!("failed to append assistant error conversation log: {log_err}");
                    }
                    return Err(err.into());
                }
            };

            planner_steps_taken = planner_steps_taken.saturating_add(1);
            let step_tool_count = step_result.tool_results.len();
            if step_tool_count == 0 {
                consecutive_empty_steps = consecutive_empty_steps.saturating_add(1);
            } else {
                consecutive_empty_steps = 0;
            }
            if let Some(thoughts) = step_result.thoughts.clone() {
                aggregated_result.thoughts = Some(thoughts);
            }
            let step_results = step_result.tool_results;
            aggregated_result.tool_results.extend(step_results.clone());
            if let Err(err) = persist_runtime_approval_allowances(runtime, &cfg.sieve_home) {
                eprintln!(
                    "failed to persist approval allowances for {}: {}",
                    run_id.0, err
                );
            }

            if has_repeated_bash_outcome(&aggregated_result.tool_results) {
                let can_retry =
                    planner_steps_taken < planner_step_limit && consecutive_empty_steps < 2;
                append_turn_controller_event(
                    event_log,
                    run_id,
                    "planner_repeat_guard",
                    serde_json::json!({
                        "step_number": step_number,
                        "planner_steps_taken": planner_steps_taken,
                        "reason": "detected repeated bash command/result; forcing action-change guidance",
                        "continue": can_retry,
                        "next_signal_code": PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                    }),
                )
                .await;
                if can_retry {
                    planner_guidance = Some(PlannerGuidanceFrame {
                        code: PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                        confidence_bps: 9_000,
                        source_hit_index: None,
                        evidence_ref_index: None,
                    });
                    continue;
                }
                break;
            }

            let guidance_prompt = build_guidance_prompt(
                trusted_user_message,
                step_number,
                cfg.max_planner_steps,
                &step_results,
                &aggregated_result.tool_results,
            );
            let guidance_output = match guidance_model
                .classify_guidance(PlannerGuidanceInput {
                    run_id: run_id.clone(),
                    prompt: guidance_prompt,
                })
                .await
            {
                Ok(output) => output,
                Err(err) => {
                    eprintln!(
                        "guidance model failed for {} at step {}: {}",
                        run_id.0, step_number, err
                    );
                    append_turn_controller_event(
                        event_log,
                        run_id,
                        "planner_guidance_error",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                        }),
                    )
                    .await;
                    break;
                }
            };
            let signal = match guidance_output.guidance.signal() {
                Ok(signal) => signal,
                Err(err) => {
                    eprintln!(
                        "invalid guidance signal for {} at step {}: {}",
                        run_id.0, step_number, err
                    );
                    append_turn_controller_event(
                        event_log,
                        run_id,
                        "planner_guidance_invalid",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                        }),
                    )
                    .await;
                    break;
                }
            };
            let override_applied = progress_contract_override_signal(
                trusted_user_message,
                signal,
                &aggregated_result.tool_results,
            );
            let effective_signal = override_applied
                .map(|(override_signal, _)| override_signal)
                .unwrap_or(signal);
            let (should_continue, next_step_limit, auto_extended_limit) =
                guidance_continue_decision(
                    effective_signal,
                    consecutive_empty_steps,
                    planner_steps_taken,
                    planner_step_limit,
                    planner_step_hard_limit,
                );
            planner_step_limit = next_step_limit;
            append_turn_controller_event(
                event_log,
                run_id,
                "planner_guidance",
                serde_json::json!({
                    "step_number": step_number,
                    "signal_code": signal.code(),
                    "effective_signal_code": effective_signal.code(),
                    "override_reason": override_applied.map(|(_, reason)| reason),
                    "continue": should_continue,
                    "step_tool_count": step_tool_count,
                    "planner_steps_taken": planner_steps_taken,
                    "planner_step_limit": planner_step_limit,
                    "planner_step_hard_limit": planner_step_hard_limit,
                    "auto_extended_limit": auto_extended_limit,
                    "consecutive_empty_steps": consecutive_empty_steps,
                }),
            )
            .await;
            let mut guidance_frame = guidance_output.guidance;
            guidance_frame.code = effective_signal.code();
            planner_guidance = Some(guidance_frame);
            if !should_continue {
                break;
            }
        }

        let (response_input, render_refs) = build_response_turn_input(
            run_id,
            trusted_user_message,
            response_modality,
            &aggregated_result,
        );
        let mut response_input = response_input;
        response_input.extracted_evidence = build_response_evidence_records(
            summary_model,
            run_id,
            trusted_user_message,
            &response_input,
            &render_refs,
            &mut summary_calls_used,
            cfg.max_summary_calls_per_turn,
        )
        .await;
        let mut response_output = match response_model
            .write_turn_response(response_input.clone())
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if let Err(log_err) =
                    emit_assistant_error_message(event_log, run_id, format!("error: {err}")).await
                {
                    eprintln!("failed to append assistant error conversation log: {log_err}");
                }
                return Err(err.into());
            }
        };

        if requires_output_visibility(&response_input)
            && !response_has_visible_selected_output(&response_input, &response_output)
        {
            let diagnostics = "Non-empty output refs exist (stdout/stderr). Include at least one output token directly in `message` using [[ref:<id>]] or [[summary:<id>]], and list the same id in referenced_ref_ids or summarized_ref_ids.";
            response_input.planner_thoughts = Some(match response_input.planner_thoughts.take() {
                Some(existing) if !existing.trim().is_empty() => {
                    format!("{existing}\n{diagnostics}")
                }
                _ => diagnostics.to_string(),
            });

            response_output = match response_model
                .write_turn_response(response_input.clone())
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    if let Err(log_err) =
                        emit_assistant_error_message(event_log, run_id, format!("error: {err}"))
                            .await
                    {
                        eprintln!("failed to append assistant error conversation log: {log_err}");
                    }
                    return Err(err.into());
                }
            };

            if !response_has_visible_selected_output(&response_input, &response_output) {
                if let Some(fallback_ref_id) =
                    non_empty_output_ref_ids(&response_input).into_iter().next()
                {
                    response_output
                        .summarized_ref_ids
                        .insert(fallback_ref_id.clone());
                    let token = format!("[[summary:{fallback_ref_id}]]");
                    if !response_output.message.contains(&token) {
                        let base = response_output.message.trim();
                        response_output.message = if base.is_empty() {
                            token
                        } else {
                            format!("{base}\n{token}")
                        };
                    }
                }
            }
        }

        let output_visibility_required = requires_output_visibility(&response_input);
        let evidence_fingerprint = response_evidence_fingerprint(&response_input);
        let draft_message = if output_visibility_required {
            render_assistant_message(
                &response_output.message,
                &response_output.referenced_ref_ids,
                &response_output.summarized_ref_ids,
                &render_refs,
                summary_model,
                run_id,
            )
            .await
        } else {
            let stripped = strip_unexpanded_render_tokens(&response_output.message);
            if stripped.trim().is_empty() {
                render_assistant_message(
                    &response_output.message,
                    &response_output.referenced_ref_ids,
                    &response_output.summarized_ref_ids,
                    &render_refs,
                    summary_model,
                    run_id,
                )
                .await
            } else {
                stripped
            }
        };
        let zero_tool_no_action_needed = aggregated_result.tool_results.is_empty()
            && matches!(
                planner_guidance
                    .as_ref()
                    .and_then(|guidance| guidance.signal().ok()),
                Some(PlannerGuidanceSignal::FinalNoToolActionNeeded)
            );
        if zero_tool_no_action_needed {
            append_turn_controller_event(
                event_log,
                run_id,
                "turn_finalize",
                serde_json::json!({
                    "planner_steps_taken": planner_steps_taken,
                    "planner_step_limit": planner_step_limit,
                    "planner_step_hard_limit": planner_step_hard_limit,
                    "compose_followup_cycles": compose_followup_cycles,
                    "quality_gate_len": 0,
                    "summary_calls_used": summary_calls_used,
                    "summary_call_budget": cfg.max_summary_calls_per_turn,
                }),
            )
            .await;
            return Ok(draft_message);
        }
        let remaining_summary_budget = cfg
            .max_summary_calls_per_turn
            .saturating_sub(summary_calls_used);
        let composed = if remaining_summary_budget == 0 {
            ComposeAssistantOutcome {
                message: draft_message,
                quality_gate: Some("REVISE: summary call budget exhausted".to_string()),
                planner_decision: ComposePlannerDecision::Finalize,
                summary_calls: 0,
            }
        } else {
            compose_assistant_message(
                summary_model,
                &cfg.sieve_home,
                event_log,
                run_id,
                trusted_user_message,
                &response_input,
                &render_refs,
                draft_message,
                &mut compose_evidence_cache,
                remaining_summary_budget,
            )
            .await
        };
        summary_calls_used = summary_calls_used.saturating_add(composed.summary_calls);

        if let ComposePlannerDecision::Continue(signal) = composed.planner_decision {
            let mut can_continue = planner_steps_taken < planner_step_hard_limit
                && compose_followup_cycles < max_compose_followup_cycles;
            let mut continue_block_reason: Option<&str> = None;
            if can_continue && summary_calls_used >= cfg.max_summary_calls_per_turn {
                can_continue = false;
                continue_block_reason = Some("summary_budget_exhausted");
            }
            if can_continue && !compose_continue_fingerprints.insert(evidence_fingerprint.clone()) {
                can_continue = false;
                continue_block_reason = Some("no_new_evidence");
            }
            append_turn_controller_event(
                event_log,
                run_id,
                "compose_decision",
                serde_json::json!({
                    "planner_decision_code": signal.code(),
                    "quality_gate_len": composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                    "planner_steps_taken": planner_steps_taken,
                    "planner_step_limit": planner_step_limit,
                    "planner_step_hard_limit": planner_step_hard_limit,
                    "compose_followup_cycles": compose_followup_cycles,
                    "continue": can_continue,
                    "continue_block_reason": continue_block_reason,
                    "summary_calls_used": summary_calls_used,
                    "summary_call_budget": cfg.max_summary_calls_per_turn,
                }),
            )
            .await;
            if can_continue {
                compose_followup_cycles = compose_followup_cycles.saturating_add(1);
                planner_step_limit = planner_step_limit
                    .saturating_add(1)
                    .min(planner_step_hard_limit.max(1));
                planner_guidance = Some(PlannerGuidanceFrame {
                    code: signal.code(),
                    confidence_bps: 9_000,
                    source_hit_index: None,
                    evidence_ref_index: None,
                });
                continue;
            }
        }

        append_turn_controller_event(
            event_log,
            run_id,
            "turn_finalize",
            serde_json::json!({
                "planner_steps_taken": planner_steps_taken,
                "planner_step_limit": planner_step_limit,
                "planner_step_hard_limit": planner_step_hard_limit,
                "compose_followup_cycles": compose_followup_cycles,
                "quality_gate_len": composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                "summary_calls_used": summary_calls_used,
                "summary_call_budget": cfg.max_summary_calls_per_turn,
            }),
        )
        .await;
        return Ok(composed.message);
    }
}

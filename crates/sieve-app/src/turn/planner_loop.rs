use super::response_refs::response_evidence_fingerprint;
use crate::compose::{
    compose_assistant_message, ComposeAssistantOutcome, ComposePlannerDecision, ComposeRequest,
};
use crate::config::persist_runtime_approval_allowances;
use crate::ingress::TurnKind;
use crate::json_util::to_json_value;
use crate::logging::{append_turn_controller_event, now_ms};
use crate::planner_conversation::history_entries_to_planner_messages;
use crate::planner_conversation::{build_planner_conversation, planner_step_trace_messages};
use crate::planner_feedback::{planner_memory_feedback, planner_policy_feedback};
use crate::planner_products::PlannerOpaqueHandleStore;
use crate::planner_progress::{
    build_guidance_prompt, guidance_continue_decision, has_repeated_bash_outcome,
    progress_contract_override_signal,
};
use chrono::{SecondsFormat, TimeZone, Utc};
use sieve_runtime::{PlannerRunRequest, PlannerRunResult};
use sieve_types::{
    PlannerConversationMessage, PlannerGuidanceFrame, PlannerGuidanceInput, PlannerGuidanceSignal,
};
use std::collections::{BTreeMap, BTreeSet};

mod codex_status;
mod compose_continue;
mod errors;
mod heartbeat;
mod payloads;
mod response_draft;
mod types;

use codex_status::{
    find_referenced_codex_session, format_codex_session_status_reply,
    looks_like_codex_status_query, reply_to_codex_session_id,
};
use compose_continue::{continue_guidance_frame, handle_compose_continue, ComposeContinueRequest};
pub(super) use errors::emit_assistant_error_message;
use heartbeat::{finalize_heartbeat_turn, HeartbeatFinalizeRequest};
use payloads::{
    PlannerGuidancePayload, PlannerRepeatGuardPayload, StepCounterPayload, TurnFinalizePayload,
};
use response_draft::{write_response_draft, WriteResponseDraftRequest};
pub(super) use types::{GenerateAssistantRequest, GeneratedAssistantMessage};

pub(super) async fn generate_assistant_message(
    request: GenerateAssistantRequest<'_>,
) -> Result<GeneratedAssistantMessage, Box<dyn std::error::Error>> {
    let deps = request.deps;
    let runtime = deps.runtime.as_ref();
    let guidance_model = deps.guidance_model.as_ref();
    let response_model = deps.response_model.as_ref();
    let summary_model = deps.summary_model.clone();
    let lcm = deps.lcm.clone();
    let event_log = deps.event_log.as_ref();
    let cfg = &deps.cfg;
    let run_id = request.run_id;
    let session_key = request.session_key;
    let trusted_user_message = request.trusted_user_message;
    let response_modality = request.response_modality;
    let turn_kind = request.turn_kind;

    if matches!(turn_kind, TurnKind::User) {
        let codex_sessions = runtime.planner_codex_sessions().await?;
        if let Some(session) = find_referenced_codex_session(trusted_user_message, &codex_sessions)
        {
            if looks_like_codex_status_query(trusted_user_message) {
                return Ok(GeneratedAssistantMessage::Deliver {
                    message: format_codex_session_status_reply(session),
                    reply_to_session_id: Some(session.session_id.clone()),
                });
            }
        }
    }

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
    let mut planner_trace_messages: Vec<PlannerConversationMessage> = Vec::new();
    let mut opaque_handle_store = PlannerOpaqueHandleStore::default();
    let planner_history_messages = if let Some(memory) = lcm.as_ref() {
        match memory.planner_context_for_session(session_key, None).await {
            Ok(context) => context.messages,
            Err(err) => {
                eprintln!("lcm planner context failed for {}: {}", run_id.0, err);
                history_entries_to_planner_messages(&event_log.snapshot_conversation(session_key))
            }
        }
    } else {
        history_entries_to_planner_messages(&event_log.snapshot_conversation(session_key))
    };

    loop {
        while planner_steps_taken < planner_step_limit {
            let step_number = planner_steps_taken + 1;
            let policy_feedback = planner_policy_feedback(&aggregated_result.tool_results);
            let memory_feedback = planner_memory_feedback(&aggregated_result.tool_results).await;
            let has_known_value_refs = runtime.has_known_value_refs()?;
            let allowed_tools_for_turn = super::planner_allowed_tools_for_turn(
                &cfg.allowed_tools,
                has_known_value_refs,
                sieve_command_summaries::command_available("bash"),
                runtime.has_automation_tool(),
                runtime.has_codex_tool(),
            );
            let browser_sessions = runtime.planner_browser_sessions()?;
            let codex_sessions = runtime.planner_codex_sessions().await?;
            let planner_conversation = build_planner_conversation(
                &planner_history_messages,
                policy_feedback.as_deref(),
                memory_feedback.as_deref(),
                &planner_trace_messages,
            );
            let current_time_utc = Utc
                .timestamp_millis_opt(now_ms() as i64)
                .single()
                .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true));
            runtime
                .set_bash_placeholder_values(run_id, opaque_handle_store.placeholder_values())?;
            let step_result = match runtime
                .orchestrate_planner_turn(PlannerRunRequest {
                    run_id: run_id.clone(),
                    cwd: cfg.runtime_cwd.clone(),
                    user_message: trusted_user_message.to_string(),
                    conversation: planner_conversation,
                    allowed_tools: allowed_tools_for_turn,
                    current_time_utc,
                    current_timezone: Some("UTC".to_string()),
                    allowed_net_connect_scopes: cfg.allowed_net_connect_scopes.clone(),
                    browser_sessions,
                    codex_sessions,
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
            let step_products = opaque_handle_store.record_step_products(&step_results);
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
                    to_json_value(
                        PlannerRepeatGuardPayload {
                            step_number,
                            planner_steps_taken,
                            reason: "detected repeated bash command/result; forcing action-change guidance",
                            should_continue: can_retry,
                            next_signal_code: PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                        },
                        "planner repeat guard payload",
                    ),
                )
                .await;
                let guidance_frame = PlannerGuidanceFrame {
                    code: PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                    confidence_bps: 9_000,
                    source_hit_index: None,
                    evidence_ref_index: None,
                };
                planner_trace_messages.extend(planner_step_trace_messages(
                    step_number,
                    &step_results,
                    &guidance_frame,
                    &step_products,
                ));
                if can_retry {
                    planner_guidance = Some(guidance_frame);
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
                        to_json_value(
                            StepCounterPayload {
                                step_number,
                                planner_steps_taken,
                            },
                            "planner guidance error payload",
                        ),
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
                        to_json_value(
                            StepCounterPayload {
                                step_number,
                                planner_steps_taken,
                            },
                            "planner guidance invalid payload",
                        ),
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
            let (mut should_continue, next_step_limit, auto_extended_limit) =
                guidance_continue_decision(
                    effective_signal,
                    consecutive_empty_steps,
                    planner_steps_taken,
                    planner_step_limit,
                    planner_step_hard_limit,
                );
            if step_tool_count == 0
                && matches!(
                    effective_signal,
                    PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint
                )
            {
                should_continue = false;
            }
            planner_step_limit = next_step_limit;
            append_turn_controller_event(
                event_log,
                run_id,
                "planner_guidance",
                to_json_value(
                    PlannerGuidancePayload {
                        step_number,
                        signal_code: signal.code(),
                        effective_signal_code: effective_signal.code(),
                        override_reason: override_applied.map(|(_, reason)| reason.to_string()),
                        should_continue,
                        step_tool_count,
                        planner_steps_taken,
                        planner_step_limit,
                        planner_step_hard_limit,
                        auto_extended_limit,
                        consecutive_empty_steps,
                    },
                    "planner guidance payload",
                ),
            )
            .await;
            let mut guidance_frame = guidance_output.guidance;
            guidance_frame.code = effective_signal.code();
            planner_trace_messages.extend(planner_step_trace_messages(
                step_number,
                &step_results,
                &guidance_frame,
                &step_products,
            ));
            planner_guidance = Some(guidance_frame);
            if !should_continue {
                break;
            }
        }

        if matches!(turn_kind, TurnKind::Heartbeat { .. }) {
            return Ok(finalize_heartbeat_turn(HeartbeatFinalizeRequest {
                event_log,
                run_id,
                thoughts: aggregated_result.thoughts.as_deref(),
                planner_steps_taken,
                planner_step_limit,
                planner_step_hard_limit,
                compose_followup_cycles,
            })
            .await);
        }

        let response_draft = match write_response_draft(WriteResponseDraftRequest {
            response_model,
            summary_model: summary_model.as_ref(),
            run_id,
            trusted_user_message,
            response_modality,
            aggregated_result: &aggregated_result,
            summary_calls_used: &mut summary_calls_used,
            summary_call_budget: cfg.max_summary_calls_per_turn,
        })
        .await
        {
            Ok(response_draft) => response_draft,
            Err(err) => {
                if let Err(log_err) =
                    emit_assistant_error_message(event_log, run_id, format!("error: {err}")).await
                {
                    eprintln!("failed to append assistant error conversation log: {log_err}");
                }
                return Err(err.into());
            }
        };
        let response_input = response_draft.response_input;
        let render_refs = response_draft.render_refs;
        let draft_message = response_draft.draft_message;
        let evidence_fingerprint = response_evidence_fingerprint(&response_input);
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
                to_json_value(
                    TurnFinalizePayload {
                        planner_steps_taken,
                        planner_step_limit,
                        planner_step_hard_limit,
                        compose_followup_cycles,
                        quality_gate_len: 0,
                        summary_calls_used,
                        summary_call_budget: cfg.max_summary_calls_per_turn,
                    },
                    "turn finalize no action payload",
                ),
            )
            .await;
            return Ok(GeneratedAssistantMessage::Deliver {
                message: draft_message,
                reply_to_session_id: reply_to_codex_session_id(
                    trusted_user_message,
                    &aggregated_result,
                    &runtime.planner_codex_sessions().await?,
                ),
            });
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
            compose_assistant_message(ComposeRequest {
                summary_model: summary_model.as_ref(),
                sieve_home: &cfg.sieve_home,
                event_log,
                run_id,
                trusted_user_message,
                response_input: &response_input,
                render_refs: &render_refs,
                draft_message,
                evidence_cache: &mut compose_evidence_cache,
                summary_budget: remaining_summary_budget,
            })
            .await
        };
        summary_calls_used = summary_calls_used.saturating_add(composed.summary_calls);

        if let ComposePlannerDecision::Continue(signal) = composed.planner_decision {
            let can_continue = handle_compose_continue(ComposeContinueRequest {
                event_log,
                run_id,
                composed: &composed,
                signal,
                evidence_fingerprint: &evidence_fingerprint,
                compose_continue_fingerprints: &mut compose_continue_fingerprints,
                planner_steps_taken,
                planner_step_limit: &mut planner_step_limit,
                planner_step_hard_limit,
                compose_followup_cycles: &mut compose_followup_cycles,
                max_compose_followup_cycles,
                summary_calls_used,
                summary_call_budget: cfg.max_summary_calls_per_turn,
            })
            .await;
            if can_continue {
                planner_guidance = Some(continue_guidance_frame(signal));
                continue;
            }
        }

        append_turn_controller_event(
            event_log,
            run_id,
            "turn_finalize",
            to_json_value(
                TurnFinalizePayload {
                    planner_steps_taken,
                    planner_step_limit,
                    planner_step_hard_limit,
                    compose_followup_cycles,
                    quality_gate_len: composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                    summary_calls_used,
                    summary_call_budget: cfg.max_summary_calls_per_turn,
                },
                "turn finalize payload",
            ),
        )
        .await;
        return Ok(GeneratedAssistantMessage::Deliver {
            message: composed.message,
            reply_to_session_id: reply_to_codex_session_id(
                trusted_user_message,
                &aggregated_result,
                &runtime.planner_codex_sessions().await?,
            ),
        });
    }
}

#[cfg(test)]
#[path = "planner_loop_tests.rs"]
mod tests;

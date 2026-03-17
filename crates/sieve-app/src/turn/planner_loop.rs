use super::response_refs::{
    build_response_evidence_records, build_response_turn_input, non_empty_output_ref_ids,
    requires_output_visibility, response_evidence_fingerprint,
    response_has_visible_selected_output,
};
use crate::automation::{parse_heartbeat_planner_action, HeartbeatPlannerAction};
use crate::compose::{compose_assistant_message, ComposeAssistantOutcome, ComposePlannerDecision};
use crate::config::{persist_runtime_approval_allowances, AppConfig};
use crate::ingress::TurnKind;
use crate::logging::{
    append_turn_controller_event, now_ms, ConversationLogRecord, ConversationRole,
    FanoutRuntimeEventLog,
};
use crate::planner_conversation::{build_planner_conversation, planner_step_trace_messages};
use crate::planner_feedback::{planner_memory_feedback, planner_policy_feedback};
use crate::planner_products::PlannerOpaqueHandleStore;
use crate::planner_progress::{
    build_guidance_prompt, guidance_continue_decision, has_repeated_bash_outcome,
    progress_contract_override_signal,
};
use crate::render_refs::render_assistant_message;
use crate::response_style::strip_unexpanded_render_tokens;
use crate::working_state::{
    build_open_loop_from_preference_turn, format_open_loop_status_reply, linked_session_for_loop,
    looks_like_open_loop_confirmation, OpenLoopStore,
};
use chrono::{SecondsFormat, TimeZone, Utc};
use serde::Serialize;
use sieve_llm::{GuidanceModel, ResponseModel, SummaryModel};
use sieve_runtime::{
    EventLogError, PlannerRunRequest, PlannerRunResult, PlannerToolResult, RuntimeEventLog,
    RuntimeOrchestrator,
};
use sieve_types::PlannerCodexSession;
use sieve_types::{
    AssistantMessageEvent, InteractionModality, PlannerConversationMessage, PlannerGuidanceFrame,
    PlannerGuidanceInput, PlannerGuidanceSignal, RunId, RuntimeEvent,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GeneratedAssistantMessage {
    Deliver {
        message: String,
        reply_to_session_id: Option<String>,
    },
    SuppressHeartbeat,
}

#[derive(Serialize)]
struct PlannerRepeatGuardPayload {
    step_number: usize,
    planner_steps_taken: usize,
    reason: &'static str,
    #[serde(rename = "continue")]
    should_continue: bool,
    next_signal_code: u16,
}

#[derive(Serialize)]
struct StepCounterPayload {
    step_number: usize,
    planner_steps_taken: usize,
}

#[derive(Serialize)]
struct PlannerGuidancePayload {
    step_number: usize,
    signal_code: u16,
    effective_signal_code: u16,
    override_reason: Option<String>,
    #[serde(rename = "continue")]
    should_continue: bool,
    step_tool_count: usize,
    planner_steps_taken: usize,
    planner_step_limit: usize,
    planner_step_hard_limit: usize,
    auto_extended_limit: bool,
    consecutive_empty_steps: usize,
}

#[derive(Serialize)]
struct HeartbeatFinalizePayload {
    planner_steps_taken: usize,
    planner_step_limit: usize,
    planner_step_hard_limit: usize,
    compose_followup_cycles: usize,
    delivered: bool,
}

#[derive(Serialize)]
struct TurnFinalizePayload {
    planner_steps_taken: usize,
    planner_step_limit: usize,
    planner_step_hard_limit: usize,
    compose_followup_cycles: usize,
    quality_gate_len: usize,
    summary_calls_used: usize,
    summary_call_budget: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    open_loop_id: Option<String>,
}

#[derive(Serialize)]
struct ComposeDecisionPayload {
    planner_decision_code: u16,
    quality_gate_len: usize,
    planner_steps_taken: usize,
    planner_step_limit: usize,
    planner_step_hard_limit: usize,
    compose_followup_cycles: usize,
    #[serde(rename = "continue")]
    should_continue: bool,
    continue_block_reason: Option<&'static str>,
    summary_calls_used: usize,
    summary_call_budget: usize,
}

fn to_json_value<T: Serialize>(value: T, context: &str) -> serde_json::Value {
    serde_json::to_value(value)
        .unwrap_or_else(|err| panic!("failed to serialize {context}: {err}"))
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
            reply_to_session_id: None,
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
    session_key: &str,
    trusted_user_message: &str,
    response_modality: InteractionModality,
    turn_kind: &TurnKind,
) -> Result<GeneratedAssistantMessage, Box<dyn std::error::Error>> {
    let open_loop_store = OpenLoopStore::new(&cfg.codex_store_path)?;
    if matches!(turn_kind, TurnKind::User) {
        let codex_sessions = runtime.planner_codex_sessions().await?;
        if looks_like_codex_status_query(trusted_user_message) {
            if let Some(loop_record) =
                open_loop_store.referenced_loop_for_status(session_key, trusted_user_message)?
            {
                let linked_session = linked_session_for_loop(&loop_record, &codex_sessions);
                return Ok(GeneratedAssistantMessage::Deliver {
                    message: format_open_loop_status_reply(&loop_record, linked_session),
                    reply_to_session_id: linked_session
                        .map(|session| session.session_id.clone())
                        .or_else(|| loop_record.linked_codex_session_id.clone()),
                });
            }
        }
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
    let active_open_loop = if matches!(turn_kind, TurnKind::User) {
        open_loop_store.active_loop_for_followup(session_key, trusted_user_message)?
    } else {
        None
    };
    let mut planner_trace_messages: Vec<PlannerConversationMessage> = Vec::new();
    let mut opaque_handle_store = PlannerOpaqueHandleStore::default();

    loop {
        while planner_steps_taken < planner_step_limit {
            let step_number = planner_steps_taken + 1;
            let policy_feedback = planner_policy_feedback(&aggregated_result.tool_results);
            let memory_feedback = planner_memory_feedback(&aggregated_result.tool_results).await;
            let has_known_value_refs = runtime.has_known_value_refs()?;
            let allowed_tools_for_turn = super::planner_allowed_tools_for_turn(
                &cfg.allowed_tools,
                has_known_value_refs,
                runtime.has_automation_tool(),
                runtime.has_codex_tool(),
            );
            let browser_sessions = runtime.planner_browser_sessions()?;
            let codex_sessions = runtime.planner_codex_sessions().await?;
            let planner_conversation = build_planner_conversation(
                &event_log.snapshot_conversation(session_key),
                policy_feedback.as_deref(),
                memory_feedback.as_deref(),
                active_open_loop.as_ref(),
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
                        override_reason: override_applied
                            .map(|(_, reason)| reason.to_string()),
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
            let heartbeat_message =
                finalize_heartbeat_message(aggregated_result.thoughts.as_deref());
            append_turn_controller_event(
                event_log,
                run_id,
                "heartbeat_finalize",
                to_json_value(
                    HeartbeatFinalizePayload {
                        planner_steps_taken,
                        planner_step_limit,
                        planner_step_hard_limit,
                        compose_followup_cycles,
                        delivered: matches!(heartbeat_message, GeneratedAssistantMessage::Deliver { .. }),
                    },
                    "heartbeat finalize payload",
                ),
            )
            .await;
            return Ok(heartbeat_message);
        }

        if aggregated_result.tool_results.is_empty()
            && matches!(
                planner_guidance
                    .as_ref()
                    .and_then(|guidance| guidance.signal().ok()),
                Some(PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint)
            )
        {
            if let Some(assistant_context) = aggregated_result
                .thoughts
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let open_loop = build_open_loop_from_preference_turn(
                    session_key,
                    run_id,
                    trusted_user_message,
                    assistant_context,
                    active_open_loop.as_ref(),
                    now_ms(),
                );
                open_loop_store.upsert_loop(&open_loop)?;
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
                            open_loop_id: Some(open_loop.loop_id.clone()),
                        },
                        "turn finalize open loop payload",
                    ),
                )
                .await;
                return Ok(GeneratedAssistantMessage::Deliver {
                    message: open_loop.assistant_context.clone(),
                    reply_to_session_id: None,
                });
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
                to_json_value(
                    TurnFinalizePayload {
                        planner_steps_taken,
                        planner_step_limit,
                        planner_step_hard_limit,
                        compose_followup_cycles,
                        quality_gate_len: 0,
                        summary_calls_used,
                        summary_call_budget: cfg.max_summary_calls_per_turn,
                        open_loop_id: None,
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
                to_json_value(
                    ComposeDecisionPayload {
                        planner_decision_code: signal.code(),
                        quality_gate_len: composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                        planner_steps_taken,
                        planner_step_limit,
                        planner_step_hard_limit,
                        compose_followup_cycles,
                        should_continue: can_continue,
                        continue_block_reason,
                        summary_calls_used,
                        summary_call_budget: cfg.max_summary_calls_per_turn,
                    },
                    "compose decision payload",
                ),
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

        if let Some(loop_record) = active_open_loop.as_ref() {
            if let Some((session_id, session_name)) =
                first_codex_session_result_identity(&aggregated_result)
            {
                open_loop_store.mark_executing(
                    &loop_record.loop_id,
                    session_id.as_deref(),
                    Some(&session_name),
                    now_ms(),
                )?;
            } else if !aggregated_result.tool_results.is_empty()
                && looks_like_open_loop_confirmation(trusted_user_message)
            {
                open_loop_store.mark_executing(&loop_record.loop_id, None, None, now_ms())?;
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
                    open_loop_id: None,
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

fn looks_like_codex_status_query(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "how is ", "how's ", "status", "doing", "done", "ongoing", "progress", "finished",
        "complete",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn find_referenced_codex_session<'a>(
    message: &str,
    sessions: &'a [PlannerCodexSession],
) -> Option<&'a PlannerCodexSession> {
    let lower = message.to_ascii_lowercase();
    sessions
        .iter()
        .filter_map(|session| {
            let mut score = 0usize;
            if lower.contains(&session.session_name.to_ascii_lowercase()) {
                score = score.max(100);
            }
            if let Some(name) = session
                .cwd
                .rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
            {
                if lower.contains(&name.to_ascii_lowercase()) {
                    score = score.max(80);
                }
            }
            if lower.contains(&session.task_summary.to_ascii_lowercase()) {
                score = score.max(60);
            }
            (score > 0).then_some((score, session))
        })
        .max_by_key(|(score, session)| (*score, session.updated_at_utc.as_str()))
        .map(|(_, session)| session)
}

fn format_codex_session_status_reply(session: &PlannerCodexSession) -> String {
    match session.status.as_str() {
        "completed" => format!(
            "It’s done, not ongoing. The saved session `{}` is marked `completed`, last updated at `{}`, and its summary says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("the work completed")
        ),
        "failed" => format!(
            "It failed. The saved session `{}` is marked `failed`, last updated at `{}`, and the latest summary says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("the last Codex turn failed")
        ),
        "needs_followup" => format!(
            "It is not done yet. The saved session `{}` needs follow-up, last updated at `{}`, and the latest summary says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("more work remains")
        ),
        "waiting_approval" => format!(
            "It’s waiting on approval right now. The saved session `{}` last updated at `{}`, and the latest status says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("Codex requested approval before it can continue")
        ),
        _ => format!(
            "It’s still running. The saved session `{}` is marked `{}`, last updated at `{}`, and the task summary is {}.",
            session.session_name,
            session.status,
            session.updated_at_utc,
            session.task_summary
        ),
    }
}

fn reply_to_codex_session_id(
    trusted_user_message: &str,
    aggregated_result: &PlannerRunResult,
    codex_sessions: &[PlannerCodexSession],
) -> Option<String> {
    aggregated_result
        .tool_results
        .iter()
        .rev()
        .find_map(|tool_result| match tool_result {
            PlannerToolResult::CodexSession {
                result: Some(result),
                ..
            } => result.session_id.clone(),
            _ => None,
        })
        .or_else(|| {
            find_referenced_codex_session(trusted_user_message, codex_sessions)
                .map(|session| session.session_id.clone())
        })
}

fn first_codex_session_result_identity(
    aggregated_result: &PlannerRunResult,
) -> Option<(Option<String>, String)> {
    aggregated_result
        .tool_results
        .iter()
        .rev()
        .find_map(|tool_result| match tool_result {
            PlannerToolResult::CodexSession {
                result: Some(result),
                ..
            } => Some((result.session_id.clone(), result.session_name.clone())),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::CodexSandboxMode;

    fn sample_session() -> PlannerCodexSession {
        PlannerCodexSession {
            session_id: "codex-session-1".to_string(),
            session_name: "you-are-starting".to_string(),
            cwd: "/root/git/modex".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            updated_at_utc: "2026-03-10T17:36:42Z".to_string(),
            status: "completed".to_string(),
            task_summary: "build modex".to_string(),
            last_result_summary: Some("a passing npm run build".to_string()),
        }
    }

    #[test]
    fn codex_status_query_detection_matches_recent_phrase() {
        assert!(looks_like_codex_status_query(
            "how is you-are-starting doing?"
        ));
        assert!(looks_like_codex_status_query(
            "is you-are-starting done or is the work ongoing?"
        ));
        assert!(!looks_like_codex_status_query("resume you-are-starting"));
    }

    #[test]
    fn find_referenced_codex_session_matches_session_name() {
        let sessions = vec![sample_session()];
        let matched = find_referenced_codex_session("how is you-are-starting doing?", &sessions)
            .expect("match codex session");
        assert_eq!(matched.session_id, "codex-session-1");
    }

    #[test]
    fn find_referenced_codex_session_matches_cwd_basename() {
        let sessions = vec![sample_session()];
        let matched =
            find_referenced_codex_session("how is modex work going?", &sessions).expect("match");
        assert_eq!(matched.session_id, "codex-session-1");
    }

    #[test]
    fn format_codex_session_status_reply_uses_saved_metadata() {
        let message = format_codex_session_status_reply(&sample_session());
        assert!(message.contains("It’s done, not ongoing."));
        assert!(message.contains("`you-are-starting`"));
        assert!(message.contains("`2026-03-10T17:36:42Z`"));
        assert!(message.contains("a passing npm run build"));
    }
}

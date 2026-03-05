use crate::compose::{compose_assistant_message, ComposeAssistantOutcome, ComposePlannerDecision};
use crate::config::{persist_runtime_approval_allowances, AppConfig};
use crate::ingress::PromptSource;
use crate::lcm_integration::LcmIntegration;
use crate::logging::{
    append_turn_controller_event, now_ms, ConversationLogRecord, ConversationRole,
    FanoutRuntimeEventLog,
};
use crate::media;
use crate::planner_feedback::{planner_memory_feedback, planner_policy_feedback};
use crate::planner_progress::{
    build_guidance_prompt, guidance_continue_decision, has_repeated_bash_outcome,
    progress_contract_override_signal,
};
use crate::render_refs::{render_assistant_message, RenderRef};
use crate::response_style::strip_unexpanded_render_tokens;
use async_trait::async_trait;
use sieve_llm::{
    GuidanceModel, ResponseModel, ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput,
    SummaryModel, SummaryRequest,
};
use sieve_runtime::{
    EventLogError, MainlineArtifact, MainlineArtifactKind, MainlineRunError, MainlineRunReport,
    MainlineRunRequest, MainlineRunner, PlannerRunRequest, PlannerRunResult, PlannerToolResult,
    RuntimeDisposition, RuntimeEventLog, RuntimeOrchestrator,
};
use sieve_types::{
    AssistantMessageEvent, Integrity, InteractionModality, ModalityContract,
    ModalityOverrideReason, PlannerGuidanceFrame, PlannerGuidanceInput, PlannerGuidanceSignal,
    RunId, RuntimeEvent,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::process::Command as TokioCommand;

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

pub(crate) struct AppMainlineRunner {
    artifact_root: PathBuf,
    next_artifact_id: AtomicU64,
}

impl AppMainlineRunner {
    pub(crate) fn new(artifact_root: PathBuf) -> Self {
        Self {
            artifact_root,
            next_artifact_id: AtomicU64::new(1),
        }
    }

    fn next_ref_id(&self) -> String {
        let next = self.next_artifact_id.fetch_add(1, Ordering::Relaxed);
        format!("artifact-{}-{next}", now_ms())
    }

    async fn persist_artifact(
        &self,
        run_id: &RunId,
        kind: MainlineArtifactKind,
        bytes: &[u8],
    ) -> Result<MainlineArtifact, MainlineRunError> {
        let ref_id = self.next_ref_id();
        let kind_name = match kind {
            MainlineArtifactKind::Stdout => "stdout",
            MainlineArtifactKind::Stderr => "stderr",
        };
        let run_dir = self.artifact_root.join(&run_id.0);
        tokio::fs::create_dir_all(&run_dir)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("create artifact dir failed: {err}")))?;
        let path = run_dir.join(format!("{ref_id}-{kind_name}.log"));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("persist artifact failed: {err}")))?;

        Ok(MainlineArtifact {
            ref_id,
            kind,
            path: path.to_string_lossy().to_string(),
            byte_count: bytes.len() as u64,
            line_count: count_newlines(bytes),
        })
    }
}

#[async_trait]
impl MainlineRunner for AppMainlineRunner {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        let output = TokioCommand::new("bash")
            .arg("-lc")
            .arg(&request.script)
            .current_dir(&request.cwd)
            .output()
            .await
            .map_err(|err| MainlineRunError::Exec(err.to_string()))?;

        let stdout_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stdout,
                &output.stdout,
            )
            .await?;
        let stderr_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stderr,
                &output.stderr,
            )
            .await?;

        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: output.status.code(),
            artifacts: vec![stdout_artifact, stderr_artifact],
        })
    }
}

fn count_newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|byte| **byte == b'\n').count() as u64
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

fn response_evidence_fingerprint(input: &ResponseTurnInput) -> String {
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

pub(crate) fn mainline_artifact_kind_name(kind: MainlineArtifactKind) -> &'static str {
    match kind {
        MainlineArtifactKind::Stdout => "stdout",
        MainlineArtifactKind::Stderr => "stderr",
    }
}

pub(crate) fn format_integrity(integrity: Integrity) -> &'static str {
    match integrity {
        Integrity::Trusted => "trusted",
        Integrity::Untrusted => "untrusted",
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

pub(crate) fn default_modality_contract(input: InteractionModality) -> ModalityContract {
    ModalityContract {
        input,
        response: input,
        override_reason: None,
    }
}

pub(crate) fn override_modality_contract(
    contract: &mut ModalityContract,
    response: InteractionModality,
    reason: ModalityOverrideReason,
) {
    contract.response = response;
    contract.override_reason = Some(reason);
}

async fn emit_assistant_error_message(
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

pub(crate) async fn run_turn(
    runtime: &RuntimeOrchestrator,
    guidance_model: &dyn GuidanceModel,
    response_model: &dyn ResponseModel,
    summary_model: &dyn SummaryModel,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_index: u64,
    source: PromptSource,
    input_modality: InteractionModality,
    media_file_id: Option<String>,
    user_message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_id = RunId(format!("run-{run_index}"));
    let mut modality_contract = default_modality_contract(input_modality);
    if modality_contract.response == InteractionModality::Image {
        override_modality_contract(
            &mut modality_contract,
            InteractionModality::Text,
            ModalityOverrideReason::NotSupported,
        );
    }
    let (trusted_user_message, input_error) = match input_modality {
        InteractionModality::Text => (user_message.clone(), None),
        InteractionModality::Audio => match media_file_id.as_deref() {
            Some(file_id) => match media::transcribe_audio_prompt(cfg, &run_id, file_id).await {
                Ok(transcript) => (transcript, None),
                Err(err) => (
                    String::new(),
                    Some(format!("audio input unavailable: {err}")),
                ),
            },
            None => (
                String::new(),
                Some("audio input missing media file id".to_string()),
            ),
        },
        InteractionModality::Image => match media_file_id.as_deref() {
            Some(file_id) => {
                match media::extract_image_prompt(
                    &cfg.telegram_bot_token,
                    &cfg.sieve_home,
                    &run_id,
                    file_id,
                )
                .await
                {
                    Ok(extracted) => (extracted, None),
                    Err(err) => (
                        String::new(),
                        Some(format!("image input unavailable: {err}")),
                    ),
                }
            }
            None => (
                String::new(),
                Some("image input missing media file id".to_string()),
            ),
        },
    };
    if let Some(error_message) = input_error {
        println!("{}: {}", run_id.0, error_message);
        emit_assistant_error_message(event_log, &run_id, error_message).await?;
        return Ok(());
    }

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_user_message(&trusted_user_message).await {
            eprintln!("lcm ingest user failed for {}: {}", run_id.0, err);
        }
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::User,
            trusted_user_message.clone(),
            now_ms(),
        ))
        .await?;

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
    let planner_user_message = trusted_user_message.clone();

    let assistant_message = loop {
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
            let allowed_tools_for_turn =
                planner_allowed_tools_for_turn(&cfg.allowed_tools, has_known_value_refs);
            let step_result = match runtime
                .orchestrate_planner_turn(PlannerRunRequest {
                    run_id: run_id.clone(),
                    cwd: cfg.runtime_cwd.clone(),
                    user_message: planner_turn_user_message,
                    allowed_tools: allowed_tools_for_turn,
                    allowed_net_connect_scopes: cfg.allowed_net_connect_scopes.clone(),
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
                        emit_assistant_error_message(event_log, &run_id, format!("error: {err}"))
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
                    &cfg.sieve_home,
                    &run_id,
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
                &trusted_user_message,
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
                        &cfg.sieve_home,
                        &run_id,
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
                        &cfg.sieve_home,
                        &run_id,
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
                &trusted_user_message,
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
                &cfg.sieve_home,
                &run_id,
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
            &run_id,
            &trusted_user_message,
            modality_contract.response,
            &aggregated_result,
        );
        let mut response_input = response_input;
        let mut response_output = match response_model
            .write_turn_response(response_input.clone())
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if let Err(log_err) =
                    emit_assistant_error_message(event_log, &run_id, format!("error: {err}")).await
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
                        emit_assistant_error_message(event_log, &run_id, format!("error: {err}"))
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
                &run_id,
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
                    &run_id,
                )
                .await
            } else {
                stripped
            }
        };
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
                &run_id,
                &trusted_user_message,
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
                &cfg.sieve_home,
                &run_id,
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
            &cfg.sieve_home,
            &run_id,
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
        break composed.message;
    };
    println!("{}: {}", run_id.0, assistant_message);

    let mut delivered_audio = false;
    if source == PromptSource::Telegram && modality_contract.response == InteractionModality::Audio
    {
        match media::synthesize_audio_reply(cfg, &run_id, &assistant_message).await {
            Ok(audio_path) => {
                if let Err(err) = media::send_telegram_voice(
                    &cfg.telegram_bot_token,
                    cfg.telegram_chat_id,
                    &audio_path,
                )
                .await
                {
                    eprintln!("audio reply delivery failed for {}: {}", run_id.0, err);
                    override_modality_contract(
                        &mut modality_contract,
                        InteractionModality::Text,
                        ModalityOverrideReason::ToolFailure,
                    );
                } else {
                    delivered_audio = true;
                }
            }
            Err(err) => {
                eprintln!("audio synthesis failed for {}: {}", run_id.0, err);
                override_modality_contract(
                    &mut modality_contract,
                    InteractionModality::Text,
                    ModalityOverrideReason::ToolFailure,
                );
            }
        }
    }

    if !delivered_audio {
        event_log
            .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
                schema_version: 1,
                run_id: run_id.clone(),
                message: assistant_message.clone(),
                created_at_ms: now_ms(),
            }))
            .await?;
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            assistant_message.clone(),
            now_ms(),
        ))
        .await?;

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_assistant_message(&assistant_message).await {
            eprintln!("lcm ingest assistant failed for {}: {}", run_id.0, err);
        }
    }
    Ok(())
}
